//! LO-02C: Broker WS continuity + restart reconciliation coherence proof.
//!
//! Proves that persisted WS continuity state and persisted reconcile truth
//! interact coherently and fail-closed across restart boundaries material to
//! shadow/live progression.
//!
//! # Gap this patch closes
//!
//! BRK-09R (reconcile gate) proved: in-process injected reconcile state blocks
//! start, and WS continuity gate fires before reconcile gate.
//!
//! LO-02B (shadow recovery) proved: WS cursor seeding is correct for
//! LiveShadow, Live cursor demoted, GapDetected preserved.
//!
//! Neither patch proved the JOINT restart boundary:
//! - that a dirty reconcile from a prior session automatically carries across
//!   restart (reconcile reads DB at gate-check time — no seeding call needed)
//! - that when BOTH WS cursor AND reconcile state are degraded in the DB,
//!   the gate ordering is coherent (WS fires before reconcile)
//! - that cursor demotion (Live→ColdStart) still leaves WS gate as the
//!   first blocker before a dirty reconcile gate
//!
//! # Key architectural property proven (RC-01)
//!
//! Reconcile truth reads DB at gate-check time (`current_reconcile_snapshot`).
//! There is no separate "seeding" step.  A fresh daemon process inheriting a
//! dirty reconcile state in the DB is blocked automatically — with no
//! explicit `publish_reconcile_snapshot` call in the new process.
//!
//! This is the critical asymmetry with WS continuity (which requires explicit
//! `seed_ws_continuity_from_db()` before the DB state affects in-memory state):
//!
//! - Reconcile: dirty state is unavoidable on restart — operator MUST clear it.
//! - WS continuity: not seeded → stays ColdStartUnproven (still fails gate,
//!   but the in-memory state does not reflect the prior-session DB cursor).
//!
//! # Test matrix
//!
//! | ID    | DB state at restart           | After WS seed  | Gate that fires (step 1)   |
//! |-------|-------------------------------|----------------|----------------------------|
//! | RC-01 | dirty reconcile only          | ColdStart      | alpaca_ws_continuity (1)   |
//! | RC-02 | GapDetected + dirty reconcile | GapDetected    | alpaca_ws_continuity (1)   |
//! | RC-03 | Live cursor + dirty reconcile | ColdStart (2)  | alpaca_ws_continuity (1)   |
//! | RC-04 | GapDetected + ok reconcile    | GapDetected    | alpaca_ws_continuity only  |
//!
//! (1) After fixing WS to Live, reconcile_truth fires.
//! (2) Live cursor demoted to ColdStartUnproven by `seed_ws_continuity_from_db`.
//!
//! RC-01 specifically: proves DB-driven reconcile without in-process injection.
//!
//! # DB-backed tests
//!
//! All tests require MQK_DATABASE_URL and skip gracefully without it.
//! `sys_reconcile_status_state` is a singleton (sentinel_id = 1) — tests
//! must run with --test-threads 1 to avoid parallel interference:
//!
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_continuity_restart_coherence_lo02c \
//!     -- --test-threads 1

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_daemon::routes;
use mqk_daemon::state::{AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("LO-02C: failed to connect to MQK_DATABASE_URL"),
    )
}

/// Connect, migrate, and clear the singleton reconcile state.
/// Called at the start of each test to ensure a clean baseline.
async fn setup_pool() -> Option<sqlx::PgPool> {
    let Some(pool) = db_pool_or_skip().await else {
        return None;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("LO-02C: migration failed");
    // Clear singleton reconcile state so prior test runs do not interfere.
    sqlx::query("DELETE FROM sys_reconcile_status_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("LO-02C: clear reconcile state failed");
    Some(pool)
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn arm_via_http(st: &Arc<AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm_via_http: must succeed");
}

async fn start_req(st: &Arc<AppState>) -> (StatusCode, serde_json::Value) {
    call(
        routes::build_router(Arc::clone(st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
}

async fn inject_live_ws(st: &Arc<AppState>) {
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:order-lo02c:new:2026-01-15T10:00:00Z".to_string(),
        last_event_at: "2026-01-15T10:00:00Z".to_string(),
    })
    .await;
}

async fn seed_dirty_reconcile_to_db(pool: &sqlx::PgPool, note: &str) {
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(Utc::now()),
            snapshot_watermark_ms: Some(1_000_000),
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some(note),
            updated_at_utc: Utc::now(),
        },
    )
    .await
    .expect("seed_dirty_reconcile_to_db: persist failed");
}

async fn seed_ok_reconcile_to_db(pool: &sqlx::PgPool) {
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(Utc::now()),
            snapshot_watermark_ms: Some(2_000_000),
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: Utc::now(),
        },
    )
    .await
    .expect("seed_ok_reconcile_to_db: persist failed");
}

// ---------------------------------------------------------------------------
// RC-01: Persisted dirty reconcile auto-blocks restart (no in-memory publish)
// ---------------------------------------------------------------------------

/// LO-02C / RC-01: Dirty reconcile from prior session blocks restart automatically.
///
/// `current_reconcile_snapshot()` reads from DB at gate-check time — there is
/// no separate "seeding" step.  A fresh daemon process that inherits a dirty
/// reconcile state in the DB is blocked at the reconcile gate without any
/// explicit `publish_reconcile_snapshot` call in the new process.
///
/// This is the critical architectural property: unlike WS continuity (which
/// stays ColdStartUnproven until `seed_ws_continuity_from_db()` is called),
/// reconcile truth cannot be bypassed by failing to seed it — it reads DB
/// directly at gate time.
///
/// Proof path: seed dirty reconcile to DB (no AppState publish call) →
/// fresh AppState (no publish_reconcile_snapshot) → arm → inject Live WS
/// (so WS gate passes) → start → reconcile_truth gate fires from DB.
#[tokio::test]
async fn lo02c_rc01_dirty_reconcile_in_db_auto_blocks_restart_without_in_memory_publish() {
    let Some(pool) = setup_pool().await else {
        eprintln!("RC-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Simulate prior session: write dirty reconcile to DB directly.
    // This does NOT go through AppState.publish_reconcile_snapshot().
    seed_dirty_reconcile_to_db(
        &pool,
        "lo02c-rc01: prior session ended with mismatched positions",
    )
    .await;

    // Simulate restart: fresh AppState.  NO publish_reconcile_snapshot is called.
    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    // Arm integrity gate.
    arm_via_http(&st).await;

    // Establish Live WS so Gate 3 (WS continuity) passes — isolating Gate 4.
    inject_live_ws(&st).await;

    // Start: Gate 4 (reconcile_truth) must fire — reads dirty state from DB
    // without any in-process state injection.
    let (status, json) = start_req(&st).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "RC-01: dirty reconcile in DB must block start (403) with no in-memory publish; \
         got: {status}"
    );
    assert_eq!(
        json["gate"], "reconcile_truth",
        "RC-01: gate must be reconcile_truth (DB-driven, no in-process injection); got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.reconcile_dirty",
        "RC-01: fault_class must be reconcile_dirty; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// RC-02: GapDetected cursor + dirty reconcile in DB → WS gate fires first
// ---------------------------------------------------------------------------

/// LO-02C / RC-02: Both GapDetected WS cursor AND dirty reconcile in DB.
///
/// After seeding WS continuity from a GapDetected cursor, the WS gate (Gate 3)
/// fires first — before the reconcile gate (Gate 4).  After injecting Live WS,
/// the reconcile gate fires.
///
/// This proves gate ordering coherence at the DB restart boundary: with both
/// states degraded in the DB, the operator is directed to fix WS first, then
/// reconcile.  Neither degraded state silently hides the other.
#[tokio::test]
async fn lo02c_rc02_gap_cursor_and_dirty_reconcile_ws_gate_fires_first_then_reconcile() {
    let Some(pool) = setup_pool().await else {
        eprintln!("RC-02: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "lo02c-rc02-test";

    // Seed both degraded states to DB.
    let gap_cursor = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-lo02c-rc02:filled:2026-01-15T09:00:00Z".to_string()),
        Some("2026-01-15T09:00:00Z".to_string()),
        "lo02c-rc02: prior WS reconnect without confirmed replay",
    );
    mqk_db::advance_broker_cursor(
        &pool,
        adapter_id,
        &serde_json::to_string(&gap_cursor).expect("RC-02: serialize"),
        Utc::now(),
    )
    .await
    .expect("RC-02: advance_broker_cursor failed");

    seed_dirty_reconcile_to_db(&pool, "lo02c-rc02: position mismatch concurrent with WS gap").await;

    // Fresh restart.
    let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    st.seed_ws_continuity_from_db().await;

    // WS must be GapDetected after seeding.
    assert!(
        matches!(
            st.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::GapDetected { .. }
        ),
        "RC-02: cursor seeding must produce GapDetected"
    );

    arm_via_http(&st).await;

    // Step 1: WS gate fires first.
    let (status1, json1) = start_req(&st).await;

    assert_eq!(
        status1,
        StatusCode::FORBIDDEN,
        "RC-02: both degraded — WS gate must fire first (403); got: {status1}"
    );
    assert_eq!(
        json1["gate"], "alpaca_ws_continuity",
        "RC-02: gate must be alpaca_ws_continuity (not reconcile_truth); got: {json1}"
    );

    // Step 2: fix WS → reconcile gate fires.
    inject_live_ws(&st).await;

    let (status2, json2) = start_req(&st).await;

    assert_eq!(
        status2,
        StatusCode::FORBIDDEN,
        "RC-02: after fixing WS, dirty reconcile must block (403); got: {status2}"
    );
    assert_eq!(
        json2["gate"], "reconcile_truth",
        "RC-02: after fixing WS, gate must be reconcile_truth; got: {json2}"
    );
}

// ---------------------------------------------------------------------------
// RC-03: Live cursor (demoted to ColdStart) + dirty reconcile → WS gate first
// ---------------------------------------------------------------------------

/// LO-02C / RC-03: Live cursor demoted to ColdStartUnproven + dirty reconcile.
///
/// After seeding from a Live cursor (demoted to ColdStartUnproven), the WS gate
/// still fires first.  Cursor demotion does not bypass the WS gate or allow
/// the reconcile gate to fire out of order.
///
/// After injecting Live WS, the reconcile gate fires from DB — the dirty
/// reconcile is still load-bearing despite the cursor demotion.
///
/// This proves: neither cursor demotion nor the asymmetric seeding behavior
/// create a path where reconcile truth is checked before WS continuity.
#[tokio::test]
async fn lo02c_rc03_live_cursor_demoted_with_dirty_reconcile_ws_gate_fires_first() {
    let Some(pool) = setup_pool().await else {
        eprintln!("RC-03: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "lo02c-rc03-test";

    // Seed Live cursor (will be demoted on seed) + dirty reconcile.
    let live_cursor = AlpacaFetchCursor::live(
        None,
        "alpaca:order-lo02c-rc03:filled:2026-01-15T08:00:00Z",
        "2026-01-15T08:00:00Z",
    );
    mqk_db::advance_broker_cursor(
        &pool,
        adapter_id,
        &serde_json::to_string(&live_cursor).expect("RC-03: serialize"),
        Utc::now(),
    )
    .await
    .expect("RC-03: advance_broker_cursor failed");

    seed_dirty_reconcile_to_db(
        &pool,
        "lo02c-rc03: position drift — session ended with Live cursor but dirty reconcile",
    )
    .await;

    let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    // Seed: Live cursor demoted to ColdStartUnproven.
    st.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            st.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "RC-03: Live cursor must be demoted to ColdStartUnproven"
    );

    arm_via_http(&st).await;

    // Step 1: WS gate fires (ColdStartUnproven after demotion).
    let (status1, json1) = start_req(&st).await;

    assert_eq!(
        status1,
        StatusCode::FORBIDDEN,
        "RC-03: ColdStartUnproven (demoted from Live) must block start; got: {status1}"
    );
    assert_eq!(
        json1["gate"], "alpaca_ws_continuity",
        "RC-03: WS gate fires first even after cursor demotion; got: {json1}"
    );

    // Step 2: inject Live WS → dirty reconcile in DB fires.
    inject_live_ws(&st).await;

    let (status2, json2) = start_req(&st).await;

    assert_eq!(
        status2,
        StatusCode::FORBIDDEN,
        "RC-03: after fixing WS, dirty reconcile must block (403); got: {status2}"
    );
    assert_eq!(
        json2["gate"], "reconcile_truth",
        "RC-03: dirty reconcile in DB fires after cursor demotion + WS fix; got: {json2}"
    );
}

// ---------------------------------------------------------------------------
// RC-04: GapDetected cursor + ok reconcile → only WS gate fires
// ---------------------------------------------------------------------------

/// LO-02C / RC-04: GapDetected cursor with ok reconcile from prior session.
///
/// When reconcile state is ok (prior session ended cleanly) but the WS cursor
/// is GapDetected, only the WS gate fires.  The reconcile gate is NOT triggered.
///
/// This proves the system does not spuriously raise a reconcile alarm when
/// reconcile truth is clean.  A GapDetected WS state alone is the correct and
/// sufficient blocker — reconcile ok should not compound the error.
///
/// It also proves the asymmetry in the other direction: ok reconcile in DB does
/// not block restart, even when WS is degraded.  The repair path (fix WS, then
/// start) remains open when reconcile is clean.
#[tokio::test]
async fn lo02c_rc04_gap_cursor_with_ok_reconcile_only_ws_gate_fires() {
    let Some(pool) = setup_pool().await else {
        eprintln!("RC-04: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "lo02c-rc04-test";

    // Seed GapDetected cursor + ok reconcile.
    let gap_cursor = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-lo02c-rc04:canceled:2026-01-15T07:00:00Z".to_string()),
        Some("2026-01-15T07:00:00Z".to_string()),
        "lo02c-rc04: WS gap only, reconcile was clean at shutdown",
    );
    mqk_db::advance_broker_cursor(
        &pool,
        adapter_id,
        &serde_json::to_string(&gap_cursor).expect("RC-04: serialize"),
        Utc::now(),
    )
    .await
    .expect("RC-04: advance_broker_cursor failed");

    seed_ok_reconcile_to_db(&pool).await;

    let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    st.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            st.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::GapDetected { .. }
        ),
        "RC-04: GapDetected cursor must produce GapDetected continuity"
    );

    arm_via_http(&st).await;

    // Start: WS gate fires; ok reconcile is NOT the blocker.
    let (status, json) = start_req(&st).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "RC-04: GapDetected must block start at WS gate (403); got: {status}"
    );
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "RC-04: gate must be alpaca_ws_continuity (reconcile is ok and not the blocker); got: {json}"
    );
    // Confirm reconcile gate did NOT fire.
    assert_ne!(
        json["fault_class"].as_str().unwrap_or(""),
        "runtime.start_refused.reconcile_dirty",
        "RC-04: reconcile_dirty must NOT be the fault_class when reconcile is ok; got: {json}"
    );
}
