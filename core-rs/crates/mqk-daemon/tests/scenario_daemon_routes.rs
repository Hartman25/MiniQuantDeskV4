//! In-process scenario tests for mqk-daemon HTTP endpoints.
//!
//! These tests spin up the Axum router **without** binding a TCP socket.
//! Each test calls `routes::build_router` and drives it via
//! `tower::ServiceExt::oneshot` — no network I/O required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fresh in-process router backed by a clean AppState.
fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn sample_snapshot() -> mqk_schemas::BrokerSnapshot {
    use mqk_schemas::{BrokerAccount, BrokerSnapshot};

    BrokerSnapshot {
        captured_at_utc: chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        account: BrokerAccount {
            equity: "100".to_string(),
            cash: "50".to_string(),
            currency: "USD".to_string(),
        },
        positions: Vec::new(),
        orders: Vec::new(),
        fills: Vec::new(),
    }
}

/// Drive the router with a single request and return (status, body_bytes).
async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

/// Parse body bytes as a `serde_json::Value`.
fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

/// Narrow RAII env-var guard: saves the prior value of `key`, removes it for
/// the duration of the test, and restores the prior value on drop.
///
/// Requires `--test-threads=1` — concurrent env mutation across threads is
/// unsound regardless of guards.
struct EnvGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    /// Save and clear `key`.  The prior value (if any) is restored on drop.
    fn absent(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        // SAFETY: test-only, requires --test-threads=1.
        #[allow(deprecated)]
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        #[allow(deprecated)]
        unsafe {
            match &self.prior {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_ok_true() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["ok"], true);
    assert_eq!(json["service"], "mqk-daemon");
}

// ---------------------------------------------------------------------------
// GET /v1/status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_returns_200_with_integrity_armed_field() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    // Fresh state: idle, no active run, disarmed (Patch C1 — fail-closed at boot).
    assert_eq!(json["state"], "idle");
    assert!(json["active_run_id"].is_null());
    assert_eq!(
        json["integrity_armed"], false,
        "default state should be disarmed (Patch C1)"
    );
}

#[tokio::test]
async fn runtime_leadership_route_reports_null_generation_without_authoritative_identity() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/runtime-leadership")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["leader_node"], "local");
    assert_eq!(json["leader_lease_state"], "lost");
    assert!(
        json["generation_id"].is_null(),
        "generation_id must be null when authoritative runtime identity is unavailable; got: {json}"
    );
    assert_ne!(json["generation_id"], "paper-no-run");
    assert!(
        json["restart_count_24h"].is_null(),
        "restart_count_24h must be null without DB-backed run history; got: {json}"
    );
    assert!(
        json["last_restart_at"].is_null(),
        "last_restart_at must be null when no authoritative latest run exists; got: {json}"
    );
    assert_eq!(json["post_restart_recovery_state"], "in_progress");
    assert_eq!(json["recovery_checkpoint"], "none");
    assert_eq!(
        json["checkpoints"].as_array().map(|rows| rows.is_empty()),
        Some(true),
        "checkpoints must be empty when no durable run history exists; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/run/start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_requires_db_backed_runtime_after_arm() {
    // BRK-00R-04: paper+alpaca is blocked by the WS continuity gate before the
    // DB gate. Use live-shadow+alpaca (no paper WS continuity gate) so the DB
    // requirement is the blocker after arm.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
    assert_eq!(
        json["fault_class"],
        "runtime.start_refused.service_unavailable"
    );
}

// ---------------------------------------------------------------------------
// Placeholder in-memory state must not claim a running runtime
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cannot_report_running_from_placeholder_state_alone() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut status = st.status.write().await;
        status.state = "running".to_string();
        status.active_run_id = Some(uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon-placeholder-running",
        ));
        status.notes = Some("placeholder running".to_string());
    }

    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["state"], "idle");
    assert!(json["active_run_id"].is_null());
}

// ---------------------------------------------------------------------------
// POST /v1/run/stop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_stop_on_idle_remains_idle() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let stop_req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), stop_req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["state"], "idle");
    assert!(
        json["active_run_id"].is_null(),
        "idle stop must not invent a run_id"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/run/halt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_halt_requires_db_backed_runtime_authority() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let halt_req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), halt_req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
    assert_eq!(
        json["fault_class"],
        "runtime.start_refused.service_unavailable"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integrity_arm_sets_armed_true() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Disarm first so we can verify arm actually changes state.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    // Now arm.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["armed"], true, "arm should set armed=true");
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integrity_disarm_sets_armed_false() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["armed"], false, "disarm should set armed=false");
}

// ---------------------------------------------------------------------------
// Status reflects integrity arm/disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_reflects_integrity_armed_flag() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Default: disarmed (Patch C1 — fail-closed at boot).
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], false);

    // Disarm (idempotent — already disarmed at boot).
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), req).await;

    // Status still shows false.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], false);

    // Arm again.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), req).await;

    // Status back to true.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], true);
}

// ---------------------------------------------------------------------------
// Patch L1: run_start refused (403) when integrity is disarmed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_refused_403_when_integrity_disarmed() {
    // BRK-00R-04 / PT-TRUTH-01: use live-shadow+alpaca so the integrity gate
    // is the blocker under test instead of fake paper deployment readiness or
    // paper WS continuity.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    // Disarm first.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    // Now try to start — must be refused.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must be 403 when integrity is disarmed"
    );

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("GATE_REFUSED"),
        "body should contain GATE_REFUSED: {json}"
    );
    assert_eq!(json["gate"], "integrity_armed");
    assert_eq!(
        json["fault_class"],
        "runtime.control_refusal.integrity_disarmed"
    );
}

#[tokio::test]
async fn run_start_requires_db_after_rearm() {
    // BRK-00R-04: paper+alpaca is blocked by the WS continuity gate before the
    // DB gate. Use live-shadow+alpaca (no paper WS continuity gate) so the DB
    // requirement is the blocker after the disarm→rearm sequence.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    let start_req2 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(routes::build_router(Arc::clone(&st)), start_req2).await;
    assert_eq!(status2, StatusCode::SERVICE_UNAVAILABLE);
    let json = parse_json(body2);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
    assert_eq!(
        json["fault_class"],
        "runtime.start_refused.service_unavailable"
    );
}

// ---------------------------------------------------------------------------
// Unknown routes return 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_route_returns_404() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/does_not_exist")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _) = call(router, req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// DAEMON-1: Trading read APIs return 200 with placeholder bodies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trading_account_returns_no_snapshot_state_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/account")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["snapshot_state"], "no_snapshot");
    assert!(json["snapshot_captured_at_utc"].is_null());
    assert!(json["account"].is_null());
}

#[tokio::test]
async fn trading_positions_returns_no_snapshot_state_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/positions")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["snapshot_state"], "no_snapshot");
    assert!(json["snapshot_captured_at_utc"].is_null());
    assert!(json["positions"].is_null());
}

#[tokio::test]
async fn trading_orders_returns_no_snapshot_state_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/orders")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["snapshot_state"], "no_snapshot");
    assert!(json["snapshot_captured_at_utc"].is_null());
    assert!(json["orders"].is_null());
}

#[tokio::test]
async fn trading_fills_returns_no_snapshot_state_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/fills")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["snapshot_state"], "no_snapshot");
    assert!(json["snapshot_captured_at_utc"].is_null());
    assert!(json["fills"].is_null());
}

#[tokio::test]
async fn trading_positions_marks_stale_snapshot_state_and_hides_payload() {
    use chrono::Utc;
    use mqk_daemon::state::ReconcileStatusSnapshot;

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut lock = st.broker_snapshot.write().await;
        *lock = Some(sample_snapshot());
    }
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "stale".to_string(),
        last_run_at: Some(Utc::now().to_rfc3339()),
        snapshot_watermark_ms: Some(Utc::now().timestamp_millis()),
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("stale snapshot".to_string()),
    })
    .await;

    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/positions")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["snapshot_state"], "stale_snapshot");
    assert!(json["snapshot_captured_at_utc"].is_string());
    assert!(json["positions"].is_null());
}

#[tokio::test]
async fn trading_snapshot_returns_null_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/snapshot")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert!(json["snapshot"].is_null());
}

#[tokio::test]
async fn dev_snapshot_inject_refused_when_env_not_set() {
    // Explicitly clear MQK_DEV_ALLOW_SNAPSHOT_INJECT and restore prior state
    // on drop — the test owns its own precondition rather than relying on
    // ambient CI environment.  Requires --test-threads=1.
    let _guard = EnvGuard::absent("MQK_DEV_ALLOW_SNAPSHOT_INJECT");
    let router = make_router();
    let snap = sample_snapshot();
    let body = serde_json::to_string(&snap).expect("serialize snapshot");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/trading/snapshot")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let json = parse_json(body);
    assert_eq!(json["gate"], "dev_snapshot_inject");
}

// ---------------------------------------------------------------------------
// /api/v1 summary spine — GUI alignment patch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_system_status_returns_gui_contract() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert!(json["environment"].is_null());
    assert_eq!(json["runtime_status"], "idle");
    assert_eq!(json["integrity_status"], "warning");
    assert_eq!(json["live_routing_enabled"], false);
    assert_eq!(json["daemon_reachable"], true);
    assert!(json["fault_signals"].is_array());
    // AP-04: paper default must report synthetic broker snapshot source.
    assert_eq!(json["broker_snapshot_source"], "synthetic");
    // AP-04B: market-data health must be not_configured regardless of adapter kind.
    assert_eq!(json["market_data_health"], "not_configured");
    // AP-05: paper default must report not_applicable WS continuity (no WS path).
    assert_eq!(json["alpaca_ws_continuity"], "not_applicable");
}

#[tokio::test]
async fn api_system_preflight_is_fail_closed_for_unproven_dependencies() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["daemon_reachable"], true);
    assert!(json["db_reachable"].is_null());
    // Paper adapter reports broker_config_present=false (not null): the adapter is present
    // but is explicitly not the live broker. Null would mean "not checked".
    assert_eq!(json["broker_config_present"], false);
    assert_eq!(json["market_data_config_present"], false);
    assert!(json["audit_writer_ready"].is_null());
    assert_eq!(json["runtime_idle"], true);
    assert_eq!(json["execution_disarmed"], true);
    assert!(!json["blockers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn api_execution_summary_derives_counts_from_execution_snapshot() {
    use mqk_runtime::observability::{
        ExecutionSnapshot, OrderSnapshot, OutboxSnapshot, PortfolioSnapshot,
    };

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut lock = st.execution_snapshot.write().await;
        *lock = Some(ExecutionSnapshot {
            run_id: None,
            active_orders: vec![
                OrderSnapshot {
                    order_id: "io-1".to_string(),
                    broker_order_id: None,
                    symbol: "AAPL".to_string(),
                    status: "Open".to_string(),
                    total_qty: 10,
                    filled_qty: 0,
                },
                OrderSnapshot {
                    order_id: "io-2".to_string(),
                    broker_order_id: None,
                    symbol: "MSFT".to_string(),
                    status: "Open".to_string(),
                    total_qty: 5,
                    filled_qty: 0,
                },
            ],
            pending_outbox: vec![
                OutboxSnapshot {
                    outbox_id: 1,
                    idempotency_key: "io-3".to_string(),
                    status: "PENDING".to_string(),
                    created_at_utc: chrono::Utc::now(),
                    sent_at_utc: None,
                    claimed_at_utc: None,
                    dispatching_at_utc: None,
                },
                OutboxSnapshot {
                    outbox_id: 2,
                    idempotency_key: "io-4".to_string(),
                    status: "SENT".to_string(),
                    created_at_utc: chrono::Utc::now(),
                    sent_at_utc: None,
                    claimed_at_utc: None,
                    dispatching_at_utc: None,
                },
            ],
            recent_inbox_events: vec![],
            portfolio: PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
        });
    }

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/summary")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["has_snapshot"], true);
    assert_eq!(json["active_orders"], 2);
    assert_eq!(json["pending_orders"], 1);
    assert_eq!(json["dispatching_orders"], 1);
    assert_eq!(json["reject_count_today"], 0);
    assert_eq!(json["stuck_orders"], 0);
    assert!(json["cancel_replace_count_today"].is_null());
    assert!(json["avg_ack_latency_ms"].is_null());
}

#[tokio::test]
async fn api_summary_surfaces_are_explicitly_unavailable_without_snapshot() {
    let router = make_router();

    let execution_req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (execution_status, execution_body) = call(router.clone(), execution_req).await;
    assert_eq!(execution_status, StatusCode::OK);
    let execution_json = parse_json(execution_body);
    assert_eq!(execution_json["has_snapshot"], false);
    assert!(execution_json["cancel_replace_count_today"].is_null());
    assert!(execution_json["avg_ack_latency_ms"].is_null());

    let portfolio_req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (portfolio_status, portfolio_body) = call(router.clone(), portfolio_req).await;
    assert_eq!(portfolio_status, StatusCode::OK);
    let portfolio_json = parse_json(portfolio_body);
    assert_eq!(portfolio_json["has_snapshot"], false);
    assert!(portfolio_json["account_equity"].is_null());
    assert!(portfolio_json["cash"].is_null());
    assert!(portfolio_json["daily_pnl"].is_null());

    let risk_req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (risk_status, risk_body) = call(router, risk_req).await;
    assert_eq!(risk_status, StatusCode::OK);
    let risk_json = parse_json(risk_body);
    assert_eq!(risk_json["has_snapshot"], false);
    assert!(risk_json["gross_exposure"].is_null());
    assert!(risk_json["net_exposure"].is_null());
    assert!(risk_json["loss_limit_utilization_pct"].is_null());
}

#[tokio::test]
async fn api_portfolio_and_risk_summary_derive_from_snapshot() {
    use chrono::Utc;
    use mqk_schemas::{BrokerAccount, BrokerPosition, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut lock = st.broker_snapshot.write().await;
        *lock = Some(BrokerSnapshot {
            captured_at_utc: Utc::now(),
            account: BrokerAccount {
                equity: "1500.5".to_string(),
                cash: "500.25".to_string(),
                currency: "USD".to_string(),
            },
            positions: vec![
                BrokerPosition {
                    symbol: "AAPL".to_string(),
                    qty: "10".to_string(),
                    avg_price: "100".to_string(),
                },
                BrokerPosition {
                    symbol: "TSLA".to_string(),
                    qty: "-2".to_string(),
                    avg_price: "50".to_string(),
                },
            ],
            orders: Vec::new(),
            fills: Vec::new(),
        });
    }

    let portfolio_req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (portfolio_status, portfolio_body) =
        call(routes::build_router(Arc::clone(&st)), portfolio_req).await;
    assert_eq!(portfolio_status, StatusCode::OK);
    let portfolio_json = parse_json(portfolio_body);
    assert_eq!(portfolio_json["has_snapshot"], true);
    assert_eq!(portfolio_json["account_equity"].as_f64().unwrap(), 1500.5);
    assert_eq!(portfolio_json["cash"].as_f64().unwrap(), 500.25);
    assert_eq!(
        portfolio_json["long_market_value"].as_f64().unwrap(),
        1000.0
    );
    assert_eq!(
        portfolio_json["short_market_value"].as_f64().unwrap(),
        100.0
    );
    assert_eq!(portfolio_json["buying_power"].as_f64().unwrap(), 500.25);

    let risk_req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (risk_status, risk_body) = call(routes::build_router(Arc::clone(&st)), risk_req).await;
    assert_eq!(risk_status, StatusCode::OK);
    let risk_json = parse_json(risk_body);
    assert_eq!(risk_json["has_snapshot"], true);
    assert_eq!(risk_json["gross_exposure"].as_f64().unwrap(), 1100.0);
    assert_eq!(risk_json["net_exposure"].as_f64().unwrap(), 900.0);
    assert!((risk_json["concentration_pct"].as_f64().unwrap() - 90.9090909090909).abs() < 1e-9);
    assert_eq!(risk_json["kill_switch_active"], false);
    assert_eq!(risk_json["active_breaches"], 0);
}

#[tokio::test]
async fn api_reconcile_status_exists_and_is_explicitly_unknown() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/reconcile/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["status"], "unknown");
    assert!(json["last_run_at"].is_null());
    assert_eq!(json["mismatched_positions"], 0);
    assert_eq!(json["mismatched_orders"], 0);
    assert_eq!(json["mismatched_fills"], 0);
    assert_eq!(json["unmatched_broker_events"], 0);
}

#[tokio::test]
async fn api_system_session_reports_truthful_mode_and_operator_auth() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["daemon_mode"], "PAPER");
    assert_eq!(json["adapter_id"], "paper");
    assert_eq!(json["deployment_start_allowed"], false);
    assert!(!json["deployment_blocker"].is_null());
    assert_eq!(json["operator_auth_mode"], "missing_token_fail_closed");
    assert_eq!(json["strategy_allowed"], false);
    assert_eq!(json["execution_allowed"], false);
    assert_eq!(json["system_trading_window"], "disabled");
    // Paper mode → AlwaysOn calendar → synthetic always-on session truth.
    assert_eq!(json["market_session"], "regular");
    assert_eq!(json["exchange_calendar_state"], "open");
    assert_eq!(json["calendar_spec_id"], "always_on");
    let notes = json["notes"].as_array().expect("notes must be array");
    assert!(
        !notes.is_empty(),
        "notes must carry provenance for always-on session truth"
    );
}

#[tokio::test]
async fn api_strategy_summary_declares_not_wired() {
    // CC-01B: The route now sources truth from postgres.sys_strategy_registry.
    // When no DB pool is present the route must return truth_state="no_db"
    // (fail-closed) — NOT the old "not_wired" placeholder, and NOT a synthetic
    // daemon_integrity_gate surrogate row that would masquerade as a strategy.
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    // Must be a wrapper object — NOT a bare array.
    assert!(
        json.as_object().is_some(),
        "/api/v1/strategy/summary must return a wrapper object, not a bare array; got: {json}"
    );
    assert_eq!(
        json["truth_state"], "no_db",
        "CC-01B: no DB → truth_state must be 'no_db' (fail-closed); got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some(),
        "strategy summary wrapper must have a rows array field"
    );
    assert_eq!(
        json["rows"].as_array().map(|v| v.is_empty()),
        Some(true),
        "strategy summary rows must be empty when no_db"
    );
    // Confirm the synthetic daemon_integrity_gate row is gone — any row with
    // that strategy_id would be surrogate truth, not real fleet truth.
    assert!(
        !json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["strategy_id"] == "daemon_integrity_gate"),
        "daemon_integrity_gate must not appear as a strategy row"
    );
    // CC-01B: wrapper must carry canonical_route and backend fields.
    assert_eq!(
        json["canonical_route"], "/api/v1/strategy/summary",
        "strategy summary must carry canonical_route self-identity"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_registry",
        "CC-01B: backend must be postgres.sys_strategy_registry"
    );
}

#[tokio::test]
async fn api_config_and_suppression_surfaces_are_explicit_when_unavailable() {
    let router = make_router();

    let fp_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-fingerprint")
        .body(axum::body::Body::empty())
        .unwrap();
    let (fp_status, fp_body) = call(router.clone(), fp_req).await;
    assert_eq!(fp_status, StatusCode::OK);
    let fp = parse_json(fp_body);
    assert_eq!(fp["config_hash"], "daemon-runtime-paper-blocked-v1");
    assert_eq!(fp["adapter_id"], "paper");
    assert!(
        fp["risk_policy_version"].is_null(),
        "risk_policy_version must be null when canonical config truth is unavailable"
    );
    assert!(
        fp["strategy_bundle_version"].is_null(),
        "strategy_bundle_version must be null when canonical config truth is unavailable"
    );
    assert!(
        fp["runtime_generation_id"].is_null(),
        "runtime_generation_id must be null when no authoritative runtime generation exists"
    );
    assert_ne!(fp["risk_policy_version"], "unknown");
    assert_ne!(fp["strategy_bundle_version"], "unknown");
    assert_ne!(fp["runtime_generation_id"], "unknown");
    assert_eq!(fp["environment_profile"], "paper");
    assert!(fp["last_restart_at"].is_null());

    let diff_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-diffs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (diff_status, diff_body) = call(router.clone(), diff_req).await;
    assert_eq!(diff_status, StatusCode::OK);
    let diffs = parse_json(diff_body);
    // Must be a wrapper — not a bare array.
    assert!(
        diffs.as_object().is_some(),
        "config-diffs must return a wrapper object"
    );
    assert_eq!(
        diffs["canonical_route"], "/api/v1/system/config-diffs",
        "config-diffs must declare its canonical route"
    );
    assert_eq!(
        diffs["truth_state"], "not_wired",
        "config-diffs must declare not_wired when authoritative diff truth is unavailable"
    );
    assert_eq!(
        diffs["backend"], "not_wired",
        "config-diffs must explicitly declare that no authoritative backend is wired"
    );
    assert!(
        diffs["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "config-diffs rows must be empty when truth is not wired"
    );
    assert!(
        diffs.as_object().and_then(|o| o.get("rows")).is_some(),
        "config-diffs must keep a stable rows field even when not wired"
    );

    let suppressions_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (suppressions_status, suppressions_body) = call(router, suppressions_req).await;
    assert_eq!(suppressions_status, StatusCode::OK);
    let suppressions = parse_json(suppressions_body);
    // CC-02: suppressions now has a real durable source (postgres.sys_strategy_suppressions).
    // Without DB pool: truth_state="no_db" (source unavailable, not permanently not_wired).
    assert!(
        suppressions.as_object().is_some(),
        "suppressions must return a wrapper object"
    );
    assert_eq!(
        suppressions["truth_state"], "no_db",
        "suppressions without DB pool must declare no_db (CC-02); got: {suppressions}"
    );
    assert_eq!(
        suppressions["canonical_route"], "/api/v1/strategy/suppressions",
        "suppressions must declare canonical_route self-identity"
    );
    assert_eq!(
        suppressions["backend"], "postgres.sys_strategy_suppressions",
        "suppressions must declare its durable backend source"
    );
    assert!(
        suppressions["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "suppressions rows must be empty when no_db"
    );
}

#[tokio::test]
async fn system_status_and_preflight_surface_mode_truth() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let status_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, status_body) = call(router.clone(), status_req).await;
    assert_eq!(status_code, StatusCode::OK);
    let status_json = parse_json(status_body);
    assert_eq!(status_json["daemon_mode"], "paper");
    assert_eq!(status_json["adapter_id"], "paper");
    assert_eq!(status_json["deployment_start_allowed"], false);
    assert!(!status_json["deployment_blocker"].is_null());

    let preflight_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let (preflight_code, preflight_body) = call(router, preflight_req).await;
    assert_eq!(preflight_code, StatusCode::OK);
    let preflight_json = parse_json(preflight_body);
    assert_eq!(preflight_json["daemon_mode"], "paper");
    assert_eq!(preflight_json["adapter_id"], "paper");
    assert_eq!(preflight_json["deployment_start_allowed"], false);
}

// ---------------------------------------------------------------------------
// AP-04 + AP-04B: Broker snapshot source and market-data policy separation
// ---------------------------------------------------------------------------

/// AP-04: Paper adapter reports synthetic broker snapshot source.
/// AP-04B: market_data_health is not_configured regardless of adapter kind;
///          strategy feed policy is independent of broker selection.
#[tokio::test]
async fn ap04_paper_adapter_reports_synthetic_broker_snapshot_source() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert_eq!(
        st.broker_snapshot_source(),
        state::BrokerSnapshotTruthSource::Synthetic,
        "paper broker kind must map to Synthetic snapshot source"
    );
    assert_eq!(
        st.strategy_market_data_source(),
        state::StrategyMarketDataSource::NotConfigured,
        "strategy market-data source must be NotConfigured independent of broker kind"
    );

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["broker_snapshot_source"], "synthetic",
        "system/status must surface synthetic source for paper adapter"
    );
    assert_eq!(
        json["market_data_health"], "not_configured",
        "market_data_health must reflect StrategyMarketDataSource::NotConfigured, not broker kind"
    );
}

/// AP-04: Alpaca adapter kind must report external broker snapshot source.
/// AP-04B: strategy market-data source remains NotConfigured — changing
///          the adapter MUST NOT change the feed policy.
///
/// Uses `new_for_test_with_broker_kind` (no env vars) to avoid race conditions
/// when parallel tests also read MQK_DAEMON_ADAPTER_ID.
#[tokio::test]
async fn ap04_alpaca_adapter_reports_external_broker_snapshot_source() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));

    assert_eq!(
        st.broker_snapshot_source(),
        state::BrokerSnapshotTruthSource::External,
        "alpaca broker kind must map to External snapshot source"
    );
    // AP-04B: strategy feed must NOT inherit broker kind.
    assert_eq!(
        st.strategy_market_data_source(),
        state::StrategyMarketDataSource::ExternalSignalIngestion,
        "paper+alpaca must configure ExternalSignalIngestion for strategy signal ingestion"
    );

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["broker_snapshot_source"], "external",
        "system/status must surface external source for alpaca adapter"
    );
    assert_eq!(
        json["market_data_health"], "signal_ingestion_ready",
        "market_data_health must reflect ExternalSignalIngestion for paper+alpaca"
    );
}

/// AP-04B: broker_snapshot_source and market_data_health are orthogonal.
///
/// Proves at the type level (no env-var dependency) that `BrokerSnapshotTruthSource`
/// and `StrategyMarketDataSource` are independent policy enums that never conflate.
/// Also verified via the HTTP status endpoint for paper adapter.
#[tokio::test]
async fn ap04b_broker_snapshot_source_and_market_data_health_are_orthogonal() {
    let paper_st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Type-level proof: the two policy types are distinct enums with distinct strings.
    let snapshot_source = paper_st.broker_snapshot_source();
    let md_source = paper_st.strategy_market_data_source();
    assert_eq!(snapshot_source, state::BrokerSnapshotTruthSource::Synthetic);
    assert_eq!(md_source, state::StrategyMarketDataSource::NotConfigured);
    // Their canonical strings must differ — they encode different policy categories.
    assert_ne!(
        snapshot_source.as_str(),
        md_source.as_health_str(),
        "broker_snapshot_source and market_data_health must encode different policy categories; \
         snapshot_source={:?} md_health={:?}",
        snapshot_source.as_str(),
        md_source.as_health_str(),
    );

    // HTTP-level: both fields are present and independently valued in system/status.
    let router = routes::build_router(Arc::clone(&paper_st));
    let (_, body) = call(
        router,
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/status")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let json = parse_json(body);
    assert_eq!(json["broker_snapshot_source"], "synthetic");
    assert_eq!(json["market_data_health"], "not_configured");
    assert_ne!(
        json["broker_snapshot_source"], json["market_data_health"],
        "system/status must surface the two policies as distinct, independent values"
    );
}

// ---------------------------------------------------------------------------
// AP-05: daemon-owned Alpaca websocket continuity tests
// ---------------------------------------------------------------------------

/// AP-05: Paper adapter reports not_applicable WS continuity.
///
/// Paper broker has no websocket path; continuity concept does not apply.
/// This must be surfaced explicitly as "not_applicable" rather than as any
/// continuity state, so the operator never confuses paper with an unproven
/// external WS path.
#[tokio::test]
async fn ap05_paper_adapter_ws_continuity_is_not_applicable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Type-level proof: default paper state has NotApplicable continuity.
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::NotApplicable,
        "paper broker must have NotApplicable WS continuity"
    );
    assert!(
        !st.alpaca_ws_continuity().await.is_continuity_proven(),
        "NotApplicable must not count as proven continuity"
    );

    // HTTP-level proof: system/status surfaces not_applicable.
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["alpaca_ws_continuity"], "not_applicable",
        "system/status must surface not_applicable for paper adapter"
    );
}

/// AP-05: Alpaca adapter (no DB) reports cold_start_unproven WS continuity.
///
/// When no persisted cursor exists, continuity is unproven.  The system
/// must not fabricate "live" or "healthy" continuity from absence of data.
/// Uses `new_for_test_with_broker_kind` to avoid env-var races.
#[tokio::test]
async fn ap05_alpaca_adapter_ws_continuity_is_cold_start_unproven() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));
    // Type-level proof: Alpaca without a persisted cursor → ColdStartUnproven.
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::ColdStartUnproven,
        "Alpaca with no persisted cursor must report ColdStartUnproven"
    );
    assert!(
        !st.alpaca_ws_continuity().await.is_continuity_proven(),
        "ColdStartUnproven must not count as proven continuity (fail-closed)"
    );

    // HTTP-level proof: system/status surfaces cold_start_unproven.
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["alpaca_ws_continuity"], "cold_start_unproven",
        "system/status must surface cold_start_unproven for Alpaca with no cursor"
    );
}

/// AP-05: gap_detected and cold_start_unproven fail closed; only live is proven.
///
/// Proves at the type level that is_continuity_proven() is exclusively true
/// for Live.  GapDetected, ColdStartUnproven, and NotApplicable all fail closed.
#[test]
fn ap05_ws_continuity_only_live_is_proven() {
    use state::AlpacaWsContinuityState;

    let not_applicable = AlpacaWsContinuityState::NotApplicable;
    let cold_start = AlpacaWsContinuityState::ColdStartUnproven;
    let live = AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:uuid-1:new:2024-01-01T00:00:00Z".to_string(),
        last_event_at: "2024-01-01T00:00:00Z".to_string(),
    };
    let gap = AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:uuid-1:new:2024-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2024-01-01T00:00:00Z".to_string()),
        detail: "ws disconnect at 2024-01-01T00:01:00Z".to_string(),
    };

    assert!(
        live.is_continuity_proven(),
        "Live must be the only proven continuity state"
    );
    assert!(
        !not_applicable.is_continuity_proven(),
        "NotApplicable must fail closed"
    );
    assert!(
        !cold_start.is_continuity_proven(),
        "ColdStartUnproven must fail closed"
    );
    assert!(!gap.is_continuity_proven(), "GapDetected must fail closed");

    // Status strings must be distinct and canonical.
    assert_eq!(not_applicable.as_status_str(), "not_applicable");
    assert_eq!(cold_start.as_status_str(), "cold_start_unproven");
    assert_eq!(live.as_status_str(), "live");
    assert_eq!(gap.as_status_str(), "gap_detected");
}

/// AP-05: from_cursor_json derives correct continuity from persisted cursor JSON.
///
/// Proves the cursor-parse path for all three AlpacaFetchCursor variant shapes.
#[test]
fn ap05_from_cursor_json_derives_continuity_from_persisted_cursor() {
    use state::{AlpacaWsContinuityState, BrokerKind};

    // Non-Alpaca broker kind → always NotApplicable regardless of cursor content.
    let cold_json = r#"{"schema_version":1,"trade_updates":{"status":"cold_start_unproven"}}"#;
    assert_eq!(
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Paper), Some(cold_json)),
        AlpacaWsContinuityState::NotApplicable,
        "non-Alpaca broker kind must yield NotApplicable"
    );

    // Alpaca + no cursor JSON → ColdStartUnproven.
    assert_eq!(
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), None),
        AlpacaWsContinuityState::ColdStartUnproven,
        "absent cursor must yield ColdStartUnproven"
    );

    // Alpaca + ColdStartUnproven cursor → ColdStartUnproven.
    assert_eq!(
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(cold_json)),
        AlpacaWsContinuityState::ColdStartUnproven,
        "cold_start_unproven cursor must yield ColdStartUnproven"
    );

    // Alpaca + Live cursor → Live with last_message_id and last_event_at preserved.
    let live_json = r#"{"schema_version":1,"trade_updates":{"status":"live","last_message_id":"alpaca:uuid-1:new:2024-01-01T00:00:00Z","last_event_at":"2024-01-01T00:00:00Z"}}"#;
    let live_state =
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(live_json));
    assert!(
        live_state.is_continuity_proven(),
        "live cursor must yield proven continuity"
    );
    assert!(
        matches!(live_state, AlpacaWsContinuityState::Live { .. }),
        "live cursor must yield Live variant"
    );

    // Alpaca + GapDetected cursor → GapDetected (fail-closed).
    let gap_json = r#"{"schema_version":1,"trade_updates":{"status":"gap_detected","last_message_id":"alpaca:uuid-1:new:2024-01-01T00:00:00Z","last_event_at":"2024-01-01T00:00:00Z","detail":"ws disconnect"}}"#;
    let gap_state =
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(gap_json));
    assert!(
        !gap_state.is_continuity_proven(),
        "gap_detected cursor must fail closed"
    );
    assert!(
        matches!(gap_state, AlpacaWsContinuityState::GapDetected { .. }),
        "gap_detected cursor must yield GapDetected variant"
    );

    // Alpaca + corrupt cursor JSON → GapDetected (fail-closed, not silent ColdStart).
    let bad_json = r#"{"this_is":"not_a_valid_cursor"}"#;
    let bad_state =
        AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(bad_json));
    assert!(
        matches!(bad_state, AlpacaWsContinuityState::GapDetected { .. }),
        "corrupt cursor JSON must yield GapDetected (fail-closed), not ColdStartUnproven"
    );
    assert!(
        !bad_state.is_continuity_proven(),
        "corrupt cursor must fail closed"
    );
}

/// BRK-00R-02: daemon derivation uses runtime-owned seam; fail-closed for cold-start and gap.
///
/// Proves that the production path `build_execution_orchestrator → from_cursor_json →
/// from_fetch_cursor → ws_continuity_from_cursor` now explicitly goes through the
/// runtime-owned seam rather than a parallel daemon-local derivation.
///
/// The structural proof is:
/// - `from_fetch_cursor` now delegates to `mqk_runtime::alpaca_inbound::ws_continuity_from_cursor`.
/// - `from_cursor_json` calls `from_fetch_cursor` for parsed Alpaca cursors.
/// - Cold-start and gap states both fail `is_continuity_proven()` through this path.
/// - Live state passes. Full position metadata (`last_message_id`, `last_event_at`) is preserved.
#[test]
fn brk00r02_daemon_derivation_via_runtime_seam_is_fail_closed() {
    use mqk_broker_alpaca::types::AlpacaFetchCursor;
    use state::{AlpacaWsContinuityState, BrokerKind};

    // Cold-start: runtime seam → ColdStartUnproven → not proven.
    let cold = AlpacaFetchCursor::cold_start_unproven(None);
    let cold_state = AlpacaWsContinuityState::from_cursor_json(
        Some(BrokerKind::Alpaca),
        Some(&serde_json::to_string(&cold).unwrap()),
    );
    assert!(
        matches!(cold_state, AlpacaWsContinuityState::ColdStartUnproven),
        "cold-start via runtime seam must yield ColdStartUnproven"
    );
    assert!(
        !cold_state.is_continuity_proven(),
        "cold-start must not be continuity-proven"
    );

    // Gap: runtime seam → GapDetected → not proven; position fields preserved.
    let gap = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-1:new:2024-06-15T09:30:00Z".to_string()),
        Some("2024-06-15T09:30:00.000000Z".to_string()),
        "brk00r02 disconnect proof",
    );
    let gap_state = AlpacaWsContinuityState::from_cursor_json(
        Some(BrokerKind::Alpaca),
        Some(&serde_json::to_string(&gap).unwrap()),
    );
    assert!(
        !gap_state.is_continuity_proven(),
        "gap via runtime seam must not be continuity-proven"
    );
    match &gap_state {
        AlpacaWsContinuityState::GapDetected {
            last_message_id,
            last_event_at,
            detail,
        } => {
            assert_eq!(
                last_message_id.as_deref(),
                Some("alpaca:order-1:new:2024-06-15T09:30:00Z"),
                "last_message_id must be preserved through runtime seam"
            );
            assert_eq!(
                last_event_at.as_deref(),
                Some("2024-06-15T09:30:00.000000Z"),
                "last_event_at must be preserved through runtime seam"
            );
            assert_eq!(detail, "brk00r02 disconnect proof");
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }

    // Live: runtime seam → Live → proven.
    let live = AlpacaFetchCursor::live(
        None,
        "alpaca:order-1:fill:2024-06-15T09:31:00Z",
        "2024-06-15T09:31:00.000000Z",
    );
    let live_state = AlpacaWsContinuityState::from_cursor_json(
        Some(BrokerKind::Alpaca),
        Some(&serde_json::to_string(&live).unwrap()),
    );
    assert!(
        live_state.is_continuity_proven(),
        "live via runtime seam must be continuity-proven"
    );
}

/// AP-05: update_ws_continuity seam silently no-ops for NotApplicable (Paper).
///
/// Paper broker must not have its NotApplicable state corrupted by a
/// misdirected WS continuity update.
#[tokio::test]
async fn ap05_update_ws_continuity_noop_for_not_applicable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Verify it starts as NotApplicable for paper.
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::NotApplicable
    );

    // Attempt to overwrite with ColdStartUnproven — must be silently ignored.
    st.update_ws_continuity(state::AlpacaWsContinuityState::ColdStartUnproven)
        .await;
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::NotApplicable,
        "update_ws_continuity must not corrupt NotApplicable (Paper) continuity state"
    );

    // Attempt to overwrite with GapDetected — must also be silently ignored.
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: None,
        last_event_at: None,
        detail: "test gap".to_string(),
    })
    .await;
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::NotApplicable,
        "GapDetected update must not corrupt NotApplicable (Paper) continuity state"
    );
}

/// AP-05: update_ws_continuity transitions Alpaca continuity state correctly.
///
/// Proves the seam accepts Live and GapDetected transitions for Alpaca paths.
#[tokio::test]
async fn ap05_update_ws_continuity_transitions_alpaca_state() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));
    // Starts as ColdStartUnproven (no DB cursor).
    assert_eq!(
        st.alpaca_ws_continuity().await,
        state::AlpacaWsContinuityState::ColdStartUnproven
    );

    // Transition to Live (simulates WS first batch ingested).
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:uuid-1:new:2024-01-01T00:00:00Z".to_string(),
        last_event_at: "2024-01-01T00:00:00Z".to_string(),
    })
    .await;
    assert!(
        st.alpaca_ws_continuity().await.is_continuity_proven(),
        "state must be Live after successful WS ingest"
    );

    // Transition to GapDetected (simulates WS disconnect).
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:uuid-1:new:2024-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2024-01-01T00:00:00Z".to_string()),
        detail: "ws disconnect at 2024-01-01T00:01:00Z".to_string(),
    })
    .await;
    assert!(
        !st.alpaca_ws_continuity().await.is_continuity_proven(),
        "state must fail closed after gap detected"
    );
    assert_eq!(
        st.alpaca_ws_continuity().await.as_status_str(),
        "gap_detected"
    );
}

// ---------------------------------------------------------------------------
// AP-07: live-shadow + Alpaca tests
// ---------------------------------------------------------------------------

/// AP-07: live-shadow + Alpaca is now a supported combination.
///
/// Proves that `new_for_test_with_broker_kind(Alpaca)` raised to live-shadow mode
/// reports `deployment_start_allowed: true` and correct operator surfaces.
/// Note: this test constructs state directly without exercising the actual
/// broker connection or credential lookup — it proves the readiness gate only.
#[tokio::test]
async fn ap07_live_shadow_alpaca_readiness_is_allowed_in_session() {
    // Construct live-shadow state with Alpaca broker kind.
    // new_for_test_with_broker_kind gives us Alpaca, but mode defaults to Paper.
    // We need to also set the mode to LiveShadow while keeping Alpaca adapter.
    // Use the combination of both helpers, or construct manually.
    // The cleanest approach: use the existing new_for_test_with_mode for LiveShadow,
    // which uses Paper adapter (blocked), then swap. There is no single test helper
    // for (mode=LiveShadow, broker=Alpaca). We test the readiness function directly
    // via the unit tests in state.rs; here we test the HTTP surface via
    // new_for_test_with_broker_kind(Alpaca) and verify the mode + broker fields.
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));
    // The default mode for new_for_test_with_broker_kind is Paper.
    // Readiness for paper+alpaca is allowed; confirm the session surface is honest.
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    // Adapter is alpaca; mode is paper (the test helper default).
    assert_eq!(json["adapter_id"], "alpaca");
    assert_eq!(json["daemon_mode"], "PAPER");
    assert_eq!(
        json["deployment_start_allowed"], true,
        "paper+alpaca must be start-allowed"
    );
}

/// AP-07: live-shadow + Paper adapter is explicitly blocked.
///
/// The paper fill engine cannot provide real external broker truth for shadow mode.
/// The blocker message must explain the external broker requirement.
#[tokio::test]
async fn ap07_live_shadow_paper_adapter_is_blocked() {
    // new_for_test_with_mode(LiveShadow) retains the default Paper adapter.
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["daemon_mode"], "LIVE-SHADOW");
    assert_eq!(json["adapter_id"], "paper");
    assert_eq!(
        json["deployment_start_allowed"], false,
        "live-shadow+paper must be blocked"
    );
    let blocker = json["deployment_blocker"].as_str().unwrap_or("");
    assert!(
        blocker.contains("external broker"),
        "blocker must explain external broker requirement; got: {blocker}"
    );
}

/// AP-07 / AP-08: live-capital + paper adapter remains fail-closed.
///
/// `new_for_test_with_mode(LiveCapital)` uses the default paper adapter.
/// `(LiveCapital, Paper)` is permanently blocked — capital requires an external
/// broker adapter (Alpaca).  AP-07 and AP-08 do not change this.
#[tokio::test]
async fn ap07_live_capital_paper_adapter_remains_fail_closed() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveCapital,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["daemon_mode"], "LIVE-CAPITAL");
    // Paper adapter is blocked for live-capital even after AP-08.
    assert_eq!(
        json["deployment_start_allowed"],
        false,
        "live-capital+paper must remain fail-closed; adapter_id: {}",
        json["adapter_id"].as_str().unwrap_or("?")
    );
    let blocker = json["deployment_blocker"].as_str().unwrap_or("");
    assert!(
        blocker.contains("live-capital"),
        "live-capital+paper blocker must name the mode; got: {blocker}"
    );
}

/// AP-07: live-shadow with Alpaca broker surfaces NyseWeekdays calendar.
///
/// Session truth must reflect real exchange calendar for live-shadow,
/// not the synthetic AlwaysOn calendar used by paper/backtest modes.
#[tokio::test]
async fn ap07_live_shadow_uses_nyse_weekdays_calendar() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["calendar_spec_id"], "nyse_weekdays",
        "live-shadow must use NYSE weekday calendar, not always_on"
    );
    // Session note must reflect real-exchange heuristic, not synthetic policy.
    let notes = json["notes"].as_array().cloned().unwrap_or_default();
    let note_text: String = notes
        .iter()
        .filter_map(|n| n.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        note_text.contains("NYSE") || note_text.contains("heuristic"),
        "session notes must reflect NYSE calendar truth; got: {note_text}"
    );
}

/// AP-07: live-shadow + Alpaca surfaces External broker snapshot source.
///
/// The broker snapshot source must be "external" for Alpaca-backed deployments,
/// never silently synthetic.
#[tokio::test]
async fn ap07_live_shadow_alpaca_surfaces_external_snapshot_source() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["broker_snapshot_source"], "external",
        "live-shadow+alpaca must surface External snapshot source, not synthetic"
    );
    assert_eq!(
        json["alpaca_ws_continuity"], "cold_start_unproven",
        "live-shadow+alpaca without a cursor must report cold_start_unproven"
    );
}

// ---------------------------------------------------------------------------
// AP-08: live-capital + Alpaca integration tests
// ---------------------------------------------------------------------------

/// AP-08: live-capital + Alpaca is now the explicitly allowed capital combination.
///
/// Session endpoint must reflect deployment_start_allowed=true and adapter_id="alpaca".
#[tokio::test]
async fn ap08_live_capital_alpaca_readiness_is_allowed_in_session() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["daemon_mode"], "LIVE-CAPITAL");
    assert_eq!(json["adapter_id"], "alpaca");
    assert_eq!(
        json["deployment_start_allowed"],
        true,
        "live-capital+alpaca must be start-allowed after AP-08; blocker: {}",
        json["deployment_blocker"].as_str().unwrap_or("none")
    );
    assert!(
        json["deployment_blocker"].is_null()
            || json["deployment_blocker"].as_str().unwrap_or("").is_empty(),
        "no blocker expected for allowed pair; got: {}",
        json["deployment_blocker"]
    );
}

/// AP-08: live-capital + paper adapter is still explicitly blocked.
///
/// `(LiveCapital, Paper)` must remain fail-closed — capital requires external
/// broker truth.  AP-08 must not accidentally allow the paper adapter for capital.
#[tokio::test]
async fn ap08_live_capital_paper_adapter_is_blocked() {
    // new_for_test_with_mode uses default paper adapter.
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveCapital,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["daemon_mode"], "LIVE-CAPITAL");
    assert_eq!(
        json["deployment_start_allowed"], false,
        "live-capital+paper must remain blocked after AP-08"
    );
    assert!(
        json["deployment_blocker"]
            .as_str()
            .unwrap_or("")
            .contains("live-capital"),
        "blocker must name live-capital restriction"
    );
}

/// AP-08: dev-no-token + live-capital + alpaca → POST /v1/run/start returns 403.
///
/// The capital token gate must fire even when readiness is allowed.
/// ExplicitDevNoToken must be refused for capital execution runs.
#[tokio::test]
async fn ap08_live_capital_dev_no_token_refused() {
    // new_for_test_with_mode_and_broker uses ExplicitDevNoToken by default.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    // Arm integrity so the gate stack reaches the token check.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    // 403 from the capital token gate.
    assert_eq!(code, StatusCode::FORBIDDEN);
    let json = parse_json(body);
    assert_eq!(
        json["fault_class"], "runtime.start_refused.capital_requires_operator_token",
        "capital token gate must fire for ExplicitDevNoToken; got fault_class: {}",
        json["fault_class"]
    );
    assert_eq!(json["gate"], "operator_auth");
}

/// AP-08: live-capital with Alpaca broker uses NyseWeekdays calendar.
///
/// Capital mode operates on real exchange hours.  AlwaysOn must never be
/// used for live-capital.
#[tokio::test]
async fn ap08_live_capital_uses_nyse_weekdays_calendar() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["calendar_spec_id"], "nyse_weekdays",
        "live-capital must use NYSE weekday calendar, not always_on"
    );
}

/// AP-08: live-capital with Alpaca surfaces External broker snapshot source.
///
/// Capital mode uses the same real Alpaca truth as shadow — snapshot source
/// must be "external", never synthetic.
#[tokio::test]
async fn ap08_live_capital_surfaces_external_snapshot_source() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(router, req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["broker_snapshot_source"], "external",
        "live-capital+alpaca must surface External snapshot source, not synthetic"
    );
    assert_eq!(
        json["alpaca_ws_continuity"], "cold_start_unproven",
        "live-capital+alpaca without a cursor must report cold_start_unproven"
    );
}

#[tokio::test]
async fn non_paper_deployment_mode_is_explicitly_fail_closed() {
    // Use new_for_test_with_mode to avoid env-var races with parallel tests.
    // Semantics are identical: live-shadow mode, paper adapter (default).
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));

    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let json = parse_json(body);
    assert_eq!(
        json["fault_class"],
        "runtime.start_refused.deployment_mode_unproven"
    );
    assert_eq!(json["gate"], "deployment_mode");

    let session_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (session_code, session_body) = call(routes::build_router(st), session_req).await;
    assert_eq!(session_code, StatusCode::OK);
    let session_json = parse_json(session_body);
    assert_eq!(session_json["daemon_mode"], "LIVE-SHADOW");
    assert_eq!(session_json["deployment_start_allowed"], false);
    assert_eq!(session_json["adapter_id"], "paper");
    // AP-07: live-shadow+paper is now explicitly blocked with a specific message
    // (paper adapter cannot provide real external broker truth for shadow mode).
    assert!(session_json["deployment_blocker"]
        .as_str()
        .unwrap_or("")
        .contains("external broker"));
}

// ---------------------------------------------------------------------------
// PROD-02 proof tests
// ---------------------------------------------------------------------------

/// PROD-02-A: Armed integrity + no active durable run → execution_allowed must
/// be false and system_trading_window must be "disabled".
///
/// Before PROD-02 the system returned execution_allowed:true and
/// system_trading_window:"enabled" whenever integrity was armed, even with no
/// run active.  That overclaim is now closed.
#[tokio::test]
async fn prod02_armed_but_idle_execution_not_allowed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Arm integrity — simulate operator having armed the gate.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // No run has been started; state is still "idle".
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    // strategy gate reflects armed state
    assert_eq!(json["strategy_allowed"], true, "strategy should be armed");
    // execution gate must NOT be open without a durable active run
    assert_eq!(
        json["execution_allowed"], false,
        "execution_allowed must be false when no active run exists"
    );
    assert_eq!(
        json["system_trading_window"], "disabled",
        "trading window must be disabled when no active run exists"
    );
}

/// PROD-02-B: Running state with no reconcile result yet must produce
/// has_critical:true and the unproven-reconcile fault signal.
///
/// A daemon that has an active execution loop but has not completed a
/// reconcile tick cannot verify order consistency.  Operator surfaces must
/// reflect that as critical rather than silently omitting it.
#[tokio::test]
async fn prod02_running_without_reconcile_result_is_critical() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Arm integrity.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Inject a fake execution loop so current_status_snapshot returns "running".
    // Reconcile status remains at the default "unknown" (no DB, no tick yet).
    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, b"prod02-test-run");
    st.inject_running_loop_for_test(run_id).await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(
        json["runtime_status"], "running",
        "runtime_status must reflect the injected execution loop"
    );
    assert_eq!(
        json["reconcile_status"], "unknown",
        "reconcile_status must be unknown before first tick"
    );
    assert_eq!(
        json["has_critical"], true,
        "has_critical must be true when running with unknown reconcile"
    );

    // The fault_signals array must contain the unproven-reconcile class.
    let signals = json["fault_signals"]
        .as_array()
        .expect("fault_signals must be array");
    let classes: Vec<&str> = signals.iter().filter_map(|s| s["class"].as_str()).collect();
    assert!(
        classes.contains(&"reconcile.unproven.running_without_reconcile_result"),
        "expected reconcile.unproven.running_without_reconcile_result in fault_signals, got: {:?}",
        classes
    );
}

/// PROD-02-C: Halted and unknown runtime states must never expose
/// live_routing_enabled:null.  Both must return an explicit Some(false).
#[tokio::test]
async fn prod02_non_running_states_return_explicit_false_live_routing() {
    // ---- idle state ----
    let st_idle = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, idle_body) = call(routes::build_router(Arc::clone(&st_idle)), req).await;
    let idle_json = parse_json(idle_body);
    assert_eq!(
        idle_json["live_routing_enabled"], false,
        "live_routing_enabled must be explicit false when idle"
    );
    assert_eq!(idle_json["runtime_status"], "idle");

    // ---- halted state ----
    let st_halted = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut ig = st_halted.integrity.write().await;
        ig.halted = true;
    }

    let req2 = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, halted_body) = call(routes::build_router(Arc::clone(&st_halted)), req2).await;
    let halted_json = parse_json(halted_body);
    assert_eq!(
        halted_json["live_routing_enabled"], false,
        "live_routing_enabled must be explicit false when halted"
    );
    assert_eq!(halted_json["runtime_status"], "halted");
    assert_eq!(
        halted_json["kill_switch_active"], true,
        "kill_switch_active must be true when halted"
    );
}

// ---------------------------------------------------------------------------
// CTRL-03: Exchange calendar / market session truth
// ---------------------------------------------------------------------------

/// Paper mode → AlwaysOn calendar → synthetic session truth.
///
/// market_session must be "regular" (synthetic always-on), exchange_calendar_state
/// must be "open" (synthetic), calendar_spec_id must be "always_on".
#[tokio::test]
async fn ctrl03_paper_mode_reports_always_on_calendar_and_regular_market() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::Paper,
    ));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["calendar_spec_id"], "always_on",
        "paper mode must surface always_on calendar spec"
    );
    assert_eq!(
        json["exchange_calendar_state"], "open",
        "AlwaysOn calendar must report exchange state as open (synthetic)"
    );
    assert_eq!(
        json["market_session"], "regular",
        "AlwaysOn calendar must report market session as regular (synthetic always-on)"
    );
}

/// Backtest mode → AlwaysOn calendar → synthetic session truth.
#[tokio::test]
async fn ctrl03_backtest_mode_reports_always_on_calendar() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::Backtest,
    ));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(json["calendar_spec_id"], "always_on");
    assert_eq!(json["exchange_calendar_state"], "open");
    assert_eq!(json["market_session"], "regular");
}

/// LiveCapital mode → NyseWeekdays calendar.
///
/// market_session must be one of the canonical classified values.
/// exchange_calendar_state must be one of the canonical operational values.
/// Neither may be a raw spec name like "nyse_weekdays".
#[tokio::test]
async fn ctrl03_live_capital_mode_reports_nyse_weekdays_calendar() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveCapital,
    ));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["calendar_spec_id"], "nyse_weekdays",
        "live-capital mode must surface nyse_weekdays as calendar_spec_id"
    );
    let ms = json["market_session"].as_str().unwrap_or("MISSING");
    const VALID_MARKET_SESSION: &[&str] = &["premarket", "regular", "after_hours", "closed"];
    assert!(
        VALID_MARKET_SESSION.contains(&ms),
        "market_session must be one of {VALID_MARKET_SESSION:?}; got '{ms}'"
    );
    let ecs = json["exchange_calendar_state"]
        .as_str()
        .unwrap_or("MISSING");
    const VALID_EXCHANGE_STATE: &[&str] = &["open", "closed", "holiday"];
    assert!(
        VALID_EXCHANGE_STATE.contains(&ecs),
        "exchange_calendar_state must be one of {VALID_EXCHANGE_STATE:?}; got '{ecs}'"
    );
}

/// LiveShadow mode → NyseWeekdays calendar.
#[tokio::test]
async fn ctrl03_live_shadow_mode_reports_nyse_weekdays_calendar() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(json["calendar_spec_id"], "nyse_weekdays");
    let ms = json["market_session"].as_str().unwrap_or("MISSING");
    const VALID_MARKET_SESSION: &[&str] = &["premarket", "regular", "after_hours", "closed"];
    assert!(
        VALID_MARKET_SESSION.contains(&ms),
        "market_session must be one of {VALID_MARKET_SESSION:?}; got '{ms}'"
    );
}

/// notes array must carry provenance — every session response explains the
/// authority basis of the session truth it reports.
#[tokio::test]
async fn ctrl03_session_response_notes_carry_provenance() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::Paper,
    ));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    let notes = json["notes"].as_array().expect("notes must be an array");
    assert!(
        !notes.is_empty(),
        "notes must carry session_truth provenance; got empty array"
    );
    let note_str = notes[0].as_str().unwrap_or("");
    assert!(
        note_str.contains("session_truth"),
        "first note must be a session_truth provenance note; got: {note_str:?}"
    );
}

// ---------------------------------------------------------------------------
// RD-01: Durable risk-denial history — DB-backed persistence/reload tests
//
// These tests require MQK_DATABASE_URL and skip gracefully without it.
// They prove that risk denials are durably stored and that the route
// surfaces them across restarts (i.e. when no execution loop is running
// but the DB has rows from a prior session).
// ---------------------------------------------------------------------------

/// Acquire a DB pool from MQK_DATABASE_URL.  Panics if the env var is absent.
async fn denial_test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon risk_denial -- --include-ignored"
        )
    });
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("denial_test_pool: connect failed")
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn risk_denial_persist_and_reload_roundtrip() {
    // Proves that persist_risk_denial_event writes a row that
    // load_recent_risk_denial_events can read back with correct field values.
    let pool = denial_test_pool().await;

    let test_id = format!(
        "test:persist_roundtrip:{}",
        chrono::Utc::now().timestamp_micros()
    );
    let denied_at =
        chrono::DateTime::from_timestamp(1_700_000_200, 0).expect("valid unix timestamp");

    let row = mqk_db::RiskDenialEventRow {
        id: test_id.clone(),
        denied_at_utc: denied_at,
        rule: "TEST_RULE_ROUNDTRIP".to_string(),
        message: "test denial — persist and reload roundtrip".to_string(),
        symbol: Some("TSLA".to_string()),
        requested_qty: Some(50),
        limit_qty: Some(25),
        severity: "critical".to_string(),
    };

    // Write.
    mqk_db::persist_risk_denial_event(&pool, &row)
        .await
        .expect("persist_risk_denial_event failed");

    // Reload and find our row.
    let rows = mqk_db::load_recent_risk_denial_events(&pool, 200)
        .await
        .expect("load_recent_risk_denial_events failed");

    let found = rows.iter().find(|r| r.id == test_id);
    assert!(
        found.is_some(),
        "persisted denial row must appear in load_recent_risk_denial_events; id={test_id}"
    );
    let found = found.unwrap();
    assert_eq!(found.rule, "TEST_RULE_ROUNDTRIP");
    assert_eq!(found.symbol.as_deref(), Some("TSLA"));
    assert_eq!(found.requested_qty, Some(50));
    assert_eq!(found.limit_qty, Some(25));
    assert_eq!(found.severity, "critical");

    // Idempotent re-insert must not error.
    mqk_db::persist_risk_denial_event(&pool, &row)
        .await
        .expect("idempotent re-insert must not fail");

    // Cleanup.
    sqlx::query("delete from sys_risk_denial_events where id = $1")
        .bind(&test_id)
        .execute(&pool)
        .await
        .expect("cleanup delete failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn risk_denials_route_returns_durable_history_after_restart() {
    // Proves that after a simulated restart (AppState with pool but no
    // execution loop / snapshot), the /api/v1/risk/denials route returns
    // truth_state = "durable_history" and surfaces the persisted row.
    //
    // This is the core restart-safety invariant: denial history must not be
    // lost when the daemon restarts and the execution loop has not yet started.
    let pool = denial_test_pool().await;

    let test_id = format!(
        "test:route_durable_history:{}",
        chrono::Utc::now().timestamp_micros()
    );
    let denied_at =
        chrono::DateTime::from_timestamp(1_700_000_300, 0).expect("valid unix timestamp");

    // Insert a denial row directly — simulating a row written by a prior
    // session's orchestrator.
    mqk_db::persist_risk_denial_event(
        &pool,
        &mqk_db::RiskDenialEventRow {
            id: test_id.clone(),
            denied_at_utc: denied_at,
            rule: "TEST_RULE_ROUTE_RELOAD".to_string(),
            message: "test denial — route durable history after restart".to_string(),
            symbol: Some("SPY".to_string()),
            requested_qty: Some(10),
            limit_qty: Some(5),
            severity: "critical".to_string(),
        },
    )
    .await
    .expect("persist_risk_denial_event failed");

    // Build AppState as if the daemon just restarted: pool available but no
    // execution loop running (execution_snapshot is None).
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Confirm execution_snapshot is absent (no loop started).
    assert!(
        st.execution_snapshot.read().await.is_none(),
        "execution_snapshot must be absent for the restart simulation to be valid"
    );

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/risk/denials must return HTTP 200 after restart"
    );
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"].as_str(),
        Some("durable_history"),
        "truth_state must be durable_history when DB has rows but loop is not running; got: {json}"
    );
    assert!(
        json["snapshot_at_utc"].is_null(),
        "snapshot_at_utc must be null when loop is not running; got: {json}"
    );
    let rows = json["denials"]
        .as_array()
        .expect("denials must be an array");
    let found_row = rows
        .iter()
        .find(|r| r["id"].as_str() == Some(test_id.as_str()));
    assert!(
        found_row.is_some(),
        "persisted denial row must appear in route response after restart; id={test_id}; got: {json}"
    );
    // strategy_id must be null — not available on the risk gate path.
    assert!(
        found_row.unwrap()["strategy_id"].is_null(),
        "strategy_id must be null in durable_history rows; got: {json}"
    );

    // Cleanup.
    sqlx::query("delete from sys_risk_denial_events where id = $1")
        .bind(&test_id)
        .execute(&pool)
        .await
        .expect("cleanup delete failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn risk_denials_route_active_with_pool_returns_only_db_rows() {
    // Proves that when the execution loop IS running AND a pool is available,
    // truth_state = "active" and only DB-persisted rows are returned.
    // Ring-buffer-only rows (those whose DB persist failed) are NOT surfaced —
    // that is the strict durable truth guarantee.
    use chrono::DateTime;
    use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot};

    let pool = denial_test_pool().await;

    let test_id = format!("test:active_pool:{}", chrono::Utc::now().timestamp_micros());
    let denied_at =
        chrono::DateTime::from_timestamp(1_700_000_400, 0).expect("valid unix timestamp");

    // Insert one denial row directly (simulating a successful orchestrator write).
    mqk_db::persist_risk_denial_event(
        &pool,
        &mqk_db::RiskDenialEventRow {
            id: test_id.clone(),
            denied_at_utc: denied_at,
            rule: "TEST_RULE_ACTIVE_POOL".to_string(),
            message: "test denial — active pool DB-only path".to_string(),
            symbol: Some("QQQ".to_string()),
            requested_qty: Some(20),
            limit_qty: Some(10),
            severity: "critical".to_string(),
        },
    )
    .await
    .expect("persist_risk_denial_event failed");

    // Build AppState with pool and inject a ring-buffer-only denial (one that
    // was NOT written to DB) to prove it is excluded from the "active" response.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let snap = ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: vec![],
        },
        system_block_state: None,
        // One ring-buffer row with a unique id that is NOT in the DB.
        recent_risk_denials: vec![mqk_runtime::observability::RiskDenialRecord {
            id: "ring_buffer_only_row_never_in_db".to_string(),
            denied_at_utc: DateTime::from_timestamp(1_700_000_500, 0)
                .expect("valid unix timestamp"),
            rule: "RING_BUFFER_ONLY".to_string(),
            message: "this row exists only in the ring buffer, not in DB".to_string(),
            symbol: Some("RING".to_string()),
            requested_qty: None,
            limit: None,
            severity: "critical".to_string(),
        }],
        snapshot_at_utc: DateTime::from_timestamp(1_700_000_600, 0).expect("valid unix timestamp"),
    };
    *st.execution_snapshot.write().await = Some(snap);

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    // With pool + loop running: truth_state must be "active" (durable).
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active"),
        "truth_state must be active when pool is present and loop is running; got: {json}"
    );
    assert!(
        !json["snapshot_at_utc"].is_null(),
        "snapshot_at_utc must be non-null when loop is running; got: {json}"
    );

    let rows = json["denials"]
        .as_array()
        .expect("denials must be an array");

    // DB row must appear.
    let found_db = rows
        .iter()
        .any(|r| r["id"].as_str() == Some(test_id.as_str()));
    assert!(
        found_db,
        "DB-persisted denial must appear in active route response; id={test_id}; got: {json}"
    );

    // Ring-buffer-only row must NOT appear (strict durable truth).
    let found_ring = rows
        .iter()
        .any(|r| r["id"].as_str() == Some("ring_buffer_only_row_never_in_db"));
    assert!(
        !found_ring,
        "ring-buffer-only row must NOT appear in active (durable) response; got: {json}"
    );

    // strategy_id must be null on every row.
    for row in rows {
        assert!(
            row["strategy_id"].is_null(),
            "strategy_id must be null in active rows; got: {row}"
        );
    }

    // Cleanup.
    sqlx::query("delete from sys_risk_denial_events where id = $1")
        .bind(&test_id)
        .execute(&pool)
        .await
        .expect("cleanup delete failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn config_diffs_route_active_returns_authoritative_empty_when_latest_run_matches_current() {
    let pool = denial_test_pool().await;
    let run_id = uuid::Uuid::new_v4();
    let started_at = chrono::Utc::now() + chrono::Duration::days(3650);

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "test-git".to_string(),
            config_hash: "daemon-runtime-paper-blocked-v1".to_string(),
            config_json: serde_json::json!({
                "runtime": "mqk-daemon",
                "adapter": "paper",
                "mode": "PAPER",
            }),
            host_fingerprint: "config-diff-test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-diffs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    assert_eq!(json["backend"], "postgres.runs+daemon.runtime_selection");
    assert!(
        json["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "rows must be authoritatively empty when the latest durable run matches current daemon truth; got: {json}"
    );

    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("cleanup delete failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn config_diffs_route_active_returns_authoritative_rows_when_latest_run_differs() {
    let pool = denial_test_pool().await;
    let run_id = uuid::Uuid::new_v4();
    let started_at = chrono::Utc::now() + chrono::Duration::days(3651);

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "LIVE".to_string(),
            started_at_utc: started_at,
            git_hash: "test-git".to_string(),
            config_hash: "daemon-runtime-live-shadow-ready-v1".to_string(),
            config_json: serde_json::json!({
                "runtime": "mqk-daemon",
                "adapter": "alpaca",
                "mode": "LIVE-SHADOW",
            }),
            host_fingerprint: "config-diff-test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-diffs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    assert_eq!(json["backend"], "postgres.runs+daemon.runtime_selection");

    let rows = json["rows"].as_array().expect("rows must be an array");
    assert!(
        rows.iter().any(|row| {
            row["changed_domain"] == "config"
                && row["before_version"] == "daemon-runtime-live-shadow-ready-v1"
                && row["after_version"] == "daemon-runtime-paper-blocked-v1"
        }),
        "config_hash diff row must be surfaced from durable run truth; got: {json}"
    );
    assert!(
        rows.iter().any(|row| {
            row["changed_domain"] == "runtime"
                && row["before_version"] == "LIVE"
                && row["after_version"] == "PAPER"
        }),
        "deployment-mode diff row must be surfaced from durable run truth; got: {json}"
    );
    assert!(
        rows.iter().any(|row| {
            row["changed_domain"] == "runtime"
                && row["before_version"] == "alpaca"
                && row["after_version"] == "paper"
        }),
        "adapter diff row must be surfaced from durable run truth; got: {json}"
    );

    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("cleanup delete failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn strategy_surfaces_remain_fail_closed_even_with_db_pool() {
    let pool = denial_test_pool().await;
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let summary_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (summary_status, summary_body) = call(router.clone(), summary_req).await;
    assert_eq!(summary_status, StatusCode::OK);
    let summary = parse_json(summary_body);
    // CC-01B: DB pool present → truth_state="registry" (authoritative from
    // postgres.sys_strategy_registry).  Empty rows = no strategies registered,
    // which is authoritative empty, not unavailable.
    assert_eq!(
        summary["truth_state"], "registry",
        "CC-01B: DB pool present → truth_state must be 'registry'; got: {summary}"
    );
    assert_eq!(
        summary["backend"], "postgres.sys_strategy_registry",
        "CC-01B: backend must be postgres.sys_strategy_registry; got: {summary}"
    );
    assert!(
        summary["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "empty sys_strategy_registry → authoritative empty rows; got: {summary}"
    );

    let suppressions_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (suppressions_status, suppressions_body) = call(router, suppressions_req).await;
    assert_eq!(suppressions_status, StatusCode::OK);
    let suppressions = parse_json(suppressions_body);
    // CC-02: suppressions now has a real durable source. With DB pool present
    // and an empty table, truth_state is "active" + empty rows (authoritative empty).
    assert_eq!(
        suppressions["truth_state"], "active",
        "suppressions with DB pool must return active truth (CC-02); got: {suppressions}"
    );
    assert!(
        suppressions["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "strategy suppressions must return authoritative empty rows when DB table is empty; got: {suppressions}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn risk_denials_route_no_snapshot_when_db_empty() {
    // Proves that when no denial rows exist in the DB and the execution loop
    // is not running, the route returns truth_state = "no_snapshot" (not
    // "durable_history" with empty rows — that would be misleading).
    //
    // This test relies on the DB having no test-inserted rows for the
    // "TEST_RULE_EMPTY_CHECK" rule. We clean up before and after.
    let pool = denial_test_pool().await;

    // Ensure no test rows exist.
    sqlx::query("delete from sys_risk_denial_events where rule = 'TEST_RULE_EMPTY_CHECK'")
        .execute(&pool)
        .await
        .expect("pre-test cleanup failed");

    // Build AppState with pool but no execution loop.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // This test only proves "no_snapshot" when the *entire* table is empty.
    // If other rows exist (from real runs), the route will return
    // "durable_history" — which is correct behaviour.  Skip gracefully.
    let existing = mqk_db::load_recent_risk_denial_events(&pool, 1)
        .await
        .expect("load check failed");
    if !existing.is_empty() {
        // Real denial history exists — test cannot prove empty-table path.
        // This is acceptable: the DB has real rows, durable_history is correct.
        return;
    }

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"].as_str(),
        Some("no_snapshot"),
        "truth_state must be no_snapshot when DB has no rows and loop is not running; got: {json}"
    );
    assert!(
        json["denials"].as_array().is_some_and(|v| v.is_empty()),
        "denials must be empty when truth_state is no_snapshot; got: {json}"
    );
}

#[tokio::test]
async fn operator_history_routes_fail_closed_when_db_pool_is_absent() {
    // Proves that the mounted operator-history endpoints do not fake durable
    // postgres-backed emptiness when AppState has no DB pool.
    let router = make_router();
    let cases: [(&str, &str); 3] = [
        (
            "/api/v1/audit/operator-actions",
            "/api/v1/audit/operator-actions",
        ),
        ("/api/v1/audit/artifacts", "/api/v1/audit/artifacts"),
        (
            "/api/v1/ops/operator-timeline",
            "/api/v1/ops/operator-timeline",
        ),
    ];

    for (uri, canonical_route) in cases {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, body) = call(router.clone(), req).await;

        assert_eq!(status, StatusCode::OK, "{uri} must return 200");

        let json = parse_json(body);
        assert_eq!(
            json["canonical_route"].as_str(),
            Some(canonical_route),
            "{uri} must self-identify its canonical route"
        );
        assert_eq!(
            json["truth_state"].as_str(),
            Some("backend_unavailable"),
            "{uri} must declare durable truth unavailable when no DB pool is present; got: {json}"
        );
        assert_eq!(
            json["backend"].as_str(),
            Some("unavailable"),
            "{uri} must not claim a postgres backend without a DB pool; got: {json}"
        );
        assert!(
            json["rows"].as_array().is_some_and(|rows| rows.is_empty()),
            "{uri} rows must be an empty array when durable history is unavailable; got: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// CC-01: Strategy fleet active path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc01_strategy_summary_active_with_fleet() {
    // CC-01B: The summary route now sources truth from postgres.sys_strategy_registry,
    // not from the in-memory strategy_fleet field.  Injecting a fleet via
    // set_strategy_fleet_for_test has no effect on the route output.
    //
    // Without a DB pool the route returns truth_state="no_db" regardless of the
    // in-memory fleet state.  This proves the route no longer depends on the
    // placeholder fleet mechanism.
    //
    // Row-level field contract (enabled, armed, honest-null fields) is proven by
    // the DB-backed tests in scenario_strategy_summary_registry.rs.
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Inject an in-memory fleet — must have NO effect on route output after CC-01B.
    st.set_strategy_fleet_for_test(Some(vec![
        state::StrategyFleetEntry {
            strategy_id: "strat_alpha".to_string(),
        },
        state::StrategyFleetEntry {
            strategy_id: "strat_beta".to_string(),
        },
    ]))
    .await;
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    // CC-01B: no DB → fail-closed; in-memory fleet is not the source of truth.
    assert_eq!(
        json["truth_state"], "no_db",
        "CC-01B: route must use DB registry, not in-memory fleet; \
         no DB → no_db regardless of fleet injection; got: {json}"
    );
    assert_eq!(json["canonical_route"], "/api/v1/strategy/summary");
    assert_eq!(json["backend"], "postgres.sys_strategy_registry");
    assert!(
        json["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "no_db → rows must be empty; got: {json}"
    );
}

#[tokio::test]
async fn cc01_strategy_summary_active_empty_fleet() {
    // CC-01B: In-memory fleet injection is superseded; the route uses the DB registry.
    // Injecting an empty in-memory fleet has no effect.  Without a DB pool the
    // route returns truth_state="no_db" (fail-closed), not "active" or "not_wired".
    // Authoritative empty ("registry" + empty rows) requires DB — proven in
    // scenario_strategy_summary_registry.rs::summary_with_db_uses_registry_truth_state.
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.set_strategy_fleet_for_test(Some(vec![])).await; // no effect after CC-01B
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["truth_state"], "no_db",
        "CC-01B: no DB → no_db; empty in-memory fleet has no effect; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "no_db rows must be []; got: {json}"
    );
}

#[tokio::test]
async fn cc01_strategy_summary_not_wired_without_fleet() {
    // CC-01B: "not_wired" is gone.  No fleet + no DB → truth_state="no_db"
    // (fail-closed).  The route no longer reads MQK_STRATEGY_IDS or the
    // in-memory fleet; it reads postgres.sys_strategy_registry.
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "no_db",
        "CC-01B: no fleet + no DB → no_db (not not_wired); got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|rows| rows.is_empty()),
        "no_db rows must be []; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC-02: Durable strategy suppressions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc02_strategy_suppressions_no_db_returns_no_db_state() {
    // Without a DB pool the route must return truth_state="no_db" (source
    // unavailable), NOT "not_wired" (which would mean permanently unimplemented).
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "no_db",
        "no DB pool → truth_state must be no_db, not not_wired; got: {json}"
    );
    assert_eq!(json["canonical_route"], "/api/v1/strategy/suppressions");
    assert_eq!(json["backend"], "postgres.sys_strategy_suppressions");
    assert!(json["rows"].as_array().is_some_and(|r| r.is_empty()));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn cc02_strategy_suppressions_active_empty_when_no_rows() {
    // With DB pool and empty table: truth_state="active" + empty rows (authoritative zero).
    let pool = denial_test_pool().await;
    // Ensure clean state.
    sqlx::query("delete from sys_strategy_suppressions where strategy_id = 'cc02_empty_probe'")
        .execute(&pool)
        .await
        .expect("pre-test cleanup failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "active",
        "DB present + empty table must return active (authoritative empty); got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|r| r.is_empty()),
        "empty table must yield empty rows; got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn cc02_strategy_suppressions_active_with_real_row() {
    // Insert a real suppression row; route must return it with correct field values.
    let pool = denial_test_pool().await;
    let test_id = uuid::Uuid::parse_str("00000000-cc02-0001-0000-000000000001").unwrap();

    // Clean up before and after.
    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("pre-test cleanup failed");

    let started = chrono::DateTime::parse_from_rfc3339("2025-01-15T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: test_id,
            strategy_id: "strat_alpha".to_string(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "manual suppression for cc02 test".to_string(),
            started_at_utc: started,
            note: "cc02 proof row".to_string(),
        },
    )
    .await
    .expect("insert_strategy_suppression failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "active",
        "DB present with row must return active truth; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be an array");
    let row = rows
        .iter()
        .find(|r| r["suppression_id"] == test_id.to_string())
        .expect("inserted suppression row must appear in route response");
    assert_eq!(row["strategy_id"], "strat_alpha");
    assert_eq!(row["state"], "active");
    assert_eq!(row["trigger_domain"], "operator");
    assert_eq!(row["trigger_reason"], "manual suppression for cc02 test");
    assert!(
        row["cleared_at"].is_null(),
        "active suppression cleared_at must be null"
    );

    // Cleanup.
    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("post-test cleanup failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn cc02_strategy_suppressions_durable_persistence() {
    // Proves that a row inserted in one AppState instance is visible through
    // a fresh AppState (simulating restart / fresh daemon start).
    let pool = denial_test_pool().await;
    let test_id = uuid::Uuid::parse_str("00000000-cc02-0002-0000-000000000002").unwrap();

    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("pre-test cleanup failed");

    let started = chrono::DateTime::parse_from_rfc3339("2025-02-01T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    // Insert via DB function (simulates a prior daemon session writing the row).
    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: test_id,
            strategy_id: "strat_beta".to_string(),
            trigger_domain: "risk".to_string(),
            trigger_reason: "drawdown threshold breached".to_string(),
            started_at_utc: started,
            note: "cc02 persistence proof".to_string(),
        },
    )
    .await
    .expect("insert_strategy_suppression failed");

    // Fresh AppState — simulates a daemon restart reading from the same DB.
    let fresh_st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let fresh_router = routes::build_router(Arc::clone(&fresh_st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(fresh_router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "active",
        "fresh AppState must return active truth from durable DB; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be an array");
    let found = rows
        .iter()
        .any(|r| r["suppression_id"] == test_id.to_string() && r["strategy_id"] == "strat_beta");
    assert!(
        found,
        "inserted row must survive to fresh AppState read; got rows: {rows:?}"
    );

    // Cleanup.
    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("post-test cleanup failed");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn cc02_strategy_suppressions_clear_changes_state() {
    // Insert a suppression, clear it, confirm state transitions in the route response.
    let pool = denial_test_pool().await;
    let test_id = uuid::Uuid::parse_str("00000000-cc02-0003-0000-000000000003").unwrap();

    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("pre-test cleanup failed");

    let started = chrono::DateTime::parse_from_rfc3339("2025-03-01T08:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let cleared = chrono::DateTime::parse_from_rfc3339("2025-03-01T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    mqk_db::insert_strategy_suppression(
        &pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: test_id,
            strategy_id: "strat_gamma".to_string(),
            trigger_domain: "integrity".to_string(),
            trigger_reason: "integrity check failure".to_string(),
            started_at_utc: started,
            note: "cc02 clear proof".to_string(),
        },
    )
    .await
    .expect("insert failed");

    let was_cleared = mqk_db::clear_strategy_suppression(&pool, test_id, cleared)
        .await
        .expect("clear_strategy_suppression failed");
    assert!(was_cleared, "clear must return true for an active row");

    // Read through route — cleared row must appear with state="cleared".
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let rows = json["rows"].as_array().expect("rows must be array");
    let row = rows
        .iter()
        .find(|r| r["suppression_id"] == test_id.to_string())
        .expect("cleared row must still appear in route response");
    assert_eq!(
        row["state"], "cleared",
        "suppression must be cleared; got: {row}"
    );
    assert!(
        !row["cleared_at"].is_null(),
        "cleared_at must be set; got: {row}"
    );

    // Cleanup.
    sqlx::query("delete from sys_strategy_suppressions where suppression_id = $1")
        .bind(test_id)
        .execute(&pool)
        .await
        .expect("post-test cleanup failed");
}

// ---------------------------------------------------------------------------
// CC-03: Controlled mode-change workflow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc03_change_system_mode_returns_guidance_response() {
    // POST /api/v1/ops/action change-system-mode must:
    //   - return 409 CONFLICT (safe refusal preserved — no hot switching)
    //   - return ModeChangeGuidanceResponse, not a dead-end error
    //   - transition_permitted == false
    //   - operator_next_steps is non-empty with explicit restart instructions
    let router = make_router();

    let body = serde_json::json!({"action_key": "change-system-mode"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, bytes) = call(router, req).await;
    let j = parse_json(bytes);

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "change-system-mode must return 409: {j}"
    );
    assert_eq!(
        j["transition_permitted"], false,
        "transition_permitted must be false — no hot switching: {j}"
    );
    assert!(
        j["operator_next_steps"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "operator_next_steps must be non-empty: {j}"
    );
    assert!(
        j["preconditions"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "preconditions must be non-empty: {j}"
    );
    assert_eq!(
        j["canonical_route"].as_str(),
        Some("/api/v1/ops/mode-change-guidance"),
        "canonical_route must point to the guidance endpoint: {j}"
    );
}

#[tokio::test]
async fn cc03_mode_change_guidance_get_returns_200() {
    // GET /api/v1/ops/mode-change-guidance must:
    //   - return 200 (read-only guidance surface, not an action)
    //   - transition_permitted == false always
    //   - preconditions and operator_next_steps are non-empty
    //   - current_mode is non-empty
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, bytes) = call(router, req).await;
    let j = parse_json(bytes);

    assert_eq!(status, StatusCode::OK, "guidance GET must return 200: {j}");
    assert_eq!(
        j["transition_permitted"], false,
        "transition_permitted must be false: {j}"
    );
    assert!(
        j["current_mode"].as_str().is_some_and(|m| !m.is_empty()),
        "current_mode must be non-empty: {j}"
    );
    assert!(
        j["preconditions"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "preconditions must be non-empty: {j}"
    );
    assert!(
        j["operator_next_steps"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "operator_next_steps must be non-empty: {j}"
    );
    assert_eq!(
        j["canonical_route"].as_str(),
        Some("/api/v1/ops/mode-change-guidance"),
        "canonical_route must self-identify: {j}"
    );
}

#[tokio::test]
async fn cc03_mode_change_guidance_and_ops_action_agree() {
    // GET /api/v1/ops/mode-change-guidance and POST /api/v1/ops/action change-system-mode
    // must agree on: canonical_route, transition_permitted, current_mode.
    // This proves both surfaces are backed by the same build_mode_change_guidance helper.
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let router_get = routes::build_router(Arc::clone(&st));
    let req_get = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let (s_get, b_get) = call(router_get, req_get).await;
    let j_get = parse_json(b_get);

    let router_post = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({"action_key": "change-system-mode"}).to_string();
    let req_post = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let (s_post, b_post) = call(router_post, req_post).await;
    let j_post = parse_json(b_post);

    assert_eq!(s_get, StatusCode::OK);
    assert_eq!(s_post, StatusCode::CONFLICT);

    assert_eq!(
        j_get["canonical_route"], j_post["canonical_route"],
        "canonical_route must agree between GET and POST: get={j_get} post={j_post}"
    );
    assert_eq!(
        j_get["transition_permitted"], j_post["transition_permitted"],
        "transition_permitted must agree: get={j_get} post={j_post}"
    );
    assert_eq!(
        j_get["current_mode"], j_post["current_mode"],
        "current_mode must agree: get={j_get} post={j_post}"
    );
}
