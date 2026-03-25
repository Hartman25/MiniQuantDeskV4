//! CC-01D: Proof tests for the internal strategy decision-to-intent seam.
//!
//! Proves that `submit_internal_strategy_decision`:
//! - field validation rejects blank / invalid inputs before touching the DB
//! - PT-AUTO-02 day-limit gate fires before the DB is required
//! - no DB → disposition "unavailable"
//! - unregistered strategy → disposition "rejected"
//! - registered but disabled strategy → disposition "rejected"
//! - active suppression → disposition "suppressed" before outbox enqueue (CC-02B)
//! - cleared suppression → no longer blocks entry (CC-02B)
//! - suppression is keyed to strategy identity, does not bleed across strategies (CC-02B)
//! - registered + enabled but arm state not set → disposition "rejected"
//! - all gates pass → disposition "accepted", outbox row inserted
//! - duplicate decision_id → disposition "duplicate", no second outbox row
//! - signal_source in outbox row is "internal_strategy_decision" (not external)
//!
//! No-DB tests run unconditionally.
//! DB-backed tests require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_internal_strategy_decision -- --include-ignored

use std::sync::Arc;

use chrono::Utc;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state,
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

// ---------------------------------------------------------------------------
// DB helper (for #[ignore] tests)
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_internal_strategy_decision -- --include-ignored"
        )
    });
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
            display_name: format!("Test Strategy {strategy_id}"),
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

/// Seed a RUNNING run in the DB and wire up the local loop handle.
/// Returns the run_id.
async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
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
            config_json: serde_json::json!({"source": "scenario_internal_strategy_decision"}),
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

/// Clean up all test-written rows for a given engine / run.
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

// ---------------------------------------------------------------------------
// Gate 0: field validation (no DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 0: blank decision_id → rejected before any DB access.
#[tokio::test]
async fn decision_blank_decision_id_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("", "strat-a");
    d.decision_id = "  ".to_string(); // whitespace-only
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("decision_id")),
        "blockers must mention decision_id; got: {:?}",
        out.blockers
    );
}

/// CC-01D / Gate 0: blank strategy_id → rejected.
#[tokio::test]
async fn decision_blank_strategy_id_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("dec-001", "");
    d.strategy_id = String::new();
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("strategy_id")),
        "blockers must mention strategy_id; got: {:?}",
        out.blockers
    );
}

/// CC-01D / Gate 0: invalid side value → rejected.
#[tokio::test]
async fn decision_invalid_side_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("dec-002", "strat-a");
    d.side = "SHORT".to_string(); // not "buy" or "sell"
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("side")),
        "blockers must mention side; got: {:?}",
        out.blockers
    );
}

/// CC-01D / Gate 0: zero qty → rejected.
#[tokio::test]
async fn decision_zero_qty_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("dec-003", "strat-a");
    d.qty = 0;
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("qty")),
        "blockers must mention qty; got: {:?}",
        out.blockers
    );
}

/// CC-01D / Gate 0: negative qty → rejected.
#[tokio::test]
async fn decision_negative_qty_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("dec-004", "strat-a");
    d.qty = -5;
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
}

/// CC-01D / Gate 0: limit order with no limit_price → rejected.
#[tokio::test]
async fn decision_limit_order_without_price_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut d = make_decision("dec-005", "strat-a");
    d.order_type = "limit".to_string();
    d.limit_price = None;
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("limit_price")),
        "blockers must mention limit_price; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 1: PT-AUTO-02 day-limit (no DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 1: day-limit reached fires before DB access.
///
/// Even with no DB, once the counter is saturated the function must return
/// "day_limit_reached" — not "unavailable".  This proves the gate ordering
/// is correct: intake bound is checked before DB presence.
#[tokio::test]
async fn decision_day_limit_reached_blocks_before_db() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Saturate the counter.  The state has no DB, so if the order of gates
    // were wrong we would see "unavailable" instead of "day_limit_reached".
    st.set_day_signal_count_for_test(100);

    let d = make_decision("dec-limit", "strat-a");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "day_limit_reached",
        "saturated counter must produce day_limit_reached before DB gate"
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("day signal limit")),
        "blocker must mention day signal limit; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 2: DB present (no DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 2: no DB → unavailable.
///
/// After field validation and day-limit pass, the next gate checks for a
/// configured DB.  State without a DB must return "unavailable".
#[tokio::test]
async fn decision_no_db_returns_unavailable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let d = make_decision("dec-nodb", "strat-a");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "unavailable",
        "no DB must produce unavailable; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("DB")),
        "blocker must mention DB; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 3: registry check (DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 3: unregistered strategy → rejected.
///
/// A strategy that has no row in sys_strategy_registry must be refused with
/// disposition "rejected", not "unavailable".  This distinguishes a deliberate
/// unknown identity from a transient system error.
#[tokio::test]
#[ignore]
async fn decision_unregistered_strategy_rejected() {
    let pool = make_db_pool().await;
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sid = unique_id("unregistered");
    let d = make_decision(&unique_id("dec"), &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "unregistered strategy must be rejected, not unavailable; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("not registered")),
        "blocker must mention 'not registered'; got: {:?}",
        out.blockers
    );
}

/// CC-01D / Gate 3: registered but disabled → rejected.
///
/// A known strategy that has `enabled = false` in the registry must be refused.
/// This is an explicit operator decision and must be honoured by the seam.
#[tokio::test]
#[ignore]
async fn decision_disabled_strategy_rejected() {
    let pool = make_db_pool().await;
    let sid = unique_id("disabled");
    seed_registry(&pool, &sid, false).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let d = make_decision(&unique_id("dec"), &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "disabled strategy must be rejected; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("disabled")),
        "blocker must mention 'disabled'; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 4: suppression enforcement (DB required)
// ---------------------------------------------------------------------------

/// CC-02B / Gate 4: active suppression refuses a registered+enabled strategy.
///
/// An active suppression must block the decision before it reaches Gate 5
/// (arm state) or Gate 7 (outbox enqueue).  The disposition must be
/// "suppressed", not "rejected" or "unavailable".
#[tokio::test]
#[ignore]
async fn decision_active_suppression_refuses_entry() {
    let pool = make_db_pool().await;
    let sid = unique_id("sup_active");
    seed_registry(&pool, &sid, true).await;

    // Insert an active suppression for this strategy.
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: Uuid::new_v4(),
            strategy_id: sid.clone(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "CC-02B enforcement test".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("insert suppression failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "suppressed",
        "active suppression must produce 'suppressed' disposition; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("suppressed")),
        "blocker must mention suppressed; got: {:?}",
        out.blockers
    );
    // active_run_id must be None — refusal fires before Gate 6 (active run check).
    assert!(
        out.active_run_id.is_none(),
        "suppression refusal must fire before active-run gate; active_run_id must be None"
    );
}

/// CC-02B / Gate 4: cleared suppression no longer blocks decision entry.
///
/// After a suppression is cleared (state = 'cleared'), the same strategy
/// must be able to pass Gate 4.  The decision proceeds to later gates
/// (arm state, active run) rather than stopping at suppression.
#[tokio::test]
#[ignore]
async fn decision_cleared_suppression_does_not_block() {
    let pool = make_db_pool().await;
    let sid = unique_id("sup_cleared");
    seed_registry(&pool, &sid, true).await;

    let sup_id = Uuid::new_v4();
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: sup_id,
            strategy_id: sid.clone(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "will be cleared".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("insert suppression failed");

    // Clear it — the strategy is no longer suppressed.
    mqk_db::clear_strategy_suppression(&pool, sup_id, Utc::now())
        .await
        .expect("clear suppression failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    // The decision must not be refused with "suppressed".
    // It will be refused at Gate 5 (no arm state) — which proves Gate 4 passed.
    assert_ne!(
        out.disposition, "suppressed",
        "cleared suppression must not block decision entry; disposition was 'suppressed'"
    );
    assert!(
        out.disposition == "rejected" || out.disposition == "unavailable",
        "after cleared suppression, decision must fail at a later gate (arm/run), \
         not suppression gate; got disposition: {:?}",
        out.disposition
    );
}

/// CC-02B / Gate 4: suppression check is keyed to canonical strategy identity.
///
/// A suppression on strategy A must not affect decisions for strategy B.
/// Each strategy's suppression state is independent.
#[tokio::test]
#[ignore]
async fn decision_suppression_does_not_bleed_across_strategies() {
    let pool = make_db_pool().await;
    let sid_a = unique_id("sup_a");
    let sid_b = unique_id("sup_b");
    seed_registry(&pool, &sid_a, true).await;
    seed_registry(&pool, &sid_b, true).await;

    // Suppress only strategy A.
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: Uuid::new_v4(),
            strategy_id: sid_a.clone(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "only A suppressed".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("insert suppression failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Strategy A must be refused at Gate 4.
    let out_a =
        submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid_a)).await;
    assert_eq!(
        out_a.disposition, "suppressed",
        "strategy A must be refused by its suppression"
    );

    // Strategy B must NOT be refused at Gate 4 (proceeds to later gates).
    let out_b =
        submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid_b)).await;
    assert_ne!(
        out_b.disposition, "suppressed",
        "strategy B must not be affected by strategy A's suppression; \
         got disposition: {:?}",
        out_b.disposition
    );
}

/// CC-02B / Gate 4: refusal fires before outbox enqueue.
///
/// Proves suppression enforcement is load-bearing: when a strategy is
/// suppressed no outbox row is created for any decision_id.
#[tokio::test]
#[ignore]
async fn decision_suppression_blocks_before_outbox_enqueue() {
    let pool = make_db_pool().await;
    let sid = unique_id("sup_outbox");
    seed_registry(&pool, &sid, true).await;

    let sup_id = Uuid::new_v4();
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: sup_id,
            strategy_id: sid.clone(),
            trigger_domain: "risk".to_string(),
            trigger_reason: "pre-enqueue block test".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("insert suppression failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let dec_id = unique_id("dec_blocked");
    let out = submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid)).await;

    assert_eq!(out.disposition, "suppressed");

    // No outbox row must exist for this decision_id.
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM oms_outbox WHERE idempotency_key = $1")
            .bind(&dec_id)
            .fetch_one(&pool)
            .await
            .expect("count outbox");
    assert_eq!(
        count, 0,
        "suppression refusal must not create any outbox row; found {count} row(s)"
    );
}

// ---------------------------------------------------------------------------
// Gate 5: arm state (DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 5: registered + enabled but arm state absent → rejected.
///
/// A fresh system with no arm state row in the DB defaults to disarmed
/// (fail-closed).  The decision must be refused before the active-run gate.
#[tokio::test]
#[ignore]
async fn decision_passes_registry_but_no_arm_state_rejected() {
    let pool = make_db_pool().await;

    // Clear any existing arm state so the load returns None.
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("disarmed");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let d = make_decision(&unique_id("dec"), &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "missing arm state must produce rejected (fail-closed); got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("arm")),
        "blocker must mention arm state; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 7: full enqueue path (DB required)
// ---------------------------------------------------------------------------

/// CC-01D / Gate 7: all gates pass → accepted; outbox row has correct signal_source.
///
/// This is the primary proof that the canonical execution path is reached.
/// After acceptance the outbox row's order_json must carry
/// `"signal_source": "internal_strategy_decision"` to distinguish it from
/// external signal ingestion in the audit trail.
#[tokio::test]
#[ignore]
async fn decision_full_enqueue_path_accepted() {
    let pool = make_db_pool().await;

    // Clear stale state.
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("fullpath");
    seed_registry(&pool, &sid, true).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist arm state");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let d = make_decision(&dec_id, &sid);
    let out = submit_internal_strategy_decision(&st, d).await;

    // Assert acceptance.
    assert!(
        out.accepted,
        "all gates passed; decision must be accepted; disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.disposition, "accepted");
    assert_eq!(
        out.active_run_id,
        Some(run_id),
        "active_run_id must echo the seeded run"
    );
    assert!(out.blockers.is_empty(), "accepted must have no blockers");

    // Verify the outbox row exists with the correct signal_source.
    let row = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT order_json FROM oms_outbox WHERE idempotency_key = $1",
    )
    .bind(&dec_id)
    .fetch_optional(&pool)
    .await
    .expect("query outbox")
    .expect("outbox row must exist after acceptance");

    let order_json = row.0;
    assert_eq!(
        order_json["signal_source"], "internal_strategy_decision",
        "outbox row must carry signal_source='internal_strategy_decision'; got: {}",
        order_json
    );
    assert_eq!(order_json["strategy_id"], sid.as_str());
    assert_eq!(order_json["symbol"], "AAPL");

    cleanup_run(&pool, run_id).await;
}

/// CC-01D / Gate 7: duplicate decision_id → disposition "duplicate", no second row.
///
/// Idempotency is critical: resubmitting the same decision_id must be safe.
/// The second call must return "duplicate" and must NOT increment the day
/// signal counter (duplicates do not consume quota).
#[tokio::test]
#[ignore]
async fn decision_duplicate_decision_id_returns_duplicate() {
    let pool = make_db_pool().await;

    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("dup");
    seed_registry(&pool, &sid, true).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist arm state");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");

    // First submission — must be accepted.
    let first = submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid)).await;
    assert!(first.accepted, "first submission must be accepted");
    assert_eq!(first.disposition, "accepted");

    let count_after_first = st.day_signal_count();

    // Second submission with the same decision_id — must be duplicate.
    let second = submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid)).await;
    assert!(
        !second.accepted,
        "duplicate submission must not be accepted"
    );
    assert_eq!(
        second.disposition, "duplicate",
        "second submission with same decision_id must return 'duplicate'"
    );

    // The day signal counter must not have advanced for the duplicate.
    assert_eq!(
        st.day_signal_count(),
        count_after_first,
        "duplicate must not increment the day signal counter"
    );

    // Exactly one row in the outbox.
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM oms_outbox WHERE idempotency_key = $1")
            .bind(&dec_id)
            .fetch_one(&pool)
            .await
            .expect("count outbox");
    assert_eq!(
        count, 1,
        "duplicate submission must not create a second outbox row"
    );

    cleanup_run(&pool, run_id).await;
}
