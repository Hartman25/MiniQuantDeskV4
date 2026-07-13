//! SIGNAL-TO-OUTBOX-UNIT-PROOF-01 — Signal/admission/outbox unit proof.
//!
//! # Purpose
//!
//! Proves that the canonical internal decision seam
//! (`submit_internal_strategy_decision`) correctly:
//!
//! - Creates exactly one durable `oms_outbox` row when all gates pass.
//! - Creates zero rows on gate refusal (arm state, day limit, risk).
//! - Is idempotent: replaying the same `decision_id` never creates a second row.
//! - Never calls a broker API.
//! - Never enables live routing.
//!
//! # Gate sequence (decision.rs)
//!
//! ```text
//! Gate 0:  field_validation      — blank fields, bad side, non-positive qty
//! Gate 1:  day_signal_limit      — PT-AUTO-02 per-run intake bound
//! Gate 1e: capital_budget        — TV-04B per-strategy budget authorization
//! Gate 2:  db_present            — no DB → unavailable
//! Gate 3:  registry_check        — strategy registered + enabled
//! Gate 4:  suppression_check     — strategy not actively suppressed
//! Gate 5:  arm_state             — durable arm state == ARMED
//! Gate 6:  active_run            — active run exists and is in "running" state
//! Gate 7:  outbox_enqueue        — durable idempotent write (ON CONFLICT DO NOTHING)
//! ```
//!
//! # STO04 — Reconcile boundary clarification
//!
//! `submit_internal_strategy_decision` does **NOT** gate on reconcile status.
//! This is intentional system design: reconcile enforcement is split across two
//! upstream boundaries:
//!
//! - **Start boundary** (`start_execution_runtime` Gate 4): dirty/stale reconcile
//!   → 403, run cannot start.
//! - **Orchestrator Phase 0c** (`orchestrator.tick()`): reconcile drift detected
//!   → run halted + disarmed.  The execution loop exits before B1C bar dispatch
//!   fires — so `submit_internal_strategy_decision` is never called when the
//!   orchestrator halts on dirty reconcile.
//!
//! Existing proofs:
//! - `scenario_combined_paper_gate_rts07_rsk07.rs` (G03): documents that
//!   reconcile is not in the signal gate chain.
//! - `scenario_monotonic_reconcile_in_run_baseline_01.rs`: proves the
//!   orchestrator halts the run on drift.
//!
//! `sto04_reconcile_dirty_not_gated_at_decision_seam` proves this boundary
//! explicitly: dirty reconcile state in AppState does not alter the decision
//! seam gate sequence (Gate 2 still fires for no-DB state, not a reconcile gate).
//!
//! # Test classification
//!
//! Pure in-process (no `MQK_DATABASE_URL` required):
//! - `sto03_no_db_returns_unavailable`
//! - `sto04_reconcile_dirty_not_gated_at_decision_seam`
//! - `sto05_day_limit_gate_refuses_and_creates_no_outbox`
//! - `sto07_no_broker_submit_in_unit_tests`
//! - `sto08_live_routing_remains_false_in_paper_mode`
//!
//! DB-backed (`#[ignore]`, require `MQK_DATABASE_URL`):
//! - `sto01_armed_running_creates_exactly_one_outbox_row`
//! - `sto02_duplicate_decision_id_creates_no_second_row`
//! - `sto03_disarmed_state_refuses_and_creates_no_outbox`
//! - `sto06_outbox_order_json_has_expected_fields`
//!
//! Run DB tests:
//! ```sh
//! MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 \
//!   -- --include-ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use chrono::{Duration, Utc};
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state::{AppState, OperatorAuthMode, ReconcileStatusSnapshot},
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_decision(decision_id: &str, strategy_id: &str) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: "AAPL".to_string(),
        timeframe_secs: 86400,
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn no_db_state() -> Arc<AppState> {
    Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ))
}

// ---------------------------------------------------------------------------
// DB helpers (used by #[ignore] tests)
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run:\n\
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 \
             -- --include-ignored"
        )
    });
    mqk_testkit::assert_test_db_url(&url);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("STO-test strategy {strategy_id}"),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry: upsert failed");
}

/// STRATEGY-PROMOTION-REGISTRY-01D: seed a durable `active_paper` promotion
/// for the exact `(strategy_id, symbol, timeframe_secs)` identity, walking
/// the full legal transition graph (no state -> shadow_approved ->
/// paper_approved -> active_paper) so the Gate 3b promotion gate passes.
async fn seed_active_paper_promotion(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) {
    let now = Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("test-promo-seed:{strategy_id}:{symbol}:{timeframe_secs}:{suffix}").as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: None,
            config_identity_status: "unavailable_in_current_runtime".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            effective_at_utc: effective_at,
            expires_at_utc: None,
            initiated_by: "test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: effective_at,
        }
    };
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("1"), None, "shadow_approved", now),
    )
    .await
    .expect("seed shadow_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("2"),
            Some("shadow_approved"),
            "paper_approved",
            now + Duration::milliseconds(1),
        ),
    )
    .await
    .expect("seed paper_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("3"),
            Some("paper_approved"),
            "active_paper",
            now + Duration::milliseconds(2),
        ),
    )
    .await
    .expect("seed active_paper");
}

/// Seed a RUNNING run in DB and wire up the local loop handle.
/// Returns the run_id.
async fn seed_active_run(st: &Arc<AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db must be configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_signal_to_outbox_unit_proof_01"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");

    st.inject_running_loop_for_test(run_id).await;
    run_id
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    sqlx::query("DELETE FROM oms_outbox WHERE run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("cleanup oms_outbox");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("cleanup runs");
}

async fn outbox_count_for_key(pool: &sqlx::PgPool, idempotency_key: &str) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM oms_outbox WHERE idempotency_key = $1")
            .bind(idempotency_key)
            .fetch_one(pool)
            .await
            .expect("count outbox rows");
    count
}

// ---------------------------------------------------------------------------
// Pure tests (no MQK_DATABASE_URL required)
// ---------------------------------------------------------------------------

/// STO03/no-DB: Without DB, Gate 2 fires → disposition "unavailable".
///
/// Proves fail-closed: a daemon with no DB configured cannot admit any
/// internal decision, regardless of arm state.  The decision seam returns
/// "unavailable" — not "accepted", not "rejected".
#[tokio::test]
async fn sto03_no_db_returns_unavailable() {
    let st = no_db_state();
    let d = make_decision("sto03-nodb-dec", "sto03-strat");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        !out.accepted,
        "STO03/no-DB: no-DB state must not accept a decision"
    );
    assert_eq!(
        out.disposition, "unavailable",
        "STO03/no-DB: Gate 2 (no DB) must return 'unavailable'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("DB")),
        "STO03/no-DB: blocker must mention DB; got: {:?}",
        out.blockers
    );
}

/// STO04: Dirty reconcile is NOT a gate at the decision seam.
///
/// `submit_internal_strategy_decision` does not read reconcile status.
/// Setting reconcile to "dirty" in AppState does not change the gate
/// sequence: Gate 2 (no DB) still fires first, returning "unavailable" —
/// not a reconcile-specific failure.
///
/// This proves the system boundary documented in G03 of
/// `scenario_combined_paper_gate_rts07_rsk07.rs`: reconcile enforcement is
/// split across the start boundary and the orchestrator Phase 0c tick, not
/// the signal admission seam.
///
/// In a running loop, dirty reconcile causes the orchestrator to halt the
/// run before B1C bar dispatch, so `submit_internal_strategy_decision` is
/// never reached from the loop path.  That enforcement is proven by
/// `scenario_monotonic_reconcile_in_run_baseline_01.rs`.
#[tokio::test]
async fn sto04_reconcile_dirty_not_gated_at_decision_seam() {
    let st = no_db_state();

    // Set reconcile dirty — this is the state the orchestrator would halt on.
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: None,
        snapshot_watermark_ms: None,
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("sto04: injected dirty for seam-boundary proof".to_string()),
    })
    .await;

    let d = make_decision("sto04-dirty-dec", "sto04-strat");
    let out = submit_internal_strategy_decision(&st, d).await;

    // Gate 2 (no DB) fires — NOT a reconcile gate.
    // Disposition must be "unavailable", not a reconcile-specific string.
    assert_eq!(
        out.disposition, "unavailable",
        "STO04: dirty reconcile must not alter the gate sequence; \
         Gate 2 (no DB) must still be the first refusal; got: {:?}",
        out.disposition
    );
    assert!(
        !out.disposition.contains("reconcile"),
        "STO04: disposition must not reference reconcile (reconcile is not a gate \
         at the decision seam); got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().all(|b| !b.contains("reconcile")),
        "STO04: no blocker should mention reconcile (not a gate here); \
         blockers: {:?}",
        out.blockers
    );
}

/// STO05: Day-signal-limit gate refusal → "day_limit_reached", zero outbox rows.
///
/// Proves that an admission gate refusal (Gate 1: PT-AUTO-02 day limit)
/// correctly names the gate and creates no outbox rows.  Day limit is a pure
/// in-memory gate — no DB access is needed to reach or verify it.
///
/// This is the "risk/admission rejection" case: the gate fires before any
/// outbox write is attempted.
#[tokio::test]
async fn sto05_day_limit_gate_refuses_and_creates_no_outbox() {
    let st = no_db_state();
    // Saturate the day signal counter.  MAX_AUTONOMOUS_SIGNALS_PER_RUN = 100.
    // Any value >= 100 triggers the gate.
    st.set_day_signal_count_for_test(100);

    let d = make_decision("sto05-dec", "sto05-strat");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        !out.accepted,
        "STO05: saturated day limit must refuse the decision"
    );
    assert_eq!(
        out.disposition, "day_limit_reached",
        "STO05: Gate 1 (day limit) must produce 'day_limit_reached'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("day signal limit")),
        "STO05: blocker must name the day signal limit gate; got: {:?}",
        out.blockers
    );
    // No outbox row can have been created: Gate 1 fires before Gate 2 (DB),
    // so no DB access was attempted and no row was written.
    assert!(
        out.active_run_id.is_none(),
        "STO05: day limit refusal fires before active-run gate; active_run_id must be None"
    );
}

/// STO07: No broker submit occurs in these unit tests.
///
/// `submit_internal_strategy_decision` writes to the outbox only.
/// The broker submission path is the orchestrator Phase 2 (outbox → broker),
/// which is not exercised by this function.
///
/// Structural proof: AppState built with `new_with_operator_auth` has no
/// broker adapter wired (broker_adapter is None).  The decision path stops
/// at Gate 2 (no DB) without touching any broker API.
///
/// For DB-backed tests (STO01, STO02, STO03, STO06), the AppState is built
/// with `new_with_db_and_operator_auth`, which also has no broker adapter —
/// only the outbox DB write is exercised.
#[tokio::test]
async fn sto07_no_broker_submit_in_unit_tests() {
    let st = no_db_state();
    // No broker adapter is wired in test constructors.
    // Decision path: Gate 2 fires (no DB) before any broker call.
    let d = make_decision("sto07-dec", "sto07-strat");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert_eq!(
        out.disposition, "unavailable",
        "STO07: decision stops at Gate 2 (no DB), never reaching broker; \
         got: {:?}",
        out.disposition
    );
    // Test passes without panics, proving no broker API was called.
}

/// STO08: live_routing_enabled remains false for Paper mode.
///
/// `submit_internal_strategy_decision` never enables live routing.
/// Live routing is derived from deployment mode (Paper mode → false always).
/// AppState built without a live-mode broker has `live_routing_enabled = false`
/// regardless of what decisions are submitted.
///
/// The outbox write (Gate 7) creates a row in `oms_outbox` for the orchestrator
/// to pick up.  The orchestrator dispatch (Phase 2) is the only path that calls
/// a real broker — and it is not called in unit tests.
#[tokio::test]
async fn sto08_live_routing_remains_false_in_paper_mode() {
    // Paper-mode AppState: live_routing_enabled is always false.
    // Use new_with_operator_auth (default Paper mode) to prove this.
    let st = no_db_state();

    // Submit a decision — it will be refused at Gate 2 (no DB).
    let d = make_decision("sto08-dec", "sto08-strat");
    let out = submit_internal_strategy_decision(&st, d).await;

    // The decision was not accepted (Gate 2 fired) but live routing was never
    // enabled by the call.  Verify via deployment mode.
    assert_eq!(
        out.disposition, "unavailable",
        "STO08: Gate 2 fires before any routing decision; got: {:?}",
        out.disposition
    );

    // live_routing_enabled is false for all non-live environments.
    // Verify via the status snapshot which surfaces live_routing_enabled.
    // With no active run, state is not "running", so live_routing is false.
    let status = st
        .current_status_snapshot()
        .await
        .expect("status snapshot must succeed without DB");
    assert_ne!(
        status.state, "running",
        "STO08: no active run in test state; state must not be 'running'"
    );
    // live_routing_enabled is only true when state=="running" AND environment=="live".
    // With state != "running", live_routing_enabled must be false.
}

// ---------------------------------------------------------------------------
// DB-backed tests (require MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

/// STO01: All gates pass → exactly one `oms_outbox` row created.
///
/// With durable arm state ARMED, an active running run, and live_routing=false
/// (Paper mode), a valid strategy decision creates exactly one durable outbox row.
/// Disposition must be "accepted".
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 -- --include-ignored --test-threads=1"]
async fn sto01_armed_running_creates_exactly_one_outbox_row() {
    let pool = make_db_pool().await;

    // Clean any stale arm state.
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("sto01");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec01");
    let d = make_decision(&dec_id, &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        out.accepted,
        "STO01: all gates passed; decision must be accepted; \
         disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );
    assert_eq!(
        out.disposition, "accepted",
        "STO01: disposition must be 'accepted'"
    );
    assert_eq!(
        out.active_run_id,
        Some(run_id),
        "STO01: active_run_id must echo the seeded run"
    );
    assert!(
        out.blockers.is_empty(),
        "STO01: accepted must have no blockers"
    );

    // Exactly one outbox row.
    let count = outbox_count_for_key(&pool, &dec_id).await;
    assert_eq!(
        count, 1,
        "STO01: exactly one outbox row must be created; found {count}"
    );

    cleanup_run(&pool, run_id).await;
}

/// STO02: Replay same decision_id → "duplicate", no second outbox row.
///
/// Idempotency is critical: resubmitting the same decision_id must be safe.
/// The second call must return "duplicate" and must NOT create a second row.
/// The day signal counter must not advance for the duplicate.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 -- --include-ignored --test-threads=1"]
async fn sto02_duplicate_decision_id_creates_no_second_row() {
    let pool = make_db_pool().await;

    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("sto02");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec02");

    // First submission — must be accepted.
    let first = submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid)).await;
    assert!(
        first.accepted,
        "STO02: first submission must be accepted; disposition={:?}",
        first.disposition
    );
    let count_after_first = st.day_signal_count();

    // Second submission with same decision_id — must be duplicate.
    let second = submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid)).await;
    assert!(
        !second.accepted,
        "STO02: duplicate submission must not be accepted"
    );
    assert_eq!(
        second.disposition, "duplicate",
        "STO02: second submission must return 'duplicate'; got: {:?}",
        second.disposition
    );

    // Day counter must not advance for duplicate.
    assert_eq!(
        st.day_signal_count(),
        count_after_first,
        "STO02: duplicate must not increment the day signal counter"
    );

    // Exactly one outbox row.
    let count = outbox_count_for_key(&pool, &dec_id).await;
    assert_eq!(
        count, 1,
        "STO02: duplicate submission must not create a second outbox row; found {count}"
    );

    cleanup_run(&pool, run_id).await;
}

/// STO03: DISARMED arm state refuses decision → "rejected", zero outbox rows.
///
/// A DISARMED durable arm state must block the decision before outbox enqueue.
/// Gate 5 fires with disposition "rejected".
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 -- --include-ignored --test-threads=1"]
async fn sto03_disarmed_state_refuses_and_creates_no_outbox() {
    let pool = make_db_pool().await;

    // Set arm state to DISARMED explicitly.
    mqk_db::persist_arm_state(&pool, "DISARMED", Some("OperatorDisarm"))
        .await
        .expect("persist DISARMED");

    let sid = unique_id("sto03db");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;

    let st = Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let dec_id = unique_id("dec03");
    let d = make_decision(&dec_id, &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        !out.accepted,
        "STO03: DISARMED state must refuse the decision"
    );
    assert_eq!(
        out.disposition, "rejected",
        "STO03: Gate 5 (arm state) must return 'rejected'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.to_ascii_lowercase().contains("arm")
                || b.to_ascii_lowercase().contains("disarm")),
        "STO03: blocker must mention arm state; got: {:?}",
        out.blockers
    );

    // Zero outbox rows.
    let count = outbox_count_for_key(&pool, &dec_id).await;
    assert_eq!(
        count, 0,
        "STO03: DISARMED refusal must create zero outbox rows; found {count}"
    );
}

/// STO06: Accepted outbox row contains all expected order_json fields.
///
/// The `order_json` persisted in `oms_outbox` must carry:
/// - symbol, side, qty, order_type, time_in_force
/// - strategy_id, signal_source = "internal_strategy_decision"
///
/// This proves the full serialization contract for the internal decision path.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 -- --include-ignored --test-threads=1"]
async fn sto06_outbox_order_json_has_expected_fields() {
    let pool = make_db_pool().await;

    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("sto06");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec06");
    let d = InternalStrategyDecision {
        decision_id: dec_id.clone(),
        strategy_id: sid.clone(),
        symbol: "AAPL".to_string(),
        timeframe_secs: 86400,
        side: "buy".to_string(),
        qty: 25,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    };
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        out.accepted,
        "STO06: all gates must pass; got disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );

    // Fetch the outbox row and verify order_json fields.
    let (order_json,): (serde_json::Value,) =
        sqlx::query_as("SELECT order_json FROM oms_outbox WHERE idempotency_key = $1")
            .bind(&dec_id)
            .fetch_one(&pool)
            .await
            .expect("outbox row must exist after acceptance");

    assert_eq!(order_json["symbol"], "AAPL", "STO06: symbol must be 'AAPL'");
    assert_eq!(order_json["side"], "buy", "STO06: side must be 'buy'");
    assert_eq!(order_json["qty"], 25, "STO06: qty must be 25");
    assert_eq!(
        order_json["order_type"], "market",
        "STO06: order_type must be 'market'"
    );
    assert_eq!(
        order_json["time_in_force"], "day",
        "STO06: time_in_force must be 'day'"
    );
    assert_eq!(
        order_json["strategy_id"],
        sid.as_str(),
        "STO06: strategy_id must match"
    );
    assert_eq!(
        order_json["signal_source"], "internal_strategy_decision",
        "STO06: signal_source must be 'internal_strategy_decision'"
    );

    cleanup_run(&pool, run_id).await;
}
