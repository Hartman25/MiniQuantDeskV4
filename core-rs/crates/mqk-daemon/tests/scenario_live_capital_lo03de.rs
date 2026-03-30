//! LO-03D + LO-03E: Live-capital preflight/blocker enforcement proof and
//! halt/disarm/cut-risk control proof.
//!
//! # LO-03D: Preflight and blocker enforcement
//!
//! Proves the live-capital start gate chain is explicit, ordered, and
//! fail-closed at every layer:
//!
//! D1: live-capital+paper → 403 at deployment gate (first gate, not integrity)
//! D2: live-capital+alpaca disarmed → 403 at integrity gate (deployment passed)
//! D3: live-capital+alpaca armed ExplicitDevNoToken → 403 at operator_auth gate
//!     (deployment+integrity passed; capital-only gate fires)
//! D4: live-capital+alpaca armed TokenRequired WS=ColdStartUnproven → 403 at
//!     alpaca_ws_continuity gate (deployment+integrity+operator_auth all passed)
//! D5: live-capital+alpaca armed TokenRequired WS=Live → 503 DB gate
//!     (full pre-DB gate chain proven; all pre-DB gates pass for live-capital)
//! D6: system/preflight for live-capital+paper is honest and fail-closed
//!     (deployment_start_allowed=false, blockers non-empty, daemon_mode correct)
//!
//! # LO-03E: Halt / disarm / cut-risk controls
//!
//! Proves live-capital halt, disarm, and kill-switch controls are explicit and
//! operator-provable with the correct fail-closed semantics:
//!
//! E1: live-capital halt without DB → 503 (dangerous action requires durable
//!     authority; halt cannot claim success without persisting the record)
//! E2: live-capital disarm without DB → 200 (safe disarm is always reachable;
//!     operator can disable live-capital even when DB is unavailable)
//! E3: after halt (which returns 503 without DB), system status reflects
//!     kill_switch_active=true (halt is fail-safe: in-memory kill-switch
//!     engages even if the DB write fails)
//! E4: no cut-risk route exists → 404 (halt is the only capital kill switch;
//!     there is no separate or shortcut cut-risk mechanism)
//!
//! Does NOT reopen:
//! - LO-03A / LO-03B / LO-03C — live-shadow controls are closed
//! - LO-03G — arm/disarm audit durability is closed
//!
//! All tests are pure in-process (no DB, no env vars, no real broker).
//! All tests are always runnable in CI without MQK_DATABASE_URL.

use std::sync::Arc;

use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::util::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ===========================================================================
// LO-03D: Live-capital preflight and blocker enforcement
// ===========================================================================

// ---------------------------------------------------------------------------
// LO-03D-D1 — live-capital+paper blocked at deployment gate
//
// live-capital requires a real broker adapter.  Paper fill engine cannot
// provide real market truth for capital execution.  The deployment gate must
// fire BEFORE the integrity gate — operator does not need to arm in order to
// discover this misconfiguration.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d1_live_capital_paper_blocked_at_deployment_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Paper,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D1: live-capital+paper must be 403 at deployment gate; got: {status}"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "deployment_mode",
        "D1: gate must be deployment_mode — not integrity_armed, not operator_auth; got: {j}"
    );
    // The deployment gate fires before integrity is even checked.
    // If gate were "integrity_armed", it would mean deployment gate was bypassed.
    assert_ne!(
        j["gate"], "integrity_armed",
        "D1: deployment gate must fire before integrity gate; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03D-D2 — live-capital+alpaca disarmed: blocked at integrity gate
//
// live-capital+alpaca passes the deployment gate.  Without arming, the
// integrity gate fires.  This proves the integrity gate is real and not
// silently bypassed for live-capital mode.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d2_live_capital_alpaca_disarmed_blocked_at_integrity_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    // Do NOT arm — daemon boots disarmed.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D2: live-capital+alpaca disarmed must be 403; got: {status}"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "integrity_armed",
        "D2: gate must be integrity_armed — deployment gate passed, integrity must not be bypassed; got: {j}"
    );
    // Explicitly not deployment_mode — that gate passed (alpaca is correct).
    assert_ne!(
        j["gate"], "deployment_mode",
        "D2: deployment gate must not fire for live-capital+alpaca; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03D-D3 — live-capital+alpaca armed ExplicitDevNoToken: blocked at
//              operator_auth gate
//
// live-capital requires a real operator token.  Dev-no-token mode is
// explicitly rejected at gate 3.  This is a live-capital-specific gate —
// paper and live-shadow modes do not have this requirement.
//
// The arm succeeds (ExplicitDevNoToken passes the middleware) but
// start_execution_runtime fires the capital-specific operator_auth gate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d3_live_capital_alpaca_armed_dev_token_blocked_at_operator_auth_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    // ExplicitDevNoToken is the default test constructor posture.
    let router = routes::build_router(Arc::clone(&st));

    // Arm the integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(router.clone(), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "D3: arm must succeed");

    // Start must fail at the capital-specific operator_auth gate.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D3: live-capital+alpaca armed with dev-no-token must be 403 at operator_auth gate; got: {status}"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "operator_auth",
        "D3: gate must be operator_auth — deployment+integrity passed; \
         capital-specific token requirement must fire; got: {j}"
    );
    // This gate is capital-specific — it should not fire for paper or live-shadow.
    assert_ne!(
        j["gate"], "integrity_armed",
        "D3: integrity gate must have already passed; got: {j}"
    );
    assert_ne!(
        j["gate"], "deployment_mode",
        "D3: deployment gate must have already passed; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03D-D4 — live-capital+alpaca armed TokenRequired WS=ColdStartUnproven:
//              blocked at WS continuity gate
//
// With a real operator token, deployment+integrity+operator_auth gates all
// pass.  The live-capital WS continuity gate then fires because the WS cursor
// has not been established (ColdStartUnproven at boot).
//
// This is the fourth gate in the live-capital gate chain.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d4_live_capital_armed_token_ws_unproven_blocked_at_ws_continuity_gate() {
    let mut st_inner = state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    );
    // Upgrade to real operator token — passes the capital operator_auth gate.
    st_inner.operator_auth = state::OperatorAuthMode::TokenRequired("lo03d-token".to_string());
    let st = Arc::new(st_inner);
    // WS continuity is ColdStartUnproven at boot (default for Alpaca).

    let router = routes::build_router(Arc::clone(&st));

    // Arm via the middleware — must include the token header since auth is TokenRequired.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .header(header::AUTHORIZATION, "Bearer lo03d-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(router, arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "D4: arm must succeed with valid token");

    // Start must fail at the live-capital WS continuity gate.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header(header::AUTHORIZATION, "Bearer lo03d-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D4: live-capital WS=ColdStartUnproven must be 403 at ws continuity gate; got: {status}"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "alpaca_ws_continuity",
        "D4: gate must be alpaca_ws_continuity — deployment+integrity+operator_auth all passed; \
         WS continuity gate must fire; got: {j}"
    );
    // Prove we passed the operator_auth gate (it would have returned gate=operator_auth if not).
    assert_ne!(
        j["gate"], "operator_auth",
        "D4: operator_auth gate must have already passed (TokenRequired is set); got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03D-D5 — full live-capital pre-DB gate chain proven
//
// With all pre-DB gates satisfied:
//   - deployment: live-capital+alpaca → allowed
//   - integrity: armed
//   - operator_auth: TokenRequired set
//   - WS continuity: Live (cursor established)
//
// Start must reach and fire the DB gate (503 — no DB configured in test env).
// This is the definitive proof that the full live-capital pre-DB gate chain
// is traversable and no gate is silently skipped.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d5_live_capital_full_pre_db_gate_chain_proven() {
    // TV-04F: live-capital requires an explicit capital policy.
    // Write a minimal valid policy so the TV-04F and TV-04A/D gates pass.
    let policy_dir = std::env::temp_dir().join(format!(
        "mqk_lo03d_d5_policy_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&policy_dir).expect("D5: create policy dir");
    let policy_path = policy_dir.join("capital_allocation_policy.json");
    std::fs::write(
        &policy_path,
        r#"{"schema_version":"policy-v1","policy_id":"lo03d-d5-policy","enabled":true,"max_portfolio_notional_usd":25000,"per_strategy_budgets":[]}"#,
    )
    .expect("D5: write policy file");
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &policy_path);

    let mut st_inner = state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    );
    st_inner.operator_auth = state::OperatorAuthMode::TokenRequired("lo03d-token".to_string());
    let st = Arc::new(st_inner);

    // Establish WS continuity as Live — proves cursor is anchored.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "test-d5-msg".to_string(),
        last_event_at: "2026-03-29T00:00:00Z".to_string(),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));

    // Arm with valid token.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .header(header::AUTHORIZATION, "Bearer lo03d-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(router, arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "D5: arm must succeed with valid token");

    // Start must reach the DB gate (503 — no DB configured).
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header(header::AUTHORIZATION, "Bearer lo03d-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let _ = std::fs::remove_dir_all(&policy_dir);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "D5: live-capital with all pre-DB gates satisfied must reach DB gate (503); \
         any 403 here would mean a gate was not satisfied; got: {status}"
    );
    let j = json(body);
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "D5: 503 must state DB is not configured — all pre-DB gates passed; got: {j}"
    );
    // Not a WS gate error — that would be 403 with gate=alpaca_ws_continuity.
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "alpaca_ws_continuity",
        "D5: WS continuity gate must have already passed (Live state set); got: {j}"
    );
    // Not an operator_auth gate error.
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "operator_auth",
        "D5: operator_auth gate must have already passed (TokenRequired set); got: {j}"
    );
    // Not a TV-04F capital policy gate error.
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "D5: TV-04F capital policy gate must have passed (policy was configured); got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03D-D6 — system/preflight for live-capital+paper is honest and
//              fail-closed
//
// The preflight surface must surface the deployment blocker explicitly
// (not silently pass or return null).  deployment_start_allowed must be false
// and the blockers list must be non-empty, identifying the adapter mismatch.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03d_d6_preflight_live_capital_paper_is_fail_closed() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Paper,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "D6: preflight must return 200; got: {status}");

    let j = json(body);
    assert_eq!(
        j["daemon_mode"], "live-capital",
        "D6: daemon_mode must be live-capital; got: {j}"
    );
    assert_eq!(
        j["deployment_start_allowed"],
        serde_json::Value::Bool(false),
        "D6: deployment_start_allowed must be false for live-capital+paper; got: {j}"
    );
    let blockers = j["blockers"].as_array().expect("D6: blockers must be an array");
    assert!(
        !blockers.is_empty(),
        "D6: blockers must be non-empty for live-capital+paper — adapter mismatch must be explicit; got: {j}"
    );
    // The deployment blocker must identify the adapter requirement.
    let has_adapter_blocker = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").to_lowercase().contains("broker") ||
                 b.as_str().unwrap_or("").to_lowercase().contains("adapter") ||
                 b.as_str().unwrap_or("").to_lowercase().contains("alpaca") ||
                 b.as_str().unwrap_or("").to_lowercase().contains("live-capital"));
    assert!(
        has_adapter_blocker,
        "D6: blockers must include a message about live-capital adapter requirement; got: {j}"
    );
}

// ===========================================================================
// LO-03E: Halt / disarm / cut-risk controls
// ===========================================================================

// ---------------------------------------------------------------------------
// LO-03E-E1 — live-capital halt without DB → 503
//
// Halt is a dangerous durable action.  It requires DB to persist the halted
// lifecycle state.  Without DB, halt cannot claim success — it returns 503.
//
// This proves halt requires durable authority.  An operator cannot silently
// halt live-capital execution without a backing database.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03e_e1_live_capital_halt_requires_db() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "E1: live-capital halt without DB must return 503 — halt requires durable authority; got: {status}"
    );
    let j = json(body);
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "E1: halt error must state DB is not configured; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03E-E2 — live-capital disarm without DB → 200
//
// Disarm is a safe control action.  It must always be reachable even when DB
// is unavailable.  The disarm only needs to reach the operator — it sets the
// in-memory gate, which is sufficient to prevent new executions.
//
// This proves a safe escape path always exists for live-capital: even if DB
// is unavailable, the operator can disarm to stop new order dispatch.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03e_e2_live_capital_disarm_always_reachable_without_db() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));

    // Arm first.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(router, arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "E2: arm must succeed");

    // Disarm must succeed even without DB.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), disarm_req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E2: live-capital disarm without DB must return 200 — safe control must always be reachable; got: {status}"
    );
    let j = json(body);
    assert_eq!(
        j["armed"],
        serde_json::Value::Bool(false),
        "E2: disarm must return armed=false; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03E-E3 — after halt (returning 503), kill-switch is active in-memory
//
// halt_execution_runtime sets integrity.halted=true BEFORE calling db_pool().
// Even when halt returns 503 (no DB), the in-memory kill-switch is engaged.
//
// This is a fail-safe property: the execution gate is activated even if the
// durable record cannot be written.  system/status must reflect this.
//
// An operator seeing a 503 halt response can confirm the in-memory kill-switch
// is active by reading system/status.  Restart clears the in-memory state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03e_e3_halt_without_db_sets_in_memory_kill_switch() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));

    // Issue halt — returns 503 without DB.
    let halt_req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (halt_status, _) = call(routes::build_router(Arc::clone(&st)), halt_req).await;
    assert_eq!(halt_status, StatusCode::SERVICE_UNAVAILABLE, "E3: halt must return 503 without DB");

    // Despite 503, system/status must reflect the in-memory kill-switch state.
    let status_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, status_body) = call(routes::build_router(Arc::clone(&st)), status_req).await;
    assert_eq!(status_code, StatusCode::OK, "E3: system/status must return 200");

    let j = json(status_body);
    assert_eq!(
        j["kill_switch_active"],
        serde_json::Value::Bool(true),
        "E3: kill_switch_active must be true after halt (even 503 halt); \
         halt is fail-safe — in-memory kill-switch engages before DB write; got: {j}"
    );
    assert_eq!(
        j["integrity_halt_active"],
        serde_json::Value::Bool(true),
        "E3: integrity_halt_active must be true after halt; got: {j}"
    );
    // Prove execution is blocked in-memory.
    assert_eq!(
        j["execution_armed"],
        serde_json::Value::Bool(false),
        "E3: execution_armed must be false after halt; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03E-E4 — no cut-risk route exists
//
// There is no separate "cut-risk" mechanism distinct from halt.  Any attempt
// to call a cut-risk route must return 404 (not found).
//
// This proves the architecture is explicit: halt is the capital kill switch.
// No shortcut, synthetic, or undocumented risk-cut path exists.  An operator
// managing live-capital risk must use the explicit halt mechanism.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03e_e4_no_cut_risk_route_exists() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    // Common paths an operator might guess for a cut-risk feature.
    let candidate_routes = [
        "/api/v1/cut-risk",
        "/v1/cut-risk",
        "/api/v1/risk/cut",
        "/v1/risk/cut",
        "/api/v1/risk/halt",
    ];

    for path in &candidate_routes {
        let req = Request::builder()
            .method("POST")
            .uri(*path)
            .body(axum::body::Body::empty())
            .unwrap();
        let router_clone = routes::build_router(Arc::new(state::AppState::new_for_test_with_mode_and_broker(
            state::DeploymentMode::LiveCapital,
            state::BrokerKind::Alpaca,
        )));
        let (status, _) = call(router_clone, req).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "E4: {path} must return 404 — no cut-risk route exists; \
             halt is the only capital kill switch; got: {status}"
        );
    }

    let _ = router; // suppress unused warning
}
