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

// ---------------------------------------------------------------------------
// POST /v1/run/start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_requires_db_backed_runtime_after_arm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
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
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
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
}

#[tokio::test]
async fn run_start_requires_db_after_rearm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
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
async fn trading_account_returns_200_and_has_snapshot_false_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/account")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["has_snapshot"], false);
    assert_eq!(json["account"]["currency"], "USD");
}

#[tokio::test]
async fn trading_positions_returns_empty_vec_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/positions")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["has_snapshot"], false);
    assert!(json["positions"].is_array());
    assert_eq!(json["positions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn trading_orders_returns_empty_vec_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/orders")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["has_snapshot"], false);
    assert!(json["orders"].is_array());
    assert_eq!(json["orders"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn trading_fills_returns_empty_vec_by_default() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/trading/fills")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["has_snapshot"], false);
    assert!(json["fills"].is_array());
    assert_eq!(json["fills"].as_array().unwrap().len(), 0);
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
    std::env::remove_var("MQK_DEV_ALLOW_SNAPSHOT_INJECT");

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
    assert_eq!(json["environment"], "paper");
    assert_eq!(json["runtime_status"], "idle");
    assert_eq!(json["integrity_status"], "warning");
    assert_eq!(json["daemon_reachable"], true);
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
    assert_eq!(json["db_reachable"], false);
    assert_eq!(json["execution_disarmed"], true);
    assert!(json["blockers"].as_array().unwrap().len() >= 4);
}

#[tokio::test]
async fn api_execution_summary_derives_counts_from_broker_snapshot() {
    use chrono::{Duration, Utc};
    use mqk_schemas::{BrokerAccount, BrokerOrder, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    {
        let mut lock = st.broker_snapshot.write().await;
        *lock = Some(BrokerSnapshot {
            captured_at_utc: Utc::now(),
            account: BrokerAccount {
                equity: "1000".to_string(),
                cash: "400".to_string(),
                currency: "USD".to_string(),
            },
            positions: Vec::new(),
            fills: Vec::new(),
            orders: vec![
                BrokerOrder {
                    broker_order_id: "bo-1".to_string(),
                    client_order_id: "io-1".to_string(),
                    symbol: "AAPL".to_string(),
                    side: "buy".to_string(),
                    r#type: "limit".to_string(),
                    status: "new".to_string(),
                    qty: "10".to_string(),
                    limit_price: Some("100".to_string()),
                    stop_price: None,
                    created_at_utc: Utc::now() - Duration::minutes(6),
                },
                BrokerOrder {
                    broker_order_id: "bo-2".to_string(),
                    client_order_id: "io-2".to_string(),
                    symbol: "MSFT".to_string(),
                    side: "sell".to_string(),
                    r#type: "market".to_string(),
                    status: "submitted".to_string(),
                    qty: "5".to_string(),
                    limit_price: None,
                    stop_price: None,
                    created_at_utc: Utc::now(),
                },
                BrokerOrder {
                    broker_order_id: "bo-3".to_string(),
                    client_order_id: "io-3".to_string(),
                    symbol: "NVDA".to_string(),
                    side: "buy".to_string(),
                    r#type: "market".to_string(),
                    status: "rejected".to_string(),
                    qty: "1".to_string(),
                    limit_price: None,
                    stop_price: None,
                    created_at_utc: Utc::now(),
                },
            ],
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
    assert_eq!(json["active_orders"], 2);
    assert_eq!(json["pending_orders"], 1);
    assert_eq!(json["dispatching_orders"], 1);
    assert_eq!(json["reject_count_today"], 1);
    assert_eq!(json["stuck_orders"], 1);
    assert!(json["avg_ack_latency_ms"].is_null());
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
async fn api_alerts_active_reports_truthful_state_driven_alerts() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    let alerts = json.as_array().expect("alerts array");
    assert!(!alerts.is_empty());
    assert!(alerts
        .iter()
        .any(|a| a["id"] == "integrity-disarmed" && a["severity"] == "warning"));
    assert!(alerts
        .iter()
        .any(|a| a["id"] == "reconcile-unknown" && a["severity"] == "info"));
}

#[tokio::test]
async fn api_events_feed_exposes_current_status_event() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    let feed = json.as_array().expect("feed array");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0]["id"], "daemon-status");
    assert_eq!(feed[0]["source"], "mqk-daemon");
    assert_eq!(feed[0]["severity"], "warning");
    assert!(feed[0]["text"]
        .as_str()
        .unwrap_or("")
        .contains("integrity_armed=false"));
}

#[tokio::test]
async fn api_audit_metadata_runtime_and_artifact_surfaces_are_operator_honest() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let audit_req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (audit_status, audit_body) = call(routes::build_router(Arc::clone(&st)), audit_req).await;
    assert_eq!(audit_status, StatusCode::OK);
    assert_eq!(parse_json(audit_body), serde_json::json!([]));

    let metadata_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/metadata")
        .body(axum::body::Body::empty())
        .unwrap();
    let (metadata_status, metadata_body) =
        call(routes::build_router(Arc::clone(&st)), metadata_req).await;
    assert_eq!(metadata_status, StatusCode::OK);
    let metadata = parse_json(metadata_body);
    assert_eq!(metadata["api_version"], "v1");
    assert_eq!(metadata["broker_adapter"], "paper");
    assert_eq!(metadata["endpoint_status"], "ok");

    let leadership_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/runtime-leadership")
        .body(axum::body::Body::empty())
        .unwrap();
    let (leadership_status, leadership_body) =
        call(routes::build_router(Arc::clone(&st)), leadership_req).await;
    assert_eq!(leadership_status, StatusCode::OK);
    let leadership = parse_json(leadership_body);
    assert_eq!(leadership["leader_lease_state"], "lost");
    assert_eq!(leadership["generation_id"], "unknown");
    assert_eq!(leadership["post_restart_recovery_state"], "unknown");
    assert_eq!(leadership["recovery_checkpoint"], "unavailable");
    assert_eq!(leadership["checkpoints"], serde_json::json!([]));

    let artifacts_req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/artifacts")
        .body(axum::body::Body::empty())
        .unwrap();
    let (artifacts_status, artifacts_body) =
        call(routes::build_router(Arc::clone(&st)), artifacts_req).await;
    assert_eq!(artifacts_status, StatusCode::OK);
    let artifacts = parse_json(artifacts_body);
    assert!(artifacts["last_updated_at"].is_null());
    assert_eq!(artifacts["ready_count"], 0);
    assert_eq!(artifacts["pending_count"], 0);
    assert_eq!(artifacts["failed_count"], 0);
    assert_eq!(artifacts["artifacts"], serde_json::json!([]));
}
