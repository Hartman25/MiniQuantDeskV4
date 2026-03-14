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

fn json_str<'a>(json: &'a serde_json::Value, key: &str) -> &'a str {
    json.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("missing string key '{key}' in response: {json}"))
}

fn has_unavailable_marker(value: &str) -> bool {
    matches!(value, "unknown" | "unavailable" | "not_configured")
}

#[tokio::test]
async fn gui_contract_canonical_api_surfaces_have_expected_shape() {
    let router = make_router();

    let cases: [(&str, &[&str]); 4] = [
        (
            "/api/v1/execution/summary",
            &[
                "active_orders",
                "pending_orders",
                "dispatching_orders",
                "reject_count_today",
            ],
        ),
        (
            "/api/v1/portfolio/summary",
            &[
                "account_equity",
                "cash",
                "long_market_value",
                "buying_power",
            ],
        ),
        (
            "/api/v1/risk/summary",
            &[
                "gross_exposure",
                "net_exposure",
                "concentration_pct",
                "kill_switch_active",
            ],
        ),
        (
            "/api/v1/reconcile/status",
            &[
                "status",
                "last_run_at",
                "mismatched_positions",
                "unmatched_broker_events",
            ],
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
async fn gui_01_system_status_contract_requires_semantic_truth() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["daemon_reachable"], true);
    assert_eq!(json_str(&json, "runtime_status"), "idle");
    assert_eq!(json_str(&json, "integrity_status"), "warning");
    assert_eq!(json["strategy_armed"], false);
    assert_eq!(json["execution_armed"], false);

    let db_status = json_str(&json, "db_status");
    assert!(
        has_unavailable_marker(db_status),
        "db_status must be explicit unavailable truth, got: {db_status}"
    );

    let audit_writer_status = json_str(&json, "audit_writer_status");
    assert!(
        has_unavailable_marker(audit_writer_status),
        "audit_writer_status must be explicit unavailable truth, got: {audit_writer_status}"
    );

    assert!(
        json_str(&json, "runtime_status") != "unknown",
        "runtime_status must not regress to placeholder unknown"
    );
}

#[tokio::test]
async fn gui_02_system_preflight_contract_is_semantic_and_fail_closed() {
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
    assert_eq!(json["strategy_disarmed"], true);
    assert_eq!(json["execution_disarmed"], true);
    assert_eq!(json["live_routing_disabled"], true);
    assert_eq!(json["db_reachable"], serde_json::Value::Null);
    assert_eq!(json["broker_config_present"], serde_json::Value::Null);
    assert_eq!(json["market_data_config_present"], serde_json::Value::Null);
    assert_eq!(json["audit_writer_ready"], serde_json::Value::Null);

    let warnings = json["warnings"]
        .as_array()
        .expect("warnings must be an array");
    let blockers = json["blockers"]
        .as_array()
        .expect("blockers must be an array");

    assert!(
        !warnings.is_empty(),
        "preflight must explain unavailable wiring instead of silently succeeding"
    );
    assert!(
        blockers.len() >= 5,
        "preflight must fail closed with explicit blockers when critical readiness truth is unavailable"
    );
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str() == Some("Execution is disarmed at the integrity gate.")),
        "preflight blockers must include integrity disarm gate"
    );
}

#[tokio::test]
async fn gui_03_session_config_and_strategy_surfaces_are_semantically_truthful() {
    let router = make_router();

    let session_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (session_status, session_body) = call(router.clone(), session_req).await;
    assert_eq!(session_status, StatusCode::OK);
    let session = parse_json(session_body);
    assert_eq!(session["daemon_mode"], "PAPER");
    assert_eq!(session["strategy_allowed"], false);
    assert_eq!(session["execution_allowed"], false);
    assert_eq!(session["system_trading_window"], "disabled");
    assert_eq!(session["market_session"], "unknown");
    assert_eq!(session["exchange_calendar_state"], "unknown");
    assert!(
        session["notes"]
            .as_array()
            .expect("session notes must be an array")
            .iter()
            .any(|n| n.as_str().is_some_and(|s| s.contains("unavailable"))),
        "session must explicitly disclose unavailable calendar/session truth"
    );

    let config_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-fingerprint")
        .body(axum::body::Body::empty())
        .unwrap();
    let (config_status, config_body) = call(router.clone(), config_req).await;
    assert_eq!(config_status, StatusCode::OK);
    let config = parse_json(config_body);
    assert_eq!(config["environment_profile"], "paper");
    assert_eq!(config["config_hash"], "unknown");
    assert_eq!(config["runtime_generation_id"], "unknown");
    assert_eq!(config["risk_policy_version"], "unknown");
    assert_eq!(config["strategy_bundle_version"], "unknown");
    assert!(config["build_version"].is_string());

    let diffs_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-diffs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (diff_status, diff_body) = call(router.clone(), diffs_req).await;
    assert_eq!(diff_status, StatusCode::OK);
    assert_eq!(parse_json(diff_body), serde_json::json!([]));

    let strategy_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (strategy_status, strategy_body) = call(router.clone(), strategy_req).await;
    assert_eq!(strategy_status, StatusCode::OK);
    let strategy_rows = parse_json(strategy_body);
    let rows = strategy_rows.as_array().expect("strategy summary must be array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["strategy_id"], "daemon_integrity_gate");
    assert_eq!(rows[0]["armed"], false);
    assert_eq!(rows[0]["health"], "warning");

    let suppressions_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (suppressions_status, suppressions_body) = call(router, suppressions_req).await;
    assert_eq!(suppressions_status, StatusCode::OK);
    assert_eq!(parse_json(suppressions_body), serde_json::json!([]));
}

#[tokio::test]
async fn gui_04_operator_actions_audit_surface_is_not_placeholder_truth() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "until durable audit rows are wired, the daemon must fail explicitly (404) rather than serving placeholder operator-actions data"
    );
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
