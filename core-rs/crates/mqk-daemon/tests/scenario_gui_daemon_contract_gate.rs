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

    let cases: [(&str, &[&str]); 12] = [
        (
            "/api/v1/system/status",
            &[
                "environment",
                "runtime_status",
                "integrity_status",
                "daemon_reachable",
                // AP-09: external broker truth fields — GUI SystemStatus type must
                // receive these on every status response so continuity gating works.
                "broker_snapshot_source",
                "alpaca_ws_continuity",
                "deployment_start_allowed",
                "daemon_mode",
                "adapter_id",
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
            "/api/v1/reconcile/mismatches",
            &["truth_state", "snapshot_at_utc", "rows"],
        ),
        (
            "/api/v1/audit/operator-actions",
            &["canonical_route", "truth_state", "backend", "rows"],
        ),
        (
            "/api/v1/audit/artifacts",
            &["canonical_route", "truth_state", "backend", "rows"],
        ),
        (
            "/api/v1/ops/operator-timeline",
            &["canonical_route", "truth_state", "backend", "rows"],
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
            assert_eq!(
                json["truth_state"].as_str(),
                Some("backend_unavailable"),
                "{uri} must explicitly declare durable truth unavailable when no DB pool is present"
            );
            assert_eq!(
                json["backend"].as_str(),
                Some("unavailable"),
                "{uri} must not claim a postgres durable backend when no DB pool is present"
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
            // No active run and no DB-backed latest run in test state → generation_id
            // must be null, not a fabricated placeholder like "paper-no-run".
            assert!(
                json["generation_id"].is_null(),
                "/api/v1/system/runtime-leadership generation_id must be null when authoritative runtime identity is unavailable; got: {}",
                json["generation_id"]
            );
            // No DB pool in test state → restart_count_24h must be null (not a
            // synthetic zero); the real count requires a DB query and is unavailable
            // without a pool.  null is the honest signal.
            assert!(
                json["restart_count_24h"].is_null(),
                "/api/v1/system/runtime-leadership restart_count_24h must be null when no DB pool is present; got: {}",
                json["restart_count_24h"]
            );
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
    // PT-TRUTH-01: paper+paper default is fail-closed.
    assert_eq!(status["deployment_start_allowed"], false);
    assert!(!status["deployment_blocker"].is_null());
    // No DB pool in test state → db_status must be "unavailable" (not "unknown").
    // "unknown" = unchecked; "unavailable" = checked and confirmed no pool.
    assert_eq!(json_str(&status, "db_status"), "unavailable");
    assert_eq!(json_str(&status, "audit_writer_status"), "unavailable");
    // No market data subsystem wired → must be "not_configured", not "unknown".
    // "unknown" = we didn't even check; "not_configured" = explicitly absent.
    // AP-04B: value comes from typed StrategyMarketDataSource, independent of adapter.
    assert_eq!(json_str(&status, "market_data_health"), "not_configured");
    // AP-04: paper adapter must surface synthetic broker snapshot source.
    assert_eq!(json_str(&status, "broker_snapshot_source"), "synthetic");
    // AP-05: paper adapter → WS continuity is not_applicable (no WS path for paper).
    assert_eq!(json_str(&status, "alpaca_ws_continuity"), "not_applicable");

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
    // PT-TRUTH-01: paper+paper default is fail-closed.
    assert_eq!(preflight["deployment_start_allowed"], false);
    assert_eq!(preflight["strategy_disarmed"], true);
    assert_eq!(preflight["execution_disarmed"], true);
    assert_eq!(preflight["live_routing_disabled"], true);
    // No DB pool in test state → db_reachable must be null (not a synthetic blocker).
    assert_eq!(preflight["db_reachable"], serde_json::Value::Null);
    // Paper adapter → broker_config_present is Some(false) → JSON false, not null.
    assert_eq!(preflight["broker_config_present"], false);
    // PT-MD-01: market_data_config_present must be false (not null).
    // StrategyMarketDataSource has only NotConfigured; the value is known and
    // explicitly absent, not "unchecked."  Null would imply "not probed."
    assert_eq!(preflight["market_data_config_present"], false);
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
        blockers.contains(&"Execution is disarmed at the integrity gate."),
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
    // PT-TRUTH-01: paper+paper default is fail-closed.
    assert_eq!(session["deployment_start_allowed"], false);
    assert!(!session["deployment_blocker"].is_null());
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
    // PT-TRUTH-01: paper+paper default is fail-closed; config_hash reflects "blocked".
    assert_eq!(
        json_str(&config, "config_hash"),
        "daemon-runtime-paper-blocked-v1"
    );
    assert!(config["build_version"].is_string());
    // OPTR-01: truth_state must be "no_db" when no DB pool is configured.
    assert_eq!(
        json_str(&config, "truth_state"),
        "no_db",
        "OPTR-01: config-fingerprint truth_state must be 'no_db' when DB pool is absent"
    );
    assert!(
        config["risk_policy_version"].is_null(),
        "risk policy version must be null when canonical config fingerprint truth is unavailable"
    );
    assert!(
        config["strategy_bundle_version"].is_null(),
        "strategy bundle version must be null when canonical config fingerprint truth is unavailable"
    );
    assert!(
        config["runtime_generation_id"].is_null(),
        "runtime generation id must be null when no authoritative runtime generation exists"
    );
    assert_ne!(config["risk_policy_version"], "unknown");
    assert_ne!(config["strategy_bundle_version"], "unknown");
    assert_ne!(config["runtime_generation_id"], "unknown");

    let strategy_req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (strategy_status, strategy_body) = call(router.clone(), strategy_req).await;
    assert_eq!(strategy_status, StatusCode::OK);
    let strategy_json = parse_json(strategy_body);
    // Strategy summary must be a wrapper with truth_state, NOT a bare array
    // containing a synthetic daemon_integrity_gate row.  Real strategy-fleet
    // truth is not yet wired; the route must be explicit about that so the GUI
    // does not render a fake strategy row as authoritative fleet state.
    assert!(
        strategy_json.as_object().is_some(),
        "/api/v1/strategy/summary must return a wrapper object, not a bare array; got: {strategy_json}"
    );
    // CC-01B: route now sources truth from postgres.sys_strategy_registry.
    // No DB pool → truth_state="no_db" (fail-closed), not "not_wired".
    assert_eq!(
        strategy_json["truth_state"], "no_db",
        "CC-01B: strategy summary must declare truth_state=no_db when DB unavailable"
    );
    assert!(
        strategy_json["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "strategy summary rows must be empty when no_db"
    );
    // Explicitly confirm the synthetic daemon_integrity_gate surrogate is absent.
    assert!(
        !strategy_json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["strategy_id"] == "daemon_integrity_gate"),
        "daemon_integrity_gate must not appear as a strategy row"
    );

    let audit_req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (audit_status, audit_body) = call(router, audit_req).await;
    assert_eq!(audit_status, StatusCode::OK);
    let audit = parse_json(audit_body);
    assert_eq!(audit["canonical_route"], "/api/v1/audit/operator-actions");
    assert_eq!(audit["truth_state"], "backend_unavailable");
    assert_eq!(audit["backend"], "unavailable");
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
    assert_eq!(
        j["accepted"], true,
        "arm-execution accepted must be true: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("applied"),
        "arm-execution disposition must be 'applied': {j}"
    );
    // requested_action reflects the handler's internal canonical name ("control.arm").
    assert!(
        j["requested_action"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "arm-execution requested_action must be a non-empty string: {j}"
    );

    // disarm-execution: must return 200, accepted=true, disposition="applied".
    let (s, j) = post_action(router.clone(), "disarm-execution").await;
    assert_eq!(s, StatusCode::OK, "disarm-execution must return 200: {j}");
    assert_eq!(
        j["accepted"], true,
        "disarm-execution accepted must be true: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("applied"),
        "disarm-execution disposition must be 'applied': {j}"
    );

    // change-system-mode: must return 409 CONFLICT with ModeChangeGuidanceResponse.
    // No hot switching. Response must be authoritative guidance, not a dead-end refusal.
    let (s, j) = post_action(router.clone(), "change-system-mode").await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "change-system-mode must return 409: {j}"
    );
    // transition_permitted must be false — no hot switching ever.
    assert_eq!(
        j["transition_permitted"], false,
        "change-system-mode transition_permitted must be false: {j}"
    );
    // canonical_route must self-identify the guidance surface.
    assert_eq!(
        j["canonical_route"].as_str(),
        Some("/api/v1/ops/mode-change-guidance"),
        "change-system-mode canonical_route must be /api/v1/ops/mode-change-guidance: {j}"
    );
    // current_mode must be non-empty — this is the authoritative state the operator sees.
    assert!(
        j["current_mode"].as_str().is_some_and(|m| !m.is_empty()),
        "change-system-mode current_mode must be non-empty: {j}"
    );
    // operator_next_steps must be a non-empty array mentioning restart — explicit workflow.
    assert!(
        j["operator_next_steps"].as_array().is_some_and(|arr| {
            !arr.is_empty()
                && arr.iter().any(|v| {
                    v.as_str()
                        .is_some_and(|s| s.to_lowercase().contains("restart"))
                })
        }),
        "change-system-mode operator_next_steps must be non-empty and mention restart: {j}"
    );
    // preconditions must be present and non-empty.
    assert!(
        j["preconditions"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
        "change-system-mode preconditions must be non-empty: {j}"
    );

    // unknown key: must return 400 BAD_REQUEST, accepted=false.
    let (s, j) = post_action(router.clone(), "not-a-real-action").await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "unknown action key must return 400: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "unknown action accepted must be false: {j}"
    );
}

#[tokio::test]
async fn gui_ops_catalog_endpoint_is_daemon_authoritative() {
    // Proves that /api/v1/ops/catalog:
    // 1. Returns 200 with the canonical_route self-identifier.
    // 2. Returns exactly the 7 supported action keys — no fantasy keys.
    // 3. Does NOT include change-system-mode (returns 409 from dispatcher).
    // 4. Each entry has all required fields.
    // 5. Availability is state-correct: disarmed paper+paper test state means
    //    arm-execution=enabled, disarm-execution=disabled,
    //    start-system=disabled (idle but deployment not ready — paper+paper is
    //      fail-closed per PT-TRUTH-01; deployment gate is now reflected in
    //      the catalog per DESKTOP-10),
    //    stop-system=disabled (not running),
    //    kill-switch=enabled (not halted),
    //    request-mode-change=enabled (not halted),
    //    cancel-mode-transition=disabled (no DB → no pending intent).
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/catalog")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/ops/catalog must return 200"
    );

    let json = parse_json(body);

    // Shape: canonical_route and actions array.
    assert_eq!(
        json["canonical_route"].as_str(),
        Some("/api/v1/ops/catalog"),
        "ops/catalog must self-identify canonical_route"
    );
    let actions = json["actions"]
        .as_array()
        .expect("/api/v1/ops/catalog must have an 'actions' array");

    // Exactly 8 entries (AUTON-PAPER-OPS-04: clear-halted-run added).
    assert_eq!(
        actions.len(),
        8,
        "catalog must have exactly 8 entries; got: {actions:?}"
    );

    // Collect action_key values.
    let keys: Vec<&str> = actions
        .iter()
        .filter_map(|a| a["action_key"].as_str())
        .collect();

    // All 8 supported keys must be present.
    for expected_key in &[
        "arm-execution",
        "disarm-execution",
        "start-system",
        "stop-system",
        "kill-switch",
        "request-mode-change",
        "cancel-mode-transition",
        "clear-halted-run",
    ] {
        assert!(
            keys.contains(expected_key),
            "catalog must contain action_key '{expected_key}'; got keys: {keys:?}"
        );
    }

    // change-system-mode must NOT appear — it returns 409 from the dispatcher.
    assert!(
        !keys.contains(&"change-system-mode"),
        "change-system-mode must not appear in catalog (returns 409 from dispatcher)"
    );

    // Each entry must have all required fields.
    for entry in actions {
        let key = entry["action_key"].as_str().unwrap_or("?");
        assert!(entry["label"].is_string(), "{key}: missing 'label'");
        assert!(entry["level"].is_number(), "{key}: missing 'level'");
        assert!(
            entry["description"].is_string(),
            "{key}: missing 'description'"
        );
        assert!(
            entry["requires_reason"].is_boolean(),
            "{key}: missing 'requires_reason'"
        );
        assert!(
            entry["confirm_text"].is_string(),
            "{key}: missing 'confirm_text'"
        );
        assert!(entry["enabled"].is_boolean(), "{key}: missing 'enabled'");
    }

    // State-specific availability in test state (no DB, disarmed, idle, not halted).
    let by_key = |k: &str| -> &serde_json::Value {
        actions
            .iter()
            .find(|a| a["action_key"].as_str() == Some(k))
            .unwrap()
    };

    // Disarmed → arm-execution must be enabled.
    assert_eq!(
        by_key("arm-execution")["enabled"],
        true,
        "arm-execution must be enabled in disarmed test state"
    );

    // Disarmed → disarm-execution must be disabled.
    assert_eq!(
        by_key("disarm-execution")["enabled"],
        false,
        "disarm-execution must be disabled in disarmed test state"
    );
    assert!(
        by_key("disarm-execution")["disabled_reason"].is_string(),
        "disarm-execution must have a disabled_reason in disarmed state"
    );

    // Idle but deployment not ready (paper+paper is fail-closed per PT-TRUTH-01).
    // DESKTOP-10: start-system must be disabled when deployment_start_allowed=false,
    // even if the runtime is idle and not halted.
    assert_eq!(
        by_key("start-system")["enabled"],
        false,
        "start-system must be disabled in paper+paper test state (deployment not ready)"
    );
    assert!(
        by_key("start-system")["disabled_reason"].is_string(),
        "start-system must carry a disabled_reason explaining the deployment blocker"
    );

    // Not running → stop-system must be disabled.
    assert_eq!(
        by_key("stop-system")["enabled"],
        false,
        "stop-system must be disabled in idle test state"
    );

    // Not halted → kill-switch must be enabled.
    assert_eq!(
        by_key("kill-switch")["enabled"],
        true,
        "kill-switch must be enabled in non-halted test state"
    );

    // OPS-CONTROL-02: Not halted → request-mode-change must be enabled.
    assert_eq!(
        by_key("request-mode-change")["enabled"],
        true,
        "request-mode-change must be enabled when not halted"
    );

    // OPS-CONTROL-02: No DB → cancel-mode-transition must be disabled.
    assert_eq!(
        by_key("cancel-mode-transition")["enabled"],
        false,
        "cancel-mode-transition must be disabled when no DB (no pending intent possible)"
    );
    assert!(
        by_key("cancel-mode-transition")["disabled_reason"].is_string(),
        "cancel-mode-transition must have a disabled_reason when no pending intent"
    );
}

#[tokio::test]
async fn gui_contract_execution_orders_503_without_snapshot() {
    // /api/v1/execution/orders must return HTTP 503 when no execution snapshot exists.
    //
    // SEMANTIC INVARIANT: 503 = "no OMS snapshot, truth unavailable".
    //   This is distinct from 200 + [] = "snapshot exists, zero active orders".
    //   503 causes the GUI to keep the endpoint in missingEndpoints so
    //   isMissingPanelTruth fires and the execution panel hard-blocks.
    //
    // Without this invariant, the GUI would render an empty order list as
    // authoritative healthy truth when the execution loop has never started.
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/orders")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "/api/v1/execution/orders must return 503 when no execution snapshot is present"
    );

    // Body must be a JSON object with error/detail fields for debuggability.
    let json = parse_json(body);
    assert!(
        json.get("error").is_some(),
        "/api/v1/execution/orders 503 body must have 'error' field; got: {json}"
    );
    assert!(
        json.get("detail").is_some(),
        "/api/v1/execution/orders 503 body must have 'detail' field; got: {json}"
    );
    assert_eq!(
        json["error"].as_str(),
        Some("no_execution_snapshot"),
        "/api/v1/execution/orders 503 error key must be 'no_execution_snapshot'"
    );

    // Legacy broker-snapshot path must still resolve independently.
    let legacy_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/orders")
        .body(axum::body::Body::empty())
        .unwrap();
    let (legacy_status, _) = call(router, legacy_req).await;
    assert_eq!(
        legacy_status,
        StatusCode::OK,
        "/v1/trading/orders must remain mounted alongside canonical /api/v1/execution/orders"
    );
}

#[tokio::test]
async fn gui_contract_execution_orders_200_array_with_injected_snapshot() {
    // When an execution snapshot is injected into AppState, the endpoint must return
    // HTTP 200 + a bare JSON array.  Zero active orders in the snapshot → empty array.
    // This proves the distinction: 200 + [] means "snapshot active, no orders" (not "no snapshot").
    use chrono::DateTime;
    use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    // Inject a minimal snapshot: loop is running, no active orders.
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
        recent_risk_denials: vec![],
        snapshot_at_utc: DateTime::from_timestamp(0, 0).expect("unix epoch is valid"),
    };
    *st.execution_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/orders")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/execution/orders must return 200 when execution snapshot is present"
    );
    let json = parse_json(body);
    assert!(
        json.as_array().is_some(),
        "/api/v1/execution/orders with snapshot must return a JSON array; got: {json}"
    );
    // Snapshot has zero active orders → empty array.
    assert_eq!(
        json.as_array().map(|v| v.is_empty()),
        Some(true),
        "/api/v1/execution/orders must be empty array when snapshot has no active orders"
    );
}

#[tokio::test]
async fn gui_contract_not_wired_surfaces_declare_truth_state() {
    // config-diffs remains "not_wired" (no durable backing yet).
    // strategy/suppressions (CC-02) and strategy/summary (CC-01B) are now
    // durable and return "no_db" when no DB pool is configured — neither
    // returns "not_wired" any more.
    //
    // Each must return a wrapper object — NOT a bare array — so the GUI IIFEs
    // can emit ok:false and prevent fake-zero / fake-row rendering.
    let router = make_router();

    // /api/v1/system/config-diffs — ConfigDiffsResponse wrapper
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/config-diffs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/system/config-diffs must return 200"
    );
    let json = parse_json(body);
    assert!(
        json.as_object().is_some(),
        "/api/v1/system/config-diffs must return a wrapper object, not a bare array; got: {json}"
    );
    assert_eq!(
        json["truth_state"], "not_wired",
        "/api/v1/system/config-diffs must declare truth_state=not_wired"
    );
    assert!(
        json["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "/api/v1/system/config-diffs rows must be empty when not_wired"
    );

    // /api/v1/strategy/suppressions — CC-02: durable surface.
    // Without DB pool: returns truth_state="no_db" (source unavailable, not permanently not_wired).
    // GUI renders "unavailable" notice rather than "not wired" notice.
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/strategy/suppressions must return 200"
    );
    let json = parse_json(body);
    assert!(
        json.as_object().is_some(),
        "/api/v1/strategy/suppressions must return a wrapper object, not a bare array; got: {json}"
    );
    assert_eq!(
        json["truth_state"], "no_db",
        "/api/v1/strategy/suppressions must declare truth_state=no_db when no DB pool is configured"
    );
    assert_eq!(
        json["canonical_route"], "/api/v1/strategy/suppressions",
        "/api/v1/strategy/suppressions must carry canonical_route self-identity"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_suppressions",
        "/api/v1/strategy/suppressions must declare its backend source"
    );
    assert!(
        json["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "/api/v1/strategy/suppressions rows must be empty when no_db"
    );

    // /api/v1/strategy/summary — CC-01B: durable surface (postgres.sys_strategy_registry).
    // No DB pool → truth_state="no_db" (fail-closed), not "not_wired".
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/strategy/summary must return 200"
    );
    let json = parse_json(body);
    assert!(
        json.as_object().is_some(),
        "/api/v1/strategy/summary must return a wrapper object, not a bare array; got: {json}"
    );
    assert_eq!(
        json["truth_state"], "no_db",
        "CC-01B: /api/v1/strategy/summary must declare truth_state=no_db when no DB pool is configured"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_registry",
        "CC-01B: /api/v1/strategy/summary must identify its backend source"
    );
    assert!(
        json["rows"]
            .as_array()
            .map(|v| v.is_empty())
            .unwrap_or(false),
        "/api/v1/strategy/summary rows must be empty when no_db"
    );
    // Confirm the synthetic daemon_integrity_gate surrogate cannot sneak back in.
    assert!(
        !json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["strategy_id"] == "daemon_integrity_gate"),
        "daemon_integrity_gate must not appear as a strategy row"
    );
}

// ---------------------------------------------------------------------------
// Cluster 2 — canonical portfolio surfaces (positions, orders/open, fills)
// ---------------------------------------------------------------------------
//
// Contract invariants proven here:
//  1. When broker_snapshot is absent: HTTP 200, snapshot_state = "no_snapshot",
//     rows = [] (empty array), captured_at_utc = null.
//  2. When broker_snapshot is present: HTTP 200, snapshot_state = "active",
//     captured_at_utc is non-null, rows reflect the injected data.
//
// This distinguishes "authoritative empty snapshot" from "no broker truth".
// GUI reads snapshot_state as a typed field — not an HTTP status string.

#[tokio::test]
async fn gui_contract_portfolio_positions_no_snapshot() {
    // Without a broker snapshot, /api/v1/portfolio/positions must return HTTP 200
    // with snapshot_state = "no_snapshot" so the GUI knows truth is unavailable.
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/positions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/positions must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("no_snapshot"),
        "snapshot_state must be no_snapshot when broker_snapshot is absent; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|v| v.is_empty()),
        "rows must be empty when snapshot_state is no_snapshot; got: {json}"
    );
    assert!(
        json["captured_at_utc"].is_null(),
        "captured_at_utc must be null when snapshot_state is no_snapshot; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_portfolio_positions_active_snapshot() {
    // With an injected broker snapshot, /api/v1/portfolio/positions must return
    // snapshot_state = "active" and rows reflecting the injected positions.
    use chrono::DateTime;
    use mqk_schemas::{BrokerAccount, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let snap = BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "50000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions: vec![mqk_schemas::BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "175.50".to_string(),
        }],
    };
    *st.broker_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/positions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/positions must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("active"),
        "snapshot_state must be active when broker_snapshot is present; got: {json}"
    );
    assert!(
        json["captured_at_utc"].as_str().is_some(),
        "captured_at_utc must be non-null when snapshot_state is active; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be a JSON array");
    assert_eq!(
        rows.len(),
        1,
        "rows must reflect the injected positions; got: {json}"
    );
    assert_eq!(
        rows[0]["symbol"].as_str(),
        Some("AAPL"),
        "row symbol must match injected position; got: {json}"
    );
    assert_eq!(
        rows[0]["qty"].as_i64(),
        Some(10),
        "row qty must match injected position; got: {json}"
    );
    assert!(
        rows[0]["strategy_id"].is_null(),
        "strategy_id must be null for broker-layer position rows (no attribution at broker snapshot layer); got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_portfolio_open_orders_no_snapshot() {
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/orders/open")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/orders/open must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("no_snapshot"),
        "snapshot_state must be no_snapshot when broker_snapshot is absent; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|v| v.is_empty()),
        "rows must be empty when snapshot_state is no_snapshot; got: {json}"
    );
    assert!(
        json["captured_at_utc"].is_null(),
        "captured_at_utc must be null when snapshot_state is no_snapshot; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_portfolio_open_orders_active_snapshot() {
    use chrono::DateTime;
    use mqk_schemas::{BrokerAccount, BrokerOrder, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let snap = BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "50000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![BrokerOrder {
            broker_order_id: "broker-ord-1".to_string(),
            client_order_id: "client-ord-1".to_string(),
            symbol: "TSLA".to_string(),
            side: "buy".to_string(),
            r#type: "market".to_string(),
            status: "new".to_string(),
            qty: "5".to_string(),
            limit_price: None,
            stop_price: None,
            created_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        }],
        fills: vec![],
        positions: vec![],
    };
    *st.broker_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/orders/open")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/orders/open must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("active"),
        "snapshot_state must be active when broker_snapshot is present; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be a JSON array");
    assert_eq!(
        rows.len(),
        1,
        "rows must reflect the injected order; got: {json}"
    );
    assert_eq!(
        rows[0]["internal_order_id"].as_str(),
        Some("client-ord-1"),
        "internal_order_id must equal client_order_id from broker snapshot; got: {json}"
    );
    assert_eq!(
        rows[0]["symbol"].as_str(),
        Some("TSLA"),
        "symbol must match injected order; got: {json}"
    );
    assert!(
        rows[0]["strategy_id"].is_null(),
        "strategy_id must be null for broker-layer open order rows (no attribution at broker snapshot layer); got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_portfolio_fills_no_snapshot() {
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/fills")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/fills must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("no_snapshot"),
        "snapshot_state must be no_snapshot when broker_snapshot is absent; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|v| v.is_empty()),
        "rows must be empty when snapshot_state is no_snapshot; got: {json}"
    );
    assert!(
        json["captured_at_utc"].is_null(),
        "captured_at_utc must be null when snapshot_state is no_snapshot; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_portfolio_fills_active_snapshot() {
    use chrono::DateTime;
    use mqk_schemas::{BrokerAccount, BrokerFill, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let snap = BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "50000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![BrokerFill {
            broker_fill_id: "fill-001".to_string(),
            broker_order_id: "broker-ord-1".to_string(),
            client_order_id: "client-ord-1".to_string(),
            symbol: "NVDA".to_string(),
            side: "buy".to_string(),
            qty: "3".to_string(),
            price: "450.25".to_string(),
            fee: "0.00".to_string(),
            ts_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        }],
        positions: vec![],
    };
    *st.broker_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/portfolio/fills")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/portfolio/fills must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str(),
        Some("active"),
        "snapshot_state must be active when broker_snapshot is present; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be a JSON array");
    assert_eq!(
        rows.len(),
        1,
        "rows must reflect the injected fill; got: {json}"
    );
    assert_eq!(
        rows[0]["fill_id"].as_str(),
        Some("fill-001"),
        "fill_id must equal broker_fill_id from broker snapshot; got: {json}"
    );
    assert_eq!(
        rows[0]["symbol"].as_str(),
        Some("NVDA"),
        "symbol must match injected fill; got: {json}"
    );
    assert_eq!(
        rows[0]["applied"].as_bool(),
        Some(true),
        "applied must be true for fills in broker snapshot; got: {json}"
    );
    assert_eq!(
        rows[0]["broker_exec_id"].as_str(),
        Some("fill-001"),
        "broker_exec_id must equal fill_id; got: {json}"
    );
    assert!(
        rows[0]["strategy_id"].is_null(),
        "strategy_id must be null for broker-layer fill rows (no attribution at broker snapshot layer); got: {json}"
    );
}

// ---------------------------------------------------------------------------
// Cluster 3 — canonical risk denial surface (/api/v1/risk/denials)
// ---------------------------------------------------------------------------
//
// Contract invariants proven here:
//  1. Without an execution snapshot (no pool, no loop): HTTP 200,
//     truth_state = "no_snapshot", denials = [], snapshot_at_utc = null.
//     → GUI IIFE reads truth_state and emits ok:false → endpoint lands in
//       missingEndpoints → isMissingPanelTruth fires → risk panel blocks.
//  2. With an injected execution snapshot (no pool): HTTP 200,
//     truth_state = "active_session_only", denials = [], snapshot_at_utc non-null.
//     → No DB pool means ring-buffer only; NOT restart-safe.  Labeled honestly.
//     → In production (pool always present) truth_state would be "active" (durable).
//  3. With an injected execution snapshot and a denial record (no pool):
//     HTTP 200, truth_state = "active_session_only", one denial row in denials.
//     → Ring-buffer record propagates through the route; strategy_id = null.
//  4. (DB-backed, see scenario_daemon_routes.rs) With a pool but no loop:
//     truth_state = "durable_history" when DB has rows — restart-safe.
//  5. (DB-backed, see scenario_daemon_routes.rs) With a pool + loop running:
//     truth_state = "active"; only DB rows returned; ring buffer NOT merged.

#[tokio::test]
async fn gui_contract_risk_denials_no_snapshot() {
    // Without an execution snapshot, /api/v1/risk/denials must return HTTP 200
    // with truth_state = "no_snapshot" so the GUI knows denial truth is unavailable.
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/risk/denials must return HTTP 200"
    );
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"].as_str(),
        Some("no_snapshot"),
        "truth_state must be no_snapshot when execution_snapshot is absent; got: {json}"
    );
    assert!(
        json["denials"].as_array().is_some_and(|v| v.is_empty()),
        "denials must be empty when truth_state is no_snapshot; got: {json}"
    );
    assert!(
        json["snapshot_at_utc"].is_null(),
        "snapshot_at_utc must be null when truth_state is no_snapshot; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_risk_denials_active_snapshot() {
    // When an execution snapshot is injected into a no-pool AppState,
    // /api/v1/risk/denials must return HTTP 200 with truth_state =
    // "active_session_only" — no DB pool means only ring-buffer rows are
    // available, which are NOT restart-safe.  This is the explicit contract for
    // no-pool environments (test/dev).  In production (pool always present)
    // truth_state would be "active" (durable).
    use chrono::DateTime;
    use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    // Inject a minimal execution snapshot: loop is running, no denials.
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
        recent_risk_denials: vec![],
        snapshot_at_utc: DateTime::from_timestamp(0, 0).expect("unix epoch is valid"),
    };
    *st.execution_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/risk/denials must return HTTP 200 when execution snapshot is present"
    );
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active_session_only"),
        "truth_state must be active_session_only (no pool) when execution_snapshot is present without a DB pool; got: {json}"
    );
    assert!(
        json["denials"].as_array().is_some_and(|v| v.is_empty()),
        "denials must be empty array when ring buffer is empty; got: {json}"
    );
    assert!(
        !json["snapshot_at_utc"].is_null(),
        "snapshot_at_utc must be non-null when execution loop is running; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_risk_denials_real_row_appears() {
    // Proves that a real denial record in recent_risk_denials propagates through
    // the route as a correctly serialized denial row.
    // This is the key semantic test: the route must not suppress or transform
    // denial records; what the orchestrator captures must reach the GUI.
    use chrono::DateTime;
    use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, RiskDenialRecord};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    // Inject a snapshot with one real denial record (as the orchestrator would
    // produce when RiskGate::evaluate_gate() returns RiskDecision::Deny).
    let denial_at = DateTime::from_timestamp(1_700_000_100, 0).expect("valid unix timestamp");
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
        recent_risk_denials: vec![RiskDenialRecord {
            id: "1700000100000000:POSITION_LIMIT_EXCEEDED".to_string(),
            denied_at_utc: denial_at,
            rule: "POSITION_LIMIT_EXCEEDED".to_string(),
            message: "Order denied — resulting position would exceed limit".to_string(),
            symbol: Some("AAPL".to_string()),
            requested_qty: Some(500),
            limit: Some(200),
            severity: "critical".to_string(),
        }],
        snapshot_at_utc: DateTime::from_timestamp(1_700_000_200, 0).expect("valid unix timestamp"),
    };
    *st.execution_snapshot.write().await = Some(snap);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/risk/denials")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    // No pool → ring buffer path → truth_state must be active_session_only.
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active_session_only"),
        "truth_state must be active_session_only (no pool); got: {json}"
    );
    let rows = json["denials"]
        .as_array()
        .expect("denials must be an array");
    assert_eq!(
        rows.len(),
        1,
        "one denial record must produce one denial row"
    );

    let row = &rows[0];
    assert_eq!(
        row["id"].as_str(),
        Some("1700000100000000:POSITION_LIMIT_EXCEEDED"),
        "row id must match the record id; got: {row}"
    );
    assert_eq!(
        row["rule"].as_str(),
        Some("POSITION_LIMIT_EXCEEDED"),
        "row rule must match the record rule; got: {row}"
    );
    assert_eq!(
        row["symbol"].as_str(),
        Some("AAPL"),
        "row symbol must match the record symbol; got: {row}"
    );
    assert_eq!(
        row["severity"].as_str(),
        Some("critical"),
        "row severity must match the record severity; got: {row}"
    );
    assert!(
        row["message"]
            .as_str()
            .is_some_and(|m| m.contains("position")),
        "row message must contain the human-readable denial reason; got: {row}"
    );
    assert!(
        !row["at"].as_str().unwrap_or("").is_empty(),
        "row at must be a non-empty RFC3339 timestamp; got: {row}"
    );
    // strategy_id must be null — not available on the risk gate path.
    assert!(
        row["strategy_id"].is_null(),
        "strategy_id must be null for risk denial rows (not available on risk gate path); got: {row}"
    );
}

#[tokio::test]
async fn gui_contract_reconcile_mismatches_no_snapshot_without_authoritative_detail() {
    let router = make_router();

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/reconcile/mismatches")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/reconcile/mismatches must return HTTP 200"
    );
    let json = parse_json(body);
    // RECON-06: when the reconcile loop has never completed a tick, the
    // truth_state is "never_run" (not "no_snapshot").  "never_run" unambiguously
    // means the daemon started but reconcile has not yet run — it is distinct
    // from "no_snapshot" (which means snapshots are missing but reconcile ran).
    assert_eq!(
        json["truth_state"].as_str(),
        Some("never_run"),
        "truth_state must be never_run when reconcile has not yet completed a tick; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|v| v.is_empty()),
        "rows must be empty when truth_state is never_run; got: {json}"
    );
    assert!(
        json["snapshot_at_utc"].is_null(),
        "snapshot_at_utc must be null when truth_state is never_run; got: {json}"
    );
}

#[tokio::test]
async fn gui_contract_reconcile_mismatches_active_with_authoritative_diff_rows() {
    use chrono::DateTime;
    use mqk_daemon::state::ReconcileStatusSnapshot;
    use mqk_runtime::observability::{ExecutionSnapshot, OrderSnapshot, PortfolioSnapshot};
    use mqk_schemas::{BrokerAccount, BrokerOrder, BrokerSnapshot};

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let execution_snapshot = ExecutionSnapshot {
        run_id: None,
        active_orders: vec![OrderSnapshot {
            order_id: "OID-1".to_string(),
            broker_order_id: Some("BRK-1".to_string()),
            symbol: "NVDA".to_string(),
            total_qty: 80,
            filled_qty: 20,
            status: "PartiallyFilled".to_string(),
        }],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: vec![],
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        snapshot_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid unix timestamp"),
    };
    *st.execution_snapshot.write().await = Some(execution_snapshot);
    st.local_order_sides
        .write()
        .await
        .insert("OID-1".to_string(), mqk_reconcile::Side::Buy);

    let broker_snapshot = BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_030, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "50000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![BrokerOrder {
            broker_order_id: "BRK-1".to_string(),
            client_order_id: "OID-1".to_string(),
            symbol: "NVDA".to_string(),
            side: "buy".to_string(),
            r#type: "limit".to_string(),
            status: "partially_filled".to_string(),
            qty: "80".to_string(),
            limit_price: Some("900.00".to_string()),
            stop_price: None,
            created_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        }],
        fills: vec![],
        positions: vec![],
    };
    *st.broker_snapshot.write().await = Some(broker_snapshot);

    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some(
            DateTime::from_timestamp(1_700_000_030, 0)
                .expect("valid timestamp")
                .to_rfc3339(),
        ),
        snapshot_watermark_ms: Some(1_700_000_030_000),
        mismatched_positions: 0,
        mismatched_orders: 1,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("order drift detected".to_string()),
    })
    .await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/reconcile/mismatches")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/reconcile/mismatches must return HTTP 200 when detail truth is active"
    );
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active"),
        "truth_state must be active when reconcile detail rows are authoritative; got: {json}"
    );
    let rows = json["rows"].as_array().expect("rows must be an array");
    assert!(
        rows.iter().any(|row| {
            row["domain"].as_str() == Some("order")
                && row["symbol"].as_str() == Some("NVDA")
                && row["internal_value"].as_str() == Some("filled_qty=20")
                && row["broker_value"].as_str() == Some("filled_qty=0")
        }),
        "expected an authoritative filled_qty mismatch row for NVDA; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// Cluster 5 — durable operator-history surfaces (audit + timeline)
// ---------------------------------------------------------------------------
//
// Contract invariants proven here:
//  1. Each endpoint returns HTTP 200 + {canonical_route, backend, rows} wrapper.
//  2. canonical_route self-identifies the endpoint URI.
//  3. backend identifies the exact Postgres table(s) used as the durable source.
//  4. rows is an empty JSON array in no-DB test state.
//
// Row-level field contracts (audit_event_id, ts_utc, requested_action, etc.)
// are enforced by DB-backed integration tests (scenario_alpaca_inbound_rt_brk08r
// family and future operator-history DB scenarios) where real rows can be
// inserted and asserted.
//
// This test proves that the GUI fetch/map layer can rely on the wrapper shape
// to correctly unwrap rows[] without degrading to mock/placeholder authority.

#[tokio::test]
async fn gui_contract_operator_history_endpoints_fail_closed_when_durable_backend_is_unavailable() {
    // Proves that the three durable operator-history surfaces keep their
    // wrapper shape but DO NOT claim postgres-backed truth when no DB pool is
    // configured in test state.
    let router = make_router();

    let cases: [&str; 3] = [
        "/api/v1/audit/operator-actions",
        "/api/v1/audit/artifacts",
        "/api/v1/ops/operator-timeline",
    ];

    for uri in cases {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, body) = call(router.clone(), req).await;
        assert_eq!(status, StatusCode::OK, "{uri} must return 200");

        let json = parse_json(body);

        // Wrapper structure remains stable for GUI fetch/mapping.
        assert_eq!(
            json["canonical_route"].as_str(),
            Some(uri),
            "{uri} must self-identify its canonical_route"
        );
        assert_eq!(
            json["truth_state"].as_str(),
            Some("backend_unavailable"),
            "{uri} must explicitly declare durable truth unavailable when no DB pool is present"
        );
        assert_eq!(
            json["backend"].as_str(),
            Some("unavailable"),
            "{uri} must not claim a postgres backend when durable truth is unavailable"
        );
        assert!(
            json["rows"].is_array(),
            "{uri} rows must still be a JSON array; got: {json}"
        );
        assert_eq!(
            json["rows"].as_array().map(|v| v.is_empty()),
            Some(true),
            "{uri} rows must be empty when no DB pool is present; got: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// CC-06: alerts/active and events/feed contract gate
// ---------------------------------------------------------------------------

/// CC-06: alerts/active wrapper semantics.
///
/// In a clean daemon state (no DB, no active faults):
/// - 200 OK
/// - truth_state = "active" (always — computed from live in-memory state)
/// - canonical_route = "/api/v1/alerts/active"
/// - backend = "daemon.runtime_state"
/// - alert_count is a non-negative integer
/// - rows is a JSON array
/// - alert_count == rows.len()
#[tokio::test]
async fn gui_contract_alerts_active_wrapper_semantics() {
    let router = make_router();

    let req = Request::builder()
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "alerts/active truth_state must always be 'active'"
    );
    assert_eq!(
        json_str(&json, "canonical_route"),
        "/api/v1/alerts/active",
        "canonical_route must self-identify"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "daemon.runtime_state",
        "backend must be 'daemon.runtime_state'"
    );
    let alert_count = json
        .get("alert_count")
        .and_then(|v| v.as_u64())
        .expect("alert_count must be a non-negative integer");
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be a JSON array");
    assert_eq!(
        alert_count as usize,
        rows.len(),
        "alert_count must equal rows.len()"
    );
}

/// CC-06: events/feed no-DB path declares backend_unavailable explicitly.
///
/// When no DB pool is configured:
/// - 200 OK
/// - truth_state = "backend_unavailable"
/// - canonical_route = "/api/v1/events/feed"
/// - backend = "unavailable"
/// - rows = [] (empty, must not be treated as authoritative empty history)
#[tokio::test]
async fn gui_contract_events_feed_no_db_backend_unavailable() {
    let router = make_router(); // no DB pool

    let req = Request::builder()
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert_eq!(
        json_str(&json, "truth_state"),
        "backend_unavailable",
        "events/feed truth_state must be 'backend_unavailable' when no DB pool"
    );
    assert_eq!(
        json_str(&json, "canonical_route"),
        "/api/v1/events/feed",
        "canonical_route must self-identify"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "unavailable",
        "backend must be 'unavailable' when no DB pool"
    );
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be a JSON array");
    assert!(
        rows.is_empty(),
        "rows must be empty when no DB pool is present"
    );
}
