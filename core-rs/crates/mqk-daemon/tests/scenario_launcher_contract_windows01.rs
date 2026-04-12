//! WINDOWS-01 — Veritas Ledger Windows launcher daemon contract proof.
//!
//! ## What this file proves
//!
//! | Test  | Claim                                                                      |
//! |-------|----------------------------------------------------------------------------|
//! | W-01  | Observe path: all 6 GET daemon probes return the identity fields the       |
//! |       | launcher checks: `service`, `daemon_mode`, `adapter_id`,                  |
//! |       | `live_routing_enabled`, `deployment_start_allowed`,                        |
//! |       | `broker_config_present`, `autonomous_readiness_applicable`,                |
//! |       | `truth_state`, `canonical_path`, `signal_ingestion_configured`,            |
//! |       | `operator_auth_mode`.                                                      |
//! | W-02  | TradeReady auth probe: POST `/api/v1/ops/action` with the launcher         |
//! |       | sentinel `action_key` returns 400 + `disposition="unknown_action"` +       |
//! |       | `accepted=false` — proving Bearer auth without mutating state.             |
//! | W-03  | Wrong Bearer token returns 401; the launcher refuses to open the GUI.      |
//! | W-04  | `MissingTokenFailClosed` returns 503 + `gate=operator_auth_config`;        |
//! |       | the launcher refuses to open the GUI.                                      |
//!
//! ## Scope
//!
//! Pure in-process tests — no DB, no network.  These tests prove the daemon
//! *response contract*, not end-to-end operator execution.  End-to-end operator
//! execution requires a running binary with credentials: see
//! `docs/runbooks/operator_workflows.md`.
//!
//! ## Launcher probe sequence (for reference)
//!
//! The Veritas Launcher's `Get-BackendProbe` function (PowerShell) probes these
//! routes in order before opening the GUI:
//!
//! ```text
//! GET /v1/health                        — service identity
//! GET /api/v1/system/metadata           — daemon_mode + adapter_id
//! GET /api/v1/system/status             — runtime state + live_routing_enabled
//! GET /api/v1/system/session            — operator_auth_mode + deployment_start_allowed
//! GET /api/v1/system/preflight          — broker_config_present + autonomous_readiness_applicable
//! GET /api/v1/autonomous/readiness      — canonical_path + signal_ingestion_configured
//! POST /api/v1/ops/action               — Bearer auth probe (TradeReady mode only)
//! ```
//!
//! ## Known casing drift (documented, not a bug)
//!
//! `GET /api/v1/system/session` returns `daemon_mode: "PAPER"` (uppercase) via
//! `as_db_mode()`, while all other endpoints return `"paper"` (lowercase) via
//! `as_api_label()`.  The Veritas Launcher uses PowerShell's case-insensitive
//! `-ne` comparison operator (not `-cne`), so `"PAPER" -ne "paper"` evaluates
//! to `$false` (they match) and the launcher does not block.  W-01 documents
//! this via a `to_ascii_lowercase()` assertion with an explanatory comment.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_launcher_contract_windows01
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{BrokerKind, OperatorAuthMode};
use tower::ServiceExt;

/// Sentinel `action_key` sent by the Veritas Launcher's Bearer auth probe.
///
/// This value is intentionally not a real action so the probe can verify
/// Bearer auth without mutating runtime state.  The `ops_action` handler's
/// `_ =>` arm returns the exact 400 + `unknown_action` contract the launcher
/// validates.
const LAUNCHER_AUTH_PROBE_KEY: &str = "__veritas_launcher_auth_probe__";

/// Test operator token.  Any non-empty string — the daemon validates token
/// presence and equality, not content.
const TEST_TOKEN: &str = "veritas-test-token";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Paper+Alpaca daemon state with an explicit operator token.
///
/// This is the canonical deployment configuration the Veritas Launcher expects:
/// `MQK_DAEMON_DEPLOYMENT_MODE=paper`, `MQK_DAEMON_ADAPTER_ID=alpaca`, and
/// `MQK_OPERATOR_TOKEN` present.
fn make_paper_alpaca_with_token() -> Arc<state::AppState> {
    let mut st = state::AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st.operator_auth = OperatorAuthMode::TokenRequired(TEST_TOKEN.to_string());
    Arc::new(st)
}

/// Paper+Alpaca daemon state with `MissingTokenFailClosed` — operator token
/// absent from the environment.  This represents a mis-configured daemon that
/// the launcher must refuse to attach to.
fn make_paper_alpaca_missing_token() -> Arc<state::AppState> {
    let mut st = state::AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st.operator_auth = OperatorAuthMode::MissingTokenFailClosed;
    Arc::new(st)
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

// ---------------------------------------------------------------------------
// W-01: Observe identity chain
// ---------------------------------------------------------------------------

/// Proves the full set of launcher-checked identity fields across all 6 GET
/// endpoints that the Veritas Launcher probes in both Observe and TradeReady
/// modes.
///
/// The launcher's `Get-BackendProbe` function checks these fields in order and
/// refuses to attach (returns `IdentityVerified=false`) if any diverge.  This
/// test proves the daemon contract is satisfied for the canonical paper+alpaca
/// path so the Veritas Launcher can proceed past the identity gate.
#[tokio::test]
async fn w01_observe_identity_chain_satisfies_launcher_contract() {
    let st = make_paper_alpaca_with_token();
    let router = routes::build_router(st);

    // --- Probe 1: GET /v1/health ---
    // Launcher checks: health.service == 'mqk-daemon'
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/v1/health must return 200");
    let health = parse_json(body);
    assert_eq!(
        health["service"], "mqk-daemon",
        "launcher checks health.service == 'mqk-daemon'"
    );
    assert_eq!(health["ok"], true);

    // --- Probe 2: GET /api/v1/system/metadata ---
    // Launcher checks: metadata.daemon_mode == 'paper', metadata.adapter_id == 'alpaca'
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/api/v1/system/metadata")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/api/v1/system/metadata must return 200");
    let metadata = parse_json(body);
    assert_eq!(
        metadata["daemon_mode"], "paper",
        "launcher checks metadata.daemon_mode == 'paper'"
    );
    assert_eq!(
        metadata["adapter_id"], "alpaca",
        "launcher checks metadata.adapter_id == 'alpaca'"
    );

    // --- Probe 3: GET /api/v1/system/status ---
    // Launcher checks: daemon_mode, adapter_id, live_routing_enabled=false,
    //                  deployment_start_allowed=true
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/api/v1/system/status")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/api/v1/system/status must return 200");
    let sys_status = parse_json(body);
    assert_eq!(
        sys_status["daemon_mode"], "paper",
        "launcher checks status.daemon_mode == 'paper'"
    );
    assert_eq!(
        sys_status["adapter_id"], "alpaca",
        "launcher checks status.adapter_id == 'alpaca'"
    );
    assert_eq!(
        sys_status["live_routing_enabled"], false,
        "launcher refuses to attach if live_routing_enabled=true; must be false for idle paper daemon"
    );
    assert_eq!(
        sys_status["deployment_start_allowed"], true,
        "launcher checks deployment_start_allowed is consistently true across status/session/preflight"
    );

    // --- Probe 4: GET /api/v1/system/session ---
    // Launcher checks: daemon_mode, adapter_id, deployment_start_allowed=true,
    //                  operator_auth_mode == 'token_required'
    //
    // CASING DRIFT NOTE: session returns daemon_mode via as_db_mode() → "PAPER"
    // (uppercase), while other endpoints use as_api_label() → "paper" (lowercase).
    // The Veritas Launcher uses PowerShell's case-insensitive -ne comparison, so
    // "PAPER" -ne "paper" evaluates to $false (they match). This drift is safe for
    // the launcher but documented here so future callers do not assume consistent
    // casing across daemon surfaces.
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/api/v1/system/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/api/v1/system/session must return 200");
    let session = parse_json(body);
    let session_daemon_mode = session["daemon_mode"]
        .as_str()
        .expect("session.daemon_mode must be a string");
    assert_eq!(
        session_daemon_mode.to_ascii_lowercase(),
        "paper",
        "launcher checks session.daemon_mode == 'paper' (case-insensitive via PowerShell -ne); \
         actual value returned: '{session_daemon_mode}'"
    );
    assert_eq!(
        session["adapter_id"], "alpaca",
        "launcher checks session.adapter_id == 'alpaca'"
    );
    assert_eq!(
        session["deployment_start_allowed"], true,
        "launcher checks session.deployment_start_allowed == true"
    );
    assert_eq!(
        session["operator_auth_mode"], "token_required",
        "launcher requires operator_auth_mode == 'token_required' to proceed"
    );

    // --- Probe 5: GET /api/v1/system/preflight ---
    // Launcher checks: daemon_mode, adapter_id, deployment_start_allowed=true,
    //                  broker_config_present=true, autonomous_readiness_applicable=true
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/api/v1/system/preflight must return 200");
    let preflight = parse_json(body);
    assert_eq!(
        preflight["daemon_mode"], "paper",
        "launcher checks preflight.daemon_mode == 'paper'"
    );
    assert_eq!(
        preflight["adapter_id"], "alpaca",
        "launcher checks preflight.adapter_id == 'alpaca'"
    );
    assert_eq!(
        preflight["deployment_start_allowed"], true,
        "launcher checks preflight.deployment_start_allowed == true"
    );
    assert_eq!(
        preflight["broker_config_present"], true,
        "launcher refuses to attach if broker_config_present=false; alpaca adapter_id drives true"
    );
    assert_eq!(
        preflight["autonomous_readiness_applicable"], true,
        "launcher refuses to attach if autonomous_readiness_applicable=false; must be true for \
         paper+alpaca (ExternalSignalIngestion configured)"
    );

    // --- Probe 6: GET /api/v1/autonomous/readiness ---
    // Launcher checks: truth_state == 'active', canonical_path=true,
    //                  signal_ingestion_configured=true
    let (status, body) = call(
        router.clone(),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/v1/autonomous/readiness must return 200"
    );
    let readiness = parse_json(body);
    assert_eq!(
        readiness["truth_state"], "active",
        "launcher requires autonomous_readiness.truth_state == 'active'"
    );
    assert_eq!(
        readiness["canonical_path"], true,
        "launcher requires autonomous_readiness.canonical_path == true"
    );
    assert_eq!(
        readiness["signal_ingestion_configured"], true,
        "launcher requires autonomous_readiness.signal_ingestion_configured == true"
    );
}

// ---------------------------------------------------------------------------
// W-02: TradeReady auth probe
// ---------------------------------------------------------------------------

/// Proves the TradeReady auth probe contract.
///
/// The Veritas Launcher's `Get-BackendProbe` function (TradeReady mode) sends
/// `POST /api/v1/ops/action { "action_key": "__veritas_launcher_auth_probe__" }`
/// with a valid Bearer token, then validates the response:
///
/// - HTTP 400 (not 401 or 503)
/// - `disposition == "unknown_action"`
/// - `accepted == false`
///
/// This proves Bearer auth passed the middleware without mutating runtime state.
/// The `ops_action` handler's `_ =>` arm handles any unknown `action_key` with
/// this exact response, making the sentinel key a stateless identity proof.
#[tokio::test]
async fn w02_trade_ready_auth_probe_returns_400_unknown_action_accepted_false() {
    let st = make_paper_alpaca_with_token();
    let router = routes::build_router(st);

    let body_bytes = serde_json::to_vec(&serde_json::json!({
        "action_key": LAUNCHER_AUTH_PROBE_KEY
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("Authorization", format!("Bearer {TEST_TOKEN}"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body_bytes))
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "auth probe must return 400 Bad Request for unknown action_key; \
         401 would mean auth failed, 503 would mean auth not configured"
    );
    let resp = parse_json(body);
    assert_eq!(
        resp["disposition"], "unknown_action",
        "launcher checks disposition == 'unknown_action'"
    );
    assert_eq!(
        resp["accepted"], false,
        "launcher checks accepted == false"
    );
}

// ---------------------------------------------------------------------------
// W-03: Wrong token is rejected
// ---------------------------------------------------------------------------

/// Proves that a wrong Bearer token returns 401 on the auth probe route.
///
/// The launcher handles HTTP 401 as a hard failure: "operator token was
/// rejected by the daemon" — and refuses to open the GUI.  This test proves
/// the daemon correctly rejects mismatched tokens so the launcher's fail-closed
/// handling is exercised against a real contract response.
#[tokio::test]
async fn w03_wrong_bearer_token_returns_401() {
    let st = make_paper_alpaca_with_token();
    let router = routes::build_router(st);

    let body_bytes = serde_json::to_vec(&serde_json::json!({
        "action_key": LAUNCHER_AUTH_PROBE_KEY
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("Authorization", "Bearer wrong-token-not-veritas-test-token")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body_bytes))
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "wrong token must return 401; launcher refuses with \
         'operator token was rejected by the daemon'"
    );
    let resp = parse_json(body);
    assert_eq!(
        resp["gate"], "operator_token",
        "401 must declare operator_token gate"
    );
}

// ---------------------------------------------------------------------------
// W-04: MissingTokenFailClosed → 503
// ---------------------------------------------------------------------------

/// Proves that with `MissingTokenFailClosed` (operator token not configured),
/// the auth probe route returns 503 + `gate=operator_auth_config`.
///
/// The launcher handles HTTP 503 as: "daemon operator routes are fail-closed
/// because operator auth is not fully configured" — and refuses to open the
/// GUI.  This test proves the daemon's fail-closed posture is correctly surfaced
/// so the launcher's specific 503 handling path is exercised against a real
/// contract response.
#[tokio::test]
async fn w04_missing_token_fail_closed_returns_503_operator_auth_config() {
    let st = make_paper_alpaca_missing_token();
    let router = routes::build_router(st);

    let body_bytes = serde_json::to_vec(&serde_json::json!({
        "action_key": LAUNCHER_AUTH_PROBE_KEY
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body_bytes))
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "missing operator token must return 503; launcher refuses with \
         'daemon operator routes fail-closed because operator auth is not fully configured'"
    );
    let resp = parse_json(body);
    assert_eq!(
        resp["gate"], "operator_auth_config",
        "503 must declare operator_auth_config gate so the launcher's specific error message fires"
    );
}
