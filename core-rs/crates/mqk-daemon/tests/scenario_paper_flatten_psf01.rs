//! PAPER-SAFE-FLATTEN-ROUTE-01 — Proof tests for the operator paper flatten path.
//!
//! Verifies that `POST /api/v1/ops/action { "action_key": "flatten-paper-positions" }`:
//! - Gates correctly on mode, reconcile, arm, run, and position state.
//! - Routes sell orders through the normal OMS/outbox path (not raw broker).
//! - Prevents oversell (qty == net_qty).
//! - Rejects short positions (v1 fail-closed).
//! - Requires operator auth (token middleware).
//!
//! ## Proof matrix
//!
//! | Test     | Scope    | What it proves                                          |
//! |----------|----------|---------------------------------------------------------|
//! | PSF-01   | pure     | Non-paper mode (LiveCapital) → 403 not_paper_mode       |
//! | PSF-02   | pure     | Paper mode, no DB → 503 db_unavailable                  |
//! | PSF-11   | pure     | TokenRequired auth: request without token → 401         |
//! | PSF-03   | DB       | Dirty reconcile (in-memory "unknown") → 403             |
//! | PSF-04   | DB       | Absent arm state → 403 not_armed                        |
//! | PSF-05   | DB       | No active run → 409 no_active_run                       |
//! | PSF-08   | DB       | Running, no positions → 409 no_positions                |
//! | PSF-09   | DB       | Running, short position → 403 short_positions_not_supported_v1 |
//! | PSF-10   | DB       | Running, long position → 200 enqueued; outbox row present |
//! | PSF-12   | DB       | Running, two longs → single-symbol flatten only targets that symbol |
//!
//! PSF-01, PSF-02, PSF-11 are pure in-process (no DB required).
//! PSF-03..PSF-12 require MQK_DATABASE_URL and skip gracefully without it.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, DeploymentMode, OperatorAuthMode},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const TEST_TOKEN: &str = "test-flatten-token";

fn paper_no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper));
    build_router(st)
}

fn live_capital_no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(
        DeploymentMode::LiveCapital,
    ));
    build_router(st)
}

fn tokenized_router(state: Arc<AppState>) -> axum::Router {
    build_router(state)
}

async fn call_flatten(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    call_flatten_with_token(router, body, Some(TEST_TOKEN)).await
}

async fn call_flatten_with_token(
    router: axum::Router,
    body: serde_json::Value,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json");
    if let Some(tok) = token {
        builder = builder.header("authorization", format!("Bearer {tok}"));
    }
    let req = builder
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

/// Connect to the test DB, run migrations, and delete stale PSF-test artifacts
/// so tests don't interfere with each other.
async fn maybe_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()?;
    mqk_db::migrate(&pool).await.ok()?;
    // Clean up stale artifacts from prior PSF test runs to ensure isolation.
    sqlx::query(
        "DELETE FROM broker_order_map \
         WHERE internal_id IN ( \
             SELECT idempotency_key FROM oms_outbox \
             WHERE run_id IN ( \
                 SELECT run_id FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER' \
             ) \
         )",
    )
    .execute(&pool)
    .await
    .ok()?;
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER'")
        .execute(&pool)
        .await
        .ok()?;
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .ok()?;
    sqlx::query("DELETE FROM sys_reconcile_status_state")
        .execute(&pool)
        .await
        .ok()?;
    Some(pool)
}

async fn daemon_state_with_db(pool: sqlx::PgPool) -> Arc<AppState> {
    Arc::new(AppState::new_with_db_and_operator_auth(
        pool,
        OperatorAuthMode::ExplicitDevNoToken,
    ))
}

/// Persist a "ok" reconcile status record to the DB so `current_reconcile_snapshot`
/// returns "ok" (DB takes priority over in-memory when a record exists).
async fn set_reconcile_ok(st: &Arc<AppState>) {
    let pool = st
        .db
        .as_ref()
        .expect("DB must be configured for set_reconcile_ok");
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(chrono::Utc::now()),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: chrono::Utc::now(),
        },
    )
    .await
    .expect("persist reconcile ok");
}

/// Insert a run into the DB, arm it, begin it, and inject it as the local owner.
async fn seed_running_run(st: &Arc<AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("DB must be configured");
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "psf01-test".to_string(),
            config_hash: "psf01-config".to_string(),
            config_json: serde_json::json!({"source": "scenario_paper_flatten_psf01"}),
            host_fingerprint: "psf01-host".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(pool, run_id).await.expect("arm run");
    mqk_db::begin_run(pool, run_id).await.expect("begin run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat run");
    // Arm the integrity state so load_arm_state returns ARMED.
    mqk_db::persist_arm_state_canonical(pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("persist arm state");
    // Inject the local owner handle so status.state == "running".
    st.inject_running_loop_for_test(run_id).await;
    run_id
}

/// Set the execution snapshot on the state with the given positions.
async fn set_execution_snapshot(st: &Arc<AppState>, run_id: Uuid, positions: Vec<(String, i64)>) {
    let mut snap = st.execution_snapshot.write().await;
    *snap = Some(mqk_runtime::observability::ExecutionSnapshot {
        run_id: Some(run_id),
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: positions
                .into_iter()
                .map(|(sym, qty)| mqk_runtime::observability::PositionSnapshot {
                    symbol: sym,
                    net_qty: qty,
                })
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        snapshot_at_utc: chrono::Utc::now(),
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
    });
}

// ---------------------------------------------------------------------------
// Pure in-process tests (no DB required)
// ---------------------------------------------------------------------------

/// PSF-01: Non-paper mode (LiveCapital) → 403 not_paper_mode.
///
/// Proves that live_routing gate fires: flatten-paper-positions is paper-only.
#[tokio::test]
async fn psf_01_non_paper_mode_returns_403_not_paper_mode() {
    let (status, j) = call_flatten(
        live_capital_no_db_router(),
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "PSF-01: non-paper mode must return 403; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-01: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("not_paper_mode"),
        "PSF-01: disposition must be not_paper_mode; body: {j}"
    );
    assert!(
        j["blockers"].as_array().is_some_and(|b| !b.is_empty()),
        "PSF-01: blockers must be non-empty; body: {j}"
    );
}

/// PSF-02: Paper mode, no DB → 503 db_unavailable.
///
/// Proves that the route requires a DB for durable outbox writes.
#[tokio::test]
async fn psf_02_paper_no_db_returns_503_db_unavailable() {
    let (status, j) = call_flatten(
        paper_no_db_router(),
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "PSF-02: no-DB paper must return 503; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-02: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("db_unavailable"),
        "PSF-02: disposition must be db_unavailable; body: {j}"
    );
}

/// PSF-11: TokenRequired auth: flatten request without token → 401.
///
/// Proves the route sits behind the operator token middleware.
#[tokio::test]
async fn psf_11_requires_operator_token_returns_401_without_token() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::TokenRequired(TEST_TOKEN.to_string()),
    ));
    let (status, _j) = call_flatten_with_token(
        tokenized_router(st),
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
        None, // no token
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "PSF-11: missing token must return 401"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests (skip gracefully without MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

/// PSF-03: Dirty reconcile (in-memory "unknown") → 403 reconcile_not_ok.
#[tokio::test]
async fn psf_03_dirty_reconcile_returns_403_reconcile_not_ok() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-03: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    // Reconcile starts as "unknown" (initial_reconcile_status) — do NOT set ok.
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "PSF-03: dirty reconcile must return 403; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-03: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("reconcile_not_ok"),
        "PSF-03: disposition must be reconcile_not_ok; body: {j}"
    );
}

/// PSF-04: Reconcile ok but no arm state in DB → 403 not_armed.
#[tokio::test]
async fn psf_04_absent_arm_state_returns_403_not_armed() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-04: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    // Do NOT arm: load_arm_state returns Ok(None) → gate 4 fires.
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "PSF-04: absent arm must return 403; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-04: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("not_armed"),
        "PSF-04: disposition must be not_armed; body: {j}"
    );
}

/// PSF-05: Reconcile ok, armed, but no active run → 409 no_active_run.
#[tokio::test]
async fn psf_05_no_active_run_returns_409_no_active_run() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-05: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    // Arm the system in DB but do NOT create a run.
    mqk_db::persist_arm_state_canonical(st.db.as_ref().unwrap(), mqk_db::ArmState::Armed, None)
        .await
        .expect("persist arm");
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "PSF-05: no active run must return 409; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-05: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("no_active_run"),
        "PSF-05: disposition must be no_active_run; body: {j}"
    );
}

/// PSF-08: Running, snapshot present but no non-flat positions → 409 no_positions.
#[tokio::test]
async fn psf_08_no_positions_returns_409_no_positions() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-08: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    let run_id = seed_running_run(&st).await;
    // Set execution snapshot with NO positions.
    set_execution_snapshot(&st, run_id, vec![]).await;
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "PSF-08: no positions must return 409; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-08: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("no_positions"),
        "PSF-08: disposition must be no_positions; body: {j}"
    );
}

/// PSF-09: Running, snapshot with short position (net_qty < 0) → 403.
///
/// Proves short positions are fail-closed in v1.
#[tokio::test]
async fn psf_09_short_position_returns_403_short_positions_not_supported_v1() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-09: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    let run_id = seed_running_run(&st).await;
    // Set execution snapshot with a short AAPL position.
    set_execution_snapshot(&st, run_id, vec![("AAPL".to_string(), -50)]).await;
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions", "reason": "test" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "PSF-09: short position must return 403; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "PSF-09: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("short_positions_not_supported_v1"),
        "PSF-09: disposition must be short_positions_not_supported_v1; body: {j}"
    );
    let blockers = j["blockers"].as_array().expect("blockers is array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("AAPL")),
        "PSF-09: AAPL must appear in blockers; body: {j}"
    );
}

/// PSF-10: Running, long AAPL position (100 shares) → 200, outbox row enqueued.
///
/// Proves:
/// - Route accepts, disposition = "enqueued"
/// - Sell qty == 100 (no oversell)
/// - outbox row present in DB with signal_source = "operator_flatten"
/// - audit_event_id present in response
/// - Route used normal OMS/outbox path (not raw broker shortcut)
#[tokio::test]
async fn psf_10_flatten_all_long_positions_enqueues_outbox_row() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-10: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    let run_id = seed_running_run(&st).await;
    // 100 shares of AAPL (long).
    set_execution_snapshot(&st, run_id, vec![("AAPL".to_string(), 100)]).await;
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({
            "action_key": "flatten-paper-positions",
            "reason": "psf-10-test-flatten"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PSF-10: flatten must return 200; body: {j}"
    );
    assert_eq!(
        j["accepted"], true,
        "PSF-10: accepted must be true; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("enqueued"),
        "PSF-10: disposition must be enqueued; body: {j}"
    );

    // Verify the outbox row was written via normal path.
    let db = st.db.as_ref().unwrap();
    let warnings = j["warnings"].as_array().expect("warnings is array");
    assert!(
        warnings.iter().any(|w| {
            let s = w.as_str().unwrap_or("");
            s.contains("AAPL") && s.contains("enqueued")
        }),
        "PSF-10: AAPL enqueued entry must appear in warnings; body: {j}"
    );

    // Extract the idempotency key from the warnings message.
    let key_entry = warnings
        .iter()
        .find(|w| w.as_str().unwrap_or("").contains("AAPL"))
        .expect("PSF-10: must have AAPL warning");
    let key_str = key_entry.as_str().unwrap();
    // key=<uuid> appears in "enqueued: symbol=AAPL qty=100 side=sell key=<uuid>"
    let key = key_str
        .split("key=")
        .nth(1)
        .expect("PSF-10: key= must appear in warning message")
        .trim();

    let outbox_row = mqk_db::outbox_fetch_by_idempotency_key(db, key)
        .await
        .expect("PSF-10: outbox fetch must succeed")
        .expect("PSF-10: outbox row must exist");

    // Prove outbox row is for AAPL sell with qty 100 (no oversell).
    let order_json = &outbox_row.order_json;
    assert_eq!(
        order_json["symbol"].as_str(),
        Some("AAPL"),
        "PSF-10: outbox row must be for AAPL"
    );
    assert_eq!(
        order_json["side"].as_str(),
        Some("sell"),
        "PSF-10: outbox row must be sell"
    );
    assert_eq!(
        order_json["qty"].as_i64(),
        Some(100),
        "PSF-10: qty must be exactly 100 (no oversell)"
    );
    assert_eq!(
        order_json["signal_source"].as_str(),
        Some("operator_flatten"),
        "PSF-10: signal_source must be operator_flatten (not raw broker)"
    );
    assert_eq!(
        order_json["order_type"].as_str(),
        Some("market"),
        "PSF-10: order_type must be market"
    );

    // Prove audit trail exists.
    assert!(
        !j["audit"]["durable_targets"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "PSF-10: durable_targets must be non-empty"
    );
}

/// PSF-12: Running, two long positions (AAPL 100 + SPY 50).
/// Flatten with symbol=AAPL → only AAPL enqueued, SPY skipped.
///
/// Proves single-symbol filter is applied correctly.
#[tokio::test]
async fn psf_12_flatten_single_symbol_only_targets_that_symbol() {
    let Some(pool) = maybe_pool().await else {
        eprintln!("PSF-12: MQK_DATABASE_URL not set; skipping");
        return;
    };
    let st = daemon_state_with_db(pool).await;
    set_reconcile_ok(&st).await;
    let run_id = seed_running_run(&st).await;
    // Two long positions.
    set_execution_snapshot(
        &st,
        run_id,
        vec![("AAPL".to_string(), 100), ("SPY".to_string(), 50)],
    )
    .await;
    let router = build_router(Arc::clone(&st));
    let (status, j) = call_flatten(
        router,
        serde_json::json!({
            "action_key": "flatten-paper-positions",
            "symbol": "AAPL",
            "reason": "psf-12-test-single-symbol"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PSF-12: single-symbol flatten must return 200; body: {j}"
    );
    assert_eq!(
        j["accepted"], true,
        "PSF-12: accepted must be true; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("enqueued"),
        "PSF-12: disposition must be enqueued; body: {j}"
    );

    let warnings = j["warnings"].as_array().expect("warnings is array");
    // AAPL must be in warnings.
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("AAPL")),
        "PSF-12: AAPL must appear in warnings; body: {j}"
    );
    // SPY must NOT be in warnings (was not targeted).
    assert!(
        !warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("SPY")),
        "PSF-12: SPY must NOT appear in warnings — only AAPL was targeted; body: {j}"
    );
}
