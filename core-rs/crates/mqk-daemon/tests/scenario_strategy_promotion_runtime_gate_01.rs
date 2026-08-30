//! STRATEGY-PROMOTION-REGISTRY-01D: runtime promotion-gate enforcement
//! proof tests.
//!
//! Proves the shared `promotion_gate::evaluate_paper_promotion_gate`
//! evaluator, wired identically into both strategy-originated outbox write
//! paths:
//! - internal decision seam (`decision::submit_internal_strategy_decision`,
//!   Gate 3b — after registry, before suppression)
//! - external signal route (`POST /api/v1/strategy/signal`,
//!   Gate 2b — after DB-present, before arm-state)
//!
//! `registered + enabled` (`sys_strategy_registry.enabled`) is never
//! sufficient for paper trading in either path — every test in the "denied"
//! matrix below registers and enables the strategy first, then proves the
//! promotion state (or its absence) is still the deciding factor.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked
//! `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_strategy_promotion_runtime_gate_01 \
//!     -- --include-ignored --test-threads=1

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
};
use tower::ServiceExt;
use uuid::Uuid;

const SYMBOL: &str = "AAPL";
const TIMEFRAME_SECS: i64 = 86400;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_strategy_promotion_runtime_gate_01 \
             -- --include-ignored --test-threads=1"
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
            display_name: format!("RTG-01 Test Strategy {strategy_id}"),
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

/// C2: a fixed, syntactically-valid (64 lowercase hex) semantic fingerprint
/// used consistently for every row `insert_transition`/`seed_promotion_state`
/// create in this file, and by `make_decision`'s decisions and this file's
/// direct `evaluate_paper_promotion_gate` calls — this file proves gate
/// sequencing/mode/identity-matching, not config-identity binding itself
/// (see `scenario_promotion_config_identity_01.rs` for that), so the exact
/// value is irrelevant as long as every caller agrees on it.
fn rtg01_test_fingerprint() -> String {
    "9".repeat(64)
}

/// Insert one promotion transition row directly (no route/evidence
/// validation — this file proves the *runtime gate*, not the operator
/// transition surface, which is covered by
/// `scenario_strategy_promotion_routes_01.rs`).
#[allow(clippy::too_many_arguments)]
async fn insert_transition(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    previous_state: Option<&str>,
    new_state: &str,
    effective_at: chrono::DateTime<Utc>,
    expires_at: Option<chrono::DateTime<Utc>>,
    seed_suffix: &str,
) {
    let transition_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("rtg01-seed:{strategy_id}:{symbol}:{timeframe_secs}:{seed_suffix}").as_bytes(),
    );
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(rtg01_test_fingerprint()),
            config_identity_status: "verified_v1".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: effective_at,
            expires_at_utc: expires_at,
            initiated_by: "rtg01-test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: effective_at,
        },
    )
    .await
    .expect("insert_transition failed");
}

/// Walk the full legal transition graph to `active_paper` (or stop earlier
/// for shadow_approved/paper_approved) for `(strategy_id, symbol,
/// timeframe_secs)`.
async fn seed_promotion_state(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_state: &str,
) {
    let now = Utc::now();
    insert_transition(
        pool,
        strategy_id,
        symbol,
        timeframe_secs,
        None,
        "shadow_approved",
        now,
        None,
        "1",
    )
    .await;
    if target_state == "shadow_approved" {
        return;
    }
    insert_transition(
        pool,
        strategy_id,
        symbol,
        timeframe_secs,
        Some("shadow_approved"),
        "paper_approved",
        now + Duration::milliseconds(1),
        None,
        "2",
    )
    .await;
    if target_state == "paper_approved" {
        return;
    }
    if target_state == "rejected" {
        insert_transition(
            pool,
            strategy_id,
            symbol,
            timeframe_secs,
            Some("paper_approved"),
            "retired", // paper_approved cannot go directly to rejected; use retired for a terminal non-active state distinct from active_paper's demote path
            now + Duration::milliseconds(2),
            None,
            "3",
        )
        .await;
        return;
    }
    if target_state == "expired" {
        // The active_paper row itself is the latest transition for this
        // identity, but its expires_at_utc is already in the past.
        let expired_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("rtg01-seed:{strategy_id}:{symbol}:{timeframe_secs}:expired").as_bytes(),
        );
        mqk_db::insert_strategy_promotion_transition(
            pool,
            &mqk_db::InsertStrategyPromotionTransitionArgs {
                transition_id: expired_id,
                strategy_id: strategy_id.to_string(),
                symbol: symbol.to_string(),
                timeframe_secs,
                config_fingerprint: Some(rtg01_test_fingerprint()),
                config_identity_status: "verified_v1".to_string(),
                previous_state: Some("paper_approved".to_string()),
                new_state: "active_paper".to_string(),
                parent_transition_id: None,
                evidence_transition_id: None,
                evidence_review_id: None,
                evidence_scanner_scan_id: None,
                evidence_git_hash: None,
                evidence_artifact_path: None,
                evidence_fingerprint: None,
                evidence_fingerprint_v2: None,
                effective_at_utc: now + Duration::milliseconds(2),
                expires_at_utc: Some(now - Duration::hours(1)),
                initiated_by: "rtg01-test-seed".to_string(),
                reason: "expired fixture".to_string(),
                created_at_utc: now + Duration::milliseconds(2),
            },
        )
        .await
        .expect("insert expired active_paper");
        return;
    }
    insert_transition(
        pool,
        strategy_id,
        symbol,
        timeframe_secs,
        Some("paper_approved"),
        "active_paper",
        now + Duration::milliseconds(2),
        None,
        "3",
    )
    .await;
    if target_state == "active_paper" {
        return;
    }
    if target_state == "demoted" || target_state == "retired" {
        insert_transition(
            pool,
            strategy_id,
            symbol,
            timeframe_secs,
            Some("active_paper"),
            target_state,
            now + Duration::milliseconds(3),
            None,
            "4",
        )
        .await;
    }
}

/// C2: seed a durable `active_paper` chain bound to a CALLER-SUPPLIED
/// fingerprint rather than `rtg01_test_fingerprint()` — used only by the two
/// external-signal positive tests below, which need `strategy_id` to be a
/// REAL registered engine (so the external path's own server-side
/// `resolve_server_semantic_fingerprint` re-derivation genuinely matches),
/// unlike every other test in this file which controls both sides of the
/// comparison directly and can use the fixed test fingerprint.
async fn seed_active_paper_with_fingerprint(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    fingerprint: &str,
) {
    let now = Utc::now();
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(fingerprint.to_string()),
            config_identity_status: "verified_v1".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: effective_at,
            expires_at_utc: None,
            initiated_by: "rtg01-test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: effective_at,
        }
    };
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("rtg01-seed-realfp:{strategy_id}:{symbol}:{timeframe_secs}:{suffix}").as_bytes(),
        )
    };
    mqk_db::insert_strategy_promotion_transition(pool, &step(seed("1"), None, "shadow_approved", now))
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
            config_json: serde_json::json!({"source": "scenario_strategy_promotion_runtime_gate_01"}),
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

fn make_decision(
    decision_id: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs,
        strategy_semantic_fingerprint: rtg01_test_fingerprint(),
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

async fn outbox_row_count(pool: &sqlx::PgPool, decision_id: &str) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM oms_outbox WHERE idempotency_key = $1")
        .bind(decision_id)
        .fetch_one(pool)
        .await
        .expect("count outbox rows");
    row.0
}

// ---------------------------------------------------------------------------
// Internal path — denial matrix
// ---------------------------------------------------------------------------

async fn assert_internal_denied(
    promotion_state: Option<&str>,
    expired: bool,
    expected_disposition: &str,
) {
    let pool = make_db_pool().await;
    let sid = unique_id(&format!("rtg01i_{}", promotion_state.unwrap_or("none")));
    seed_registry(&pool, &sid, true).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    if let Some(state_name) = promotion_state {
        let target = if expired { "expired" } else { state_name };
        seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, target).await;
    }

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let d = make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        !out.accepted,
        "must not be accepted; disposition={:?}",
        out.disposition
    );
    assert_eq!(
        out.disposition, expected_disposition,
        "unexpected disposition; blockers={:?}",
        out.blockers
    );
    assert_eq!(
        outbox_row_count(&pool, &dec_id).await,
        0,
        "denied decision must never create an outbox row"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_no_promotion_row_denied() {
    assert_internal_denied(None, false, "promotion_missing").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_shadow_approved_denied() {
    assert_internal_denied(Some("shadow_approved"), false, "promotion_shadow_only").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_paper_approved_not_active_denied() {
    assert_internal_denied(Some("paper_approved"), false, "promotion_not_active").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_demoted_denied() {
    assert_internal_denied(Some("demoted"), false, "promotion_demoted").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_retired_denied() {
    assert_internal_denied(Some("retired"), false, "promotion_retired").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_rejected_denied() {
    assert_internal_denied(Some("rejected"), false, "promotion_retired").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_expired_active_paper_denied() {
    assert_internal_denied(Some("active_paper"), true, "promotion_expired").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_symbol_mismatch_denied() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_symmismatch");
    seed_registry(&pool, &sid, true).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    // Approve MSFT, decide on AAPL.
    seed_promotion_state(&pool, &sid, "MSFT", TIMEFRAME_SECS, "active_paper").await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(!out.accepted);
    assert_eq!(out.disposition, "promotion_missing");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_timeframe_mismatch_denied() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_tfmismatch");
    seed_registry(&pool, &sid, true).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    // Approve at 1H (3600s), decide at 1D (86400s).
    seed_promotion_state(&pool, &sid, SYMBOL, 3600, "active_paper").await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(!out.accepted);
    assert_eq!(out.disposition, "promotion_missing");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_no_db_unavailable_no_outbox() {
    let sid = unique_id("rtg01i_nodb");
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(!out.accepted);
    assert_eq!(out.disposition, "unavailable");
}

/// Suppression still blocks independently even when promotion is active
/// (Gate 3b passes, Gate 4 suppression still fires).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_active_promotion_still_blocked_by_suppression() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_suppressed");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: Uuid::new_v4(),
            strategy_id: sid.clone(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "RTG-01 independent suppression test".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("insert suppression");
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "suppressed",
        "active promotion must pass Gate 3b so Gate 4 suppression can independently fire"
    );
}

/// Disabled registry still blocks independently even when promotion is
/// active (Gate 3 fires before Gate 3b is ever reached).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_active_promotion_still_blocked_by_disabled_registry() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_disabled");
    seed_registry(&pool, &sid, false).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "disabled registry must fire at Gate 3, before Gate 3b is ever evaluated"
    );
}

// ---------------------------------------------------------------------------
// Internal path — acceptance
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_active_paper_exact_identity_accepted_one_outbox_row() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_accept");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let before_account = st.day_signal_count();
    let before_symbol = st.symbol_day_order_count(SYMBOL).await;

    let dec_id = unique_id("dec");
    let d = make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS);
    let out = submit_internal_strategy_decision(&st, d.clone()).await;

    assert!(
        out.accepted,
        "active_paper exact identity must pass the promotion gate; disposition={:?} blockers={:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.disposition, "accepted");
    assert_eq!(out.active_run_id, Some(run_id));
    assert_eq!(
        outbox_row_count(&pool, &dec_id).await,
        1,
        "exactly one synthetic outbox row must exist"
    );
    assert_eq!(
        st.day_signal_count(),
        before_account + 1,
        "account-wide counter increments only after a new outbox insert"
    );
    assert_eq!(
        st.symbol_day_order_count(SYMBOL).await,
        before_symbol + 1,
        "per-symbol counter increments only after a new outbox insert"
    );

    // Duplicate remains idempotent: no second row, no counter movement.
    let before_account_2 = st.day_signal_count();
    let out2 = submit_internal_strategy_decision(&st, d).await;
    assert!(!out2.accepted);
    assert_eq!(out2.disposition, "duplicate");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 1);
    assert_eq!(
        st.day_signal_count(),
        before_account_2,
        "duplicate must not advance the account-wide counter"
    );
}

// ---------------------------------------------------------------------------
// External path
// ---------------------------------------------------------------------------

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
    (status, json)
}

fn signal_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build external-signal-ready state: Paper+Alpaca deployment, DB wired,
/// WS continuity Live, session clock set to a real NYSE regular-session
/// timestamp (2024-01-08 Mon 10:00 ET) — mirrors the setup convention
/// already used by `scenario_paper_alpaca_proof_bundle_brk00r06.rs`'s
/// `reaches_db_gate`-style tests.
async fn make_external_signal_state(pool: sqlx::PgPool) -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:rtg01:new:2024-01-08T14:00:00Z".to_string(),
        last_event_at: "2024-01-08T14:00:00Z".to_string(),
    })
    .await;
    st.set_session_clock_ts_for_test(1_704_726_000).await;
    st
}

fn external_signal_body(
    signal_id: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: Option<i64>,
) -> serde_json::Value {
    serde_json::json!({
        "signal_id": signal_id,
        "strategy_id": strategy_id,
        "symbol": symbol,
        "side": "buy",
        "qty": 10,
        "timeframe_secs": timeframe_secs,
    })
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_no_promotion_refused_before_outbox() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01e_none");
    seed_registry(&pool, &sid, true).await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            SYMBOL,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["disposition"], "promotion_missing");
    assert_eq!(json["intent_placed"], false);
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_missing_timeframe_fails_closed() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01e_notf");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(&signal_id, &sid, SYMBOL, None)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["disposition"], "promotion_timeframe_unknown");
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_mismatched_symbol_and_timeframe_refused() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01e_mismatch");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, "MSFT", TIMEFRAME_SECS, "active_paper").await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            SYMBOL,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["disposition"], "promotion_missing");
}

async fn assert_external_denied(
    target_state: &str,
    expired: bool,
    expected_disposition: &str,
    expected_status: StatusCode,
) {
    let pool = make_db_pool().await;
    let sid = unique_id(&format!("rtg01e_{target_state}"));
    seed_registry(&pool, &sid, true).await;
    let effective_target = if expired { "expired" } else { target_state };
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, effective_target).await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            SYMBOL,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    assert_eq!(status, expected_status);
    assert_eq!(json["disposition"], expected_disposition);
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_demoted_refused() {
    assert_external_denied("demoted", false, "promotion_demoted", StatusCode::FORBIDDEN).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_retired_refused() {
    assert_external_denied("retired", false, "promotion_retired", StatusCode::FORBIDDEN).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_rejected_refused() {
    assert_external_denied(
        "rejected",
        false,
        "promotion_retired",
        StatusCode::FORBIDDEN,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_expired_refused() {
    assert_external_denied(
        "active_paper",
        true,
        "promotion_expired",
        StatusCode::FORBIDDEN,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_no_db_query_failure_is_unavailable() {
    let sid = unique_id("rtg01e_nodb");
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:rtg01:new:2024-01-08T14:00:00Z".to_string(),
        last_event_at: "2024-01-08T14:00:00Z".to_string(),
    })
    .await;
    st.set_session_clock_ts_for_test(1_704_726_000).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            SYMBOL,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    // No DB configured at all → Gate 2 (db_present) fires before Gate 2b is
    // ever reached; disposition is the pre-existing Gate-2 "unavailable".
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["disposition"], "unavailable");
}

/// EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01: an exact, real,
/// currently-matching `active_paper` identity is still refused at Gate 2b
/// on the external signal path — this channel has no trusted way to
/// authenticate what configuration/logic actually produced the signal's
/// own side/qty, so a server-side reconstruction (which would only ever
/// prove THIS daemon's own current config) is never attempted as a
/// substitute. Contrast with the internal path's
/// `internal_active_paper_exact_identity_accepted_one_outbox_row`, which
/// DOES have a real running host to query and remains accepted unchanged.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_active_paper_exact_identity_is_refused_at_promotion_gate() {
    let pool = make_db_pool().await;
    // Reset the global durable arm-state singleton: other tests in this
    // shared test DB may have left it ARMED, and this test's proof
    // (reaching Gate 3's "not armed" refusal) depends on a known-disarmed
    // starting state, matching the reset convention used elsewhere in this
    // suite (e.g. scenario_internal_strategy_decision.rs).
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    // C2: the external path re-derives its semantic fingerprint server-side
    // through the real authoritative strategy registry — a synthetic
    // strategy_id can never resolve one, so this identity must be a real
    // registered engine for the fingerprint comparison to genuinely match.
    let sid = "swing_momentum".to_string();
    let symbol = unique_id("RTG01E").to_uppercase();
    let fingerprint = mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint(
        &sid,
        &symbol,
        TIMEFRAME_SECS,
    )
    .expect("swing_momentum must resolve");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_with_fingerprint(&pool, &sid, &symbol, TIMEFRAME_SECS, &fingerprint).await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            &symbol,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    // EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01: independent review
    // found this test previously asserted the DEFECT -- that an exact
    // real-fingerprint match let the external path pass Gate 2b and reach
    // Gate 3 (arm state). The external signal route now never attempts a
    // server-side reconstruction as provenance for a config-bound identity,
    // so even this exact real match is refused AT Gate 2b, before arm state
    // is ever consulted -- never reaching (and never able to reach)
    // "rejected" for an unrelated, later gate.
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "must be refused at Gate 2b regardless of arm state: {json}"
    );
    assert_eq!(
        json["disposition"], "promotion_external_semantic_provenance_unavailable"
    );
}

/// EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01: independent review
/// found this test previously proved the DEFECT end-to-end -- every OTHER
/// gate satisfied (armed, active running run) let a reconstructed-fingerprint
/// match reach the outbox. Now proves the opposite, and the load-bearing
/// "no live-authority gap" property the mission requires: with every other
/// gate satisfied, the external path is STILL refused at Gate 2b and zero
/// outbox rows are ever created -- config-bound Paper authority can never
/// reach the outbox on this path, regardless of how "ready" every other
/// gate is.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_active_paper_full_happy_path_is_refused_at_promotion_gate() {
    let pool = make_db_pool().await;
    // Real registered engine, real matching fingerprint -- see
    // `external_active_paper_exact_identity_is_refused_at_promotion_gate`'s
    // doc comment for why a synthetic strategy_id cannot be used here.
    let sid = "swing_momentum".to_string();
    let symbol = unique_id("RTG01E2E").to_uppercase();
    let fingerprint = mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint(
        &sid,
        &symbol,
        TIMEFRAME_SECS,
    )
    .expect("swing_momentum must resolve");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_with_fingerprint(&pool, &sid, &symbol, TIMEFRAME_SECS, &fingerprint).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    let st = make_external_signal_state(pool.clone()).await;
    let _run_id = seed_active_run(&st).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            &symbol,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "every other gate satisfied, but the external path must still be refused for \
         config-bound Paper authority: {json}"
    );
    assert_eq!(
        json["disposition"], "promotion_external_semantic_provenance_unavailable"
    );
    assert_eq!(json["intent_placed"], false);
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

/// A promotion-denied signal never deposits a bar input for the execution
/// loop to dispatch — `tick_strategy_dispatch` must return `None`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_promotion_denial_never_deposits_bar_input() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01e_nobarinput");
    seed_registry(&pool, &sid, true).await;
    // No promotion seeded — must be denied at Gate 2b.
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, _json) = call(
        routes::build_router(Arc::clone(&st)),
        signal_req(external_signal_body(
            &signal_id,
            &sid,
            SYMBOL,
            Some(TIMEFRAME_SECS),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let dispatched = st.tick_strategy_dispatch().await;
    assert!(
        dispatched.is_none(),
        "promotion denial must never deposit a bar input for the execution loop"
    );
}

// ---------------------------------------------------------------------------
// Shadow / live boundaries
// ---------------------------------------------------------------------------

// NOTE: `shadow_approved` never changes dry-run diagnostic `submitted` is
// proven structurally, not by a new test here: `state::dry_run_strategy`
// is a private module not exposed to integration tests, its evaluator
// (`evaluate_dry_run_strategy`) takes only plain data (no `AppState`/DB
// handle — see the module's own doc comment), so it cannot read promotion
// state even if it wanted to, and this patch does not modify that file at
// all. Its own internal unit tests (`state::dry_run_strategy::tests::*`,
// 8 tests) already prove `submitted` is always `false` and continue to
// pass unchanged under this patch.

/// The shared promotion gate's outcome type has no live-authorization
/// field or variant that any caller could set true: `PromotionGateOutcome`
/// only carries `paper_tradable`. This is a structural (compile-time)
/// guarantee re-asserted here as a readable regression check: the reason
/// code produced for an unpromoted identity is a paper-scoped code, never
/// anything implying live authorization.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn paper_promotion_gate_never_authorizes_live() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01_live_boundary");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;

    let fp = rtg01_test_fingerprint();
    let outcome = mqk_daemon::promotion_gate::evaluate_paper_promotion_gate(
        &pool,
        mqk_daemon::promotion_gate::PromotionRunMode::Paper,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        mqk_daemon::promotion_gate::SemanticProvenance::Fingerprint(Some(fp.as_str())),
    )
    .await;
    assert!(
        outcome.paper_tradable,
        "sanity: active_paper must be paper_tradable"
    );
    assert_ne!(
        outcome.reason_code.code(),
        "promotion_live_not_authorized",
        "an active_paper outcome must never surface a live-authorization reason code"
    );
    // No field on PromotionGateOutcome can express live authorization — the
    // outcome only ever answers the paper-tradability question.
}

/// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase E): the exact same
/// `active_paper` identity is denied when evaluated from a LIVE or
/// unknown runtime mode — the gate now requires an explicit mode input
/// and fails closed for anything but `Paper`, rather than relying solely
/// on "no LIVE call site exists today."
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_paper_denied_in_live_mode() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01_live_denied");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;

    let outcome = mqk_daemon::promotion_gate::evaluate_paper_promotion_gate(
        &pool,
        mqk_daemon::promotion_gate::PromotionRunMode::Live,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        mqk_daemon::promotion_gate::SemanticProvenance::Fingerprint(None),
    )
    .await;
    assert!(
        !outcome.paper_tradable,
        "active_paper must never be tradable when evaluated from a LIVE runtime mode"
    );
    assert_eq!(outcome.reason_code.code(), "promotion_live_not_authorized");
}

/// Same identity, evaluated from an `Unknown` runtime mode (e.g.
/// `Backtest`, or any future mode this gate has not been explicitly
/// taught about) — must also fail closed, not default to paper-tradable.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_paper_denied_in_unknown_mode() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01_unknown_denied");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;

    let outcome = mqk_daemon::promotion_gate::evaluate_paper_promotion_gate(
        &pool,
        mqk_daemon::promotion_gate::PromotionRunMode::Unknown,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        mqk_daemon::promotion_gate::SemanticProvenance::Fingerprint(None),
    )
    .await;
    assert!(
        !outcome.paper_tradable,
        "active_paper must never be tradable when evaluated from an unknown runtime mode"
    );
    assert_eq!(outcome.reason_code.code(), "promotion_live_not_authorized");
}

/// End-to-end proof on the internal decision path: the exact same
/// `active_paper`+armed+active-run fixture that is accepted in PAPER mode
/// (see `internal_active_paper_exact_identity_accepted_one_outbox_row`)
/// must be refused with zero outbox rows when the daemon's own
/// deployment mode is LIVE-shadow/LIVE-capital.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_active_paper_denied_when_daemon_mode_is_live() {
    let pool = make_db_pool().await;
    let sid = unique_id("rtg01i_live_denied");
    seed_registry(&pool, &sid, true).await;
    seed_promotion_state(&pool, &sid, SYMBOL, TIMEFRAME_SECS, "active_paper").await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let _run_id = seed_active_run(&st).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS),
    )
    .await;
    assert!(
        !out.accepted,
        "active_paper must not authorize an internal decision when daemon mode is LIVE-shadow"
    );
    assert_eq!(out.disposition, "promotion_live_not_authorized");
    assert_eq!(
        outbox_row_count(&pool, &dec_id).await,
        0,
        "a mode-denied decision must never create an outbox row"
    );
}
