//! Stable daemon/GUI contract gate tests used by CI (TEST-02R).
//!
//! These assertions intentionally focus on the endpoint surfaces and response
//! shape the GUI depends on most directly.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

#[tokio::test]
async fn gui_contract_canonical_api_surfaces_have_expected_shape() {
    let router = make_router();

    let cases: [(&str, &[&str]); 6] = [
        (
            "/api/v1/system/status",
            &["environment", "runtime_status", "integrity_status", "daemon_reachable"],
        ),
        (
            "/api/v1/system/preflight",
            &["daemon_reachable", "db_reachable", "execution_disarmed", "blockers"],
        ),
        (
            "/api/v1/execution/summary",
            &["active_orders", "pending_orders", "dispatching_orders", "reject_count_today"],
        ),
        (
            "/api/v1/portfolio/summary",
            &["account_equity", "cash", "long_market_value", "buying_power"],
        ),
        (
            "/api/v1/risk/summary",
            &["gross_exposure", "net_exposure", "concentration_pct", "kill_switch_active"],
        ),
        (
            "/api/v1/reconcile/status",
            &["status", "last_run_at", "mismatched_positions", "unmatched_broker_events"],
        ),
    ];

    for (uri, required_keys) in cases {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();

        let (status, body) = call(router.clone(), req).await;
        assert_eq!(status, StatusCode::OK, "{uri} must return 200");

        let json = parse_json(body);
        for key in required_keys {
            assert!(
                json.get(key).is_some(),
                "{uri} missing required key '{key}' in response: {json}"
            );
        }
    }
}

#[tokio::test]
async fn gui_contract_legacy_api_surfaces_have_expected_shape() {
    let router = make_router();

    let health_req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let (health_status, health_body) = call(router.clone(), health_req).await;
    assert_eq!(health_status, StatusCode::OK);
    let health_json = parse_json(health_body);
    assert!(health_json.get("ok").is_some());
    assert!(health_json.get("service").is_some());

    let status_req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_status, status_body) = call(router.clone(), status_req).await;
    assert_eq!(status_status, StatusCode::OK);
    let status_json = parse_json(status_body);
    assert!(status_json.get("state").is_some());
    assert!(status_json.get("active_run_id").is_some());
    assert!(status_json.get("integrity_armed").is_some());

    let account_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/account")
        .body(axum::body::Body::empty())
        .unwrap();
    let (account_status, account_body) = call(router.clone(), account_req).await;
    assert_eq!(account_status, StatusCode::OK);
    let account_json = parse_json(account_body);
    assert!(account_json.get("snapshot_state").is_some());
    assert!(account_json.get("snapshot_captured_at_utc").is_some());
    assert!(account_json.get("account").is_some());
    assert!(
        account_json.get("has_snapshot").is_none(),
        "stale has_snapshot flag must not exist on accepted DMON-04 account contract"
    );

    let positions_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/positions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (positions_status, positions_body) = call(router.clone(), positions_req).await;
    assert_eq!(positions_status, StatusCode::OK);
    let positions_json = parse_json(positions_body);
    assert!(positions_json.get("snapshot_state").is_some());
    assert!(positions_json.get("snapshot_captured_at_utc").is_some());
    assert!(positions_json.get("positions").is_some());
    assert!(
        positions_json.get("has_snapshot").is_none(),
        "stale has_snapshot flag must not exist on accepted DMON-04 positions contract"
    );

    let orders_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/orders")
        .body(axum::body::Body::empty())
        .unwrap();
    let (orders_status, orders_body) = call(router.clone(), orders_req).await;
    assert_eq!(orders_status, StatusCode::OK);
    let orders_json = parse_json(orders_body);
    assert!(orders_json.get("snapshot_state").is_some());
    assert!(orders_json.get("snapshot_captured_at_utc").is_some());
    assert!(orders_json.get("orders").is_some());
    assert!(
        orders_json.get("has_snapshot").is_none(),
        "stale has_snapshot flag must not exist on accepted DMON-04 orders contract"
    );

    let fills_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/fills")
        .body(axum::body::Body::empty())
        .unwrap();
    let (fills_status, fills_body) = call(router, fills_req).await;
    assert_eq!(fills_status, StatusCode::OK);
    let fills_json = parse_json(fills_body);
    assert!(fills_json.get("snapshot_state").is_some());
    assert!(fills_json.get("snapshot_captured_at_utc").is_some());
    assert!(fills_json.get("fills").is_some());
    assert!(
        fills_json.get("has_snapshot").is_none(),
        "stale has_snapshot flag must not exist on accepted DMON-04 fills contract"
    );
}
