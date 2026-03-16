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

#[tokio::test]
async fn gui_contract_canonical_api_surfaces_have_expected_shape() {
    let router = make_router();

    let cases: [(&str, &[&str]); 11] = [
        (
            "/api/v1/system/status",
            &[
                "environment",
                "runtime_status",
                "integrity_status",
                "daemon_reachable",
            ],
        ),
        (
            "/api/v1/system/preflight",
            &[
                "daemon_reachable",
                "db_reachable",
                "execution_disarmed",
                "blockers",
            ],
        ),
        (
            "/api/v1/system/metadata",
            &[
                "build_version",
                "api_version",
                "broker_adapter",
                "endpoint_status",
            ],
        ),
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
        (
            "/api/v1/audit/operator-actions",
            &["canonical_route", "backend", "rows"],
        ),
        (
            "/api/v1/audit/artifacts",
            &["canonical_route", "backend", "rows"],
        ),
        (
            "/api/v1/ops/operator-timeline",
            &["canonical_route", "backend", "rows"],
        ),
        (
            "/api/v1/system/runtime-leadership",
            &[
                "leader_node",
                "leader_lease_state",
                "generation_id",
                "post_restart_recovery_state",
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

        if uri == "/api/v1/audit/operator-actions"
            || uri == "/api/v1/audit/artifacts"
            || uri == "/api/v1/ops/operator-timeline"
        {
            assert_eq!(json["rows"].as_array().map(|v| v.is_empty()), Some(true));
            assert_eq!(
                json["canonical_route"].as_str(),
                Some(uri),
                "{uri} must declare canonical route identity"
            );
            assert!(
                json["backend"]
                    .as_str()
                    .is_some_and(|v| v.contains("postgres")),
                "{uri} must expose durable backend source"
            );
        }

        if uri == "/api/v1/system/metadata" {
            // api_version must be exactly "v1".
            assert_eq!(json_str(&json, "api_version"), "v1");
            // paper adapter in test state.
            assert_eq!(json_str(&json, "broker_adapter"), "paper");
            assert_eq!(json_str(&json, "adapter_id"), "paper");
            // disarmed in test state → endpoint_status must be "warning".
            assert_eq!(json_str(&json, "endpoint_status"), "warning");
            // build_version must be a non-empty string.
            assert!(
                json["build_version"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty()),
                "/api/v1/system/metadata build_version must be a non-empty string"
            );
        }

        if uri == "/api/v1/system/runtime-leadership" {
            // Single-node daemon always reports "local".
            assert_eq!(json_str(&json, "leader_node"), "local");
            // No active run in test state → idle → lease is "lost".
            assert_eq!(json_str(&json, "leader_lease_state"), "lost");
            // generation_id must be a non-empty string (synthetic fallback is fine).
            assert!(
                json["generation_id"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty()),
                "/api/v1/system/runtime-leadership generation_id must be non-empty"
            );
            // No DB pool in test state → restart_count_24h must be 0.
            assert_eq!(json["restart_count_24h"].as_u64(), Some(0));
            // No run history in test state → last_restart_at must be null.
            assert_eq!(json["last_restart_at"], serde_json::Value::Null);
            // Reconcile status "unknown" in test state → "in_progress".
            assert_eq!(
                json_str(&json, "post_restart_recovery_state"),
                "in_progress"
            );
            // No DB → checkpoints must be an empty array.
            assert_eq!(
                json["checkpoints"].as_array().map(|v| v.is_empty()),
                Some(true),
                "/api/v1/system/runtime-leadership checkpoints must be empty in test state"
            );
        }
    }

    let legacy_timeline_req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (legacy_timeline_status, _) = call(router, legacy_timeline_req).await;
    assert_eq!(
        legacy_timeline_status,
        StatusCode::NOT_FOUND,
        "legacy /api/v1/audit/operator-timeline alias must stay unmounted; canonical path is /api/v1/ops/operator-timeline"
    );
}

#[tokio::test]
async fn gui_system_status_and_preflight_surfaces_are_semantically_truthful() {
    let router = make_router();

    let status_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, status_body) = call(router.clone(), status_req).await;
    assert_eq!(status_code, StatusCode::OK);
    let status = parse_json(status_body);
    assert_eq!(status["daemon_reachable"], true);
    assert_eq!(json_str(&status, "runtime_status"), "idle");
    assert_eq!(json_str(&status, "integrity_status"), "warning");
    assert_eq!(status["strategy_armed"], false);
    assert_eq!(status["execution_armed"], false);
    assert_eq!(json_str(&status, "daemon_mode"), "paper");
    assert_eq!(json_str(&status, "adapter_id"), "paper");
    assert_eq!(status["deployment_start_allowed"], true);
    assert!(status["deployment_blocker"].is_null());
    // No DB pool in test state → db_status must be "unavailable" (not "unknown").
    // "unknown" = unchecked; "unavailable" = checked and confirmed no pool.
    assert_eq!(json_str(&status, "db_status"), "unavailable");
    assert_eq!(json_str(&status, "audit_writer_status"), "unavailable");
    // No market data subsystem wired → must be "not_configured", not "unknown".
    // "unknown" = we didn't even check; "not_configured" = explicitly absent.
    assert_eq!(json_str(&status, "market_data_health"), "not_configured");

    let preflight_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let (preflight_code, preflight_body) = call(router, preflight_req).await;
    assert_eq!(preflight_code, StatusCode::OK);
    let preflight = parse_json(preflight_body);
    assert_eq!(preflight["daemon_reachable"], true);
    assert_eq!(json_str(&preflight, "daemon_mode"), "paper");
    assert_eq!(json_str(&preflight, "adapter_id"), "paper");
    assert_eq!(preflight["deployment_start_allowed"], true);
    assert_eq!(preflight["strategy_disarmed"], true);
    assert_eq!(preflight["execution_disarmed"], true);
    assert_eq!(preflight["live_routing_disabled"], true);
    // No DB pool in test state → db_reachable must be null (not a synthetic blocker).
    assert_eq!(preflight["db_reachable"], serde_json::Value::Null);
    // Paper adapter → broker_config_present is Some(false) → JSON false, not null.
    assert_eq!(preflight["broker_config_present"], false);
    // Market data config is genuinely unknown at this level → must stay null.
    assert_eq!(
        preflight["market_data_config_present"],
        serde_json::Value::Null
    );
    // Audit writer proxies DB; no DB pool → null.
    assert_eq!(preflight["audit_writer_ready"], serde_json::Value::Null);

    let blockers: Vec<&str> = preflight["blockers"]
        .as_array()
        .expect("blockers array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    // Synthetic "unavailable from wiring" blockers must be gone.
    assert!(
        !blockers
            .iter()
            .any(|s| s.contains("unavailable from current daemon preflight wiring")),
        "synthetic wiring blockers must not appear in preflight response: {blockers:?}"
    );
    // Real execution-disarmed blocker must still be present.
    assert!(
        blockers
            .iter()
            .any(|s| *s == "Execution is disarmed at the integrity gate."),
        "real execution-disarmed blocker must be present: {blockers:?}"
    );
}

#[tokio::test]
async fn gui_session_config_strategy_and_audit_surfaces_are_semantically_truthful() {
    let router = make_router();

    let session_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap();
    let (session_status, session_body) = call(router.clone(), session_req).await;
    assert_eq!(session_status, StatusCode::OK);
    let session = parse_json(session_body);
    assert_eq!(json_str(&session, "daemon_mode"), "PAPER");
    assert_eq!(json_str(&session, "adapter_id"), "paper");
    assert_eq!(session["deployment_start_allowed"], true);
    assert!(session["deployment_blocker"].is_null());
    assert_eq!(session["strategy_allowed"], false);
    assert_eq!(session["execution_allowed"], false);

    let config_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-fingerprint")
        .body(axum::body::Body::empty())
        .unwrap();
    let (config_status, config_body) = call(router.clone(), config_req).await;
    assert_eq!(config_status, StatusCode::OK);
    let config = parse_json(config_body);
    assert_eq!(json_str(&config, "adapter_id"), "paper");
    assert_eq!(json_str(&config, "environment_profile"), "paper");
    assert_eq!(
        json_str(&config, "config_hash"),
        "daemon-runtime-paper-ready-v1"
    );
    assert!(config["build_version"].is_string());

    let strategy_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (strategy_status, strategy_body) = call(router.clone(), strategy_req).await;
    assert_eq!(strategy_status, StatusCode::OK);
    let strategy_rows = parse_json(strategy_body);
    let rows = strategy_rows.as_array().expect("strategy summary array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["strategy_id"], "daemon_integrity_gate");
    assert_eq!(rows[0]["armed"], false);
    assert_eq!(rows[0]["health"], "warning");

    let audit_req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (audit_status, audit_body) = call(router, audit_req).await;
    assert_eq!(audit_status, StatusCode::OK);
    let audit = parse_json(audit_body);
    assert_eq!(audit["canonical_route"], "/api/v1/audit/operator-actions");
    assert_eq!(audit["backend"], "postgres.audit_events");
    assert!(audit["rows"].is_array());
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

#[tokio::test]
async fn gui_ops_action_endpoint_dispatches_correctly() {
    use axum::http::header;

    let router = make_router();

    // Helper: POST /api/v1/ops/action with a JSON body.
    async fn post_action(
        router: axum::Router,
        action_key: &str,
    ) -> (StatusCode, serde_json::Value) {
        let body = serde_json::json!({ "action_key": action_key, "reason": null });
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/ops/action")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let (status, bytes) = call(router, req).await;
        let json = serde_json::from_slice::<serde_json::Value>(&bytes)
            .expect("ops/action response must be valid JSON");
        (status, json)
    }

    // arm-execution: must return 200, accepted=true, disposition="applied".
    let (s, j) = post_action(router.clone(), "arm-execution").await;
    assert_eq!(s, StatusCode::OK, "arm-execution must return 200: {j}");
    assert_eq!(j["accepted"], true, "arm-execution accepted must be true: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("applied"),
        "arm-execution disposition must be 'applied': {j}"
    );
    // requested_action reflects the handler's internal canonical name ("control.arm").
    assert!(
        j["requested_action"].as_str().is_some_and(|v| !v.is_empty()),
        "arm-execution requested_action must be a non-empty string: {j}"
    );

    // disarm-execution: must return 200, accepted=true, disposition="applied".
    let (s, j) = post_action(router.clone(), "disarm-execution").await;
    assert_eq!(s, StatusCode::OK, "disarm-execution must return 200: {j}");
    assert_eq!(j["accepted"], true, "disarm-execution accepted must be true: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("applied"),
        "disarm-execution disposition must be 'applied': {j}"
    );

    // change-system-mode: must return 409 CONFLICT, accepted=false, disposition="not_authoritative".
    // Route is not mounted on the daemon — mode transition requires a controlled restart.
    let (s, j) = post_action(router.clone(), "change-system-mode").await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "change-system-mode must return 409: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "change-system-mode accepted must be false: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("not_authoritative"),
        "change-system-mode disposition must be 'not_authoritative': {j}"
    );
    // blocker must explain that a restart is required.
    assert!(
        j["blocker"]
            .as_str()
            .is_some_and(|v| v.contains("restart")),
        "change-system-mode blocker must mention restart: {j}"
    );

    // unknown key: must return 400 BAD_REQUEST, accepted=false.
    let (s, j) = post_action(router.clone(), "not-a-real-action").await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "unknown action key must return 400: {j}"
    );
    assert_eq!(j["accepted"], false, "unknown action accepted must be false: {j}");
}
