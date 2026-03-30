//! TV-04F: Live vs paper capital semantics proof.
//!
//! Proves that live-capital and paper/live-shadow have explicitly distinct
//! capital allocation policy semantics at the runtime start boundary:
//!
//! - **Paper and LiveShadow**: absent capital policy → `NotConfigured` →
//!   gate not applicable; start proceeds.  Operators may choose not to
//!   configure a policy for simulation modes.
//!
//! - **LiveCapital**: absent capital policy → `live_capital_policy_required`
//!   → 403 fail-closed.  Real capital execution requires an explicit,
//!   operator-configured policy before start is authorized.
//!
//! The key distinction proven here:
//!
//! > **paper-safe "no policy = no enforcement" ≠ live-capital authorization**
//!
//! A live-capital deployment without a policy is fail-closed at the TV-04F
//! gate before reaching TV-04A (policy validity) or TV-04D (economics).
//!
//! # Gate ordering for live-capital at the start boundary
//!
//! ```
//! deployment_readiness     → passes (LiveCapital+Alpaca)
//! integrity_armed          → passes (armed below)
//! operator_auth            → passes (TokenRequired set)
//! WS continuity            → passes (Live forced in test setup)
//! TV-02C artifact gate     → not triggered (no MQK_ARTIFACT_PATH)
//! TV-03C parity gate       → not triggered (no MQK_ARTIFACT_PATH)
//! TV-04F live-capital policy required  ← what F02/F03/F04 exercise
//! TV-04A capital policy validity       → fires after TV-04F confirms presence
//! TV-04D deployment economics          → fires after TV-04A confirms validity
//! db_pool()                → ServiceUnavailable (no DB in test) → 503
//! ```
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                      |
//! |------|-------------------------------------------------------------------------------------|
//! | F01  | LiveShadow+no policy → 503 DB gate (gate not applicable for non-LiveCapital)        |
//! | F02  | LiveCapital+no policy → 403 live_capital_policy_required (key proof)                |
//! | F03  | LiveCapital+valid policy → 503 DB gate (TV-04F passes, chain complete)              |
//! | F04  | LiveCapital+invalid policy (enabled=false) → 403 capital_allocation_policy          |
//!         (TV-04F passes since policy IS configured; TV-04A fires for disabled policy)  |
//! | F05  | Semantic separation: same absent policy, different mode → different outcome          |
//!         (LiveShadow → 503, LiveCapital → 403; explicit semantic distinction proof)     |
//!
//! All tests are pure in-process (no DB, no real broker, no network).
//! All tests are always runnable in CI without MQK_DATABASE_URL.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialisation — protects MQK_CAPITAL_POLICY_PATH mutations
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// HTTP helpers
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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Minimal valid capital allocation policy (enabled=true + economics bound).
///
/// Passes TV-04F (configured), TV-04A (authorized), and TV-04D (economics).
fn valid_policy(policy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": true,
  "max_portfolio_notional_usd": 25000,
  "per_strategy_budgets": []
}}"#
    )
}

/// Policy with enabled=false.
///
/// Passes TV-04F (policy IS configured) but fails TV-04A (Denied; enabled=false).
fn disabled_policy(policy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": false,
  "max_portfolio_notional_usd": 25000,
  "per_strategy_budgets": []
}}"#
    )
}

/// Write a policy file to a unique temp dir.  Returns `(policy_path, dir)`.
fn write_policy_dir(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04f_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create policy dir");
    let policy_path = dir.join("capital_allocation_policy.json");
    std::fs::write(&policy_path, contents).expect("write policy file");
    (policy_path, dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// State helpers
// ---------------------------------------------------------------------------

/// Armed LiveShadow+Alpaca state — used for F01/F05 (non-LiveCapital).
///
/// Gate ordering: deployment → pass, integrity → pass (armed), no WS gate,
/// no operator_auth gate, TV-04F not applicable → db_pool (503).
async fn armed_live_shadow_state() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "test setup: arm must succeed");
    st
}

/// Armed LiveCapital+Alpaca+TokenRequired+WS=Live state — used for F02/F03/F04/F05.
///
/// Gate ordering: deployment → pass, integrity → pass (armed),
/// operator_auth → pass (TokenRequired), WS continuity → pass (Live),
/// TV-04F → depends on MQK_CAPITAL_POLICY_PATH, TV-04A/D → depend on policy file,
/// db_pool → 503 when all pre-DB gates pass.
async fn armed_live_capital_state(token: &str) -> Arc<state::AppState> {
    let mut st_inner = state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    );
    st_inner.operator_auth = state::OperatorAuthMode::TokenRequired(token.to_string());
    let st = Arc::new(st_inner);

    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "tv04f-test-msg".to_string(),
        last_event_at: "2026-03-29T00:00:00Z".to_string(),
    })
    .await;

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "test setup: arm must succeed");
    st
}

fn post_start(token: Option<&str>) -> Request<axum::body::Body> {
    let mut builder = Request::builder().method("POST").uri("/v1/run/start");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

// ===========================================================================
// TV-04F — Proof tests
// ===========================================================================

/// F01: LiveShadow + no capital policy → proceeds to db_pool() (503).
///
/// Proves the TV-04F gate does NOT fire for non-LiveCapital modes.
/// LiveShadow is permissive: absent policy = gate not applicable.
#[tokio::test]
async fn f01_live_shadow_without_policy_proceeds_to_db_gate() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start(None)).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "F01: LiveShadow without capital policy must reach DB gate (503); \
         TV-04F must not fire for non-LiveCapital modes; got: {status}"
    );
    let j = parse_json(body);
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F01: TV-04F gate must not fire for LiveShadow; got: {j}"
    );
}

/// F02: LiveCapital + no capital policy → 403 live_capital_policy_required.
///
/// Key proof: live-capital without an explicit capital policy is fail-closed.
/// The TV-04F gate fires before TV-04A (which would have been a pass-through).
#[tokio::test]
async fn f02_live_capital_without_policy_blocked_at_tv04f_gate() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let token = "tv04f-f02-token";
    let st = armed_live_capital_state(token).await;
    let (status, body) = call(routes::build_router(st), post_start(Some(token))).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "F02: LiveCapital without capital policy must be 403 at TV-04F gate; got: {status}"
    );
    let j = parse_json(body);
    assert_eq!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F02: gate must be live_capital_policy_required; got: {j}"
    );
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "alpaca_ws_continuity",
        "F02: WS continuity gate must have passed before TV-04F; got: {j}"
    );
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "operator_auth",
        "F02: operator_auth gate must have passed before TV-04F; got: {j}"
    );
}

/// F03: LiveCapital + valid capital policy → proceeds to db_pool() (503).
///
/// Proves that a live-capital start with a fully configured, enabled, and
/// economics-specified policy passes all pre-DB gates (TV-04F, TV-04A, TV-04D)
/// and reaches the DB gate.
#[tokio::test]
async fn f03_live_capital_with_valid_policy_proceeds_to_db_gate() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("f03", &valid_policy("tv04f-f03-policy"));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let token = "tv04f-f03-token";
    let st = armed_live_capital_state(token).await;
    let (status, body) = call(routes::build_router(st), post_start(Some(token))).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "F03: LiveCapital with valid policy must reach DB gate (503); \
         all pre-DB gates (TV-04F + TV-04A + TV-04D) must pass; got: {status}"
    );
    let j = parse_json(body);
    let error = j["error"].as_str().unwrap_or("");
    assert!(
        error.contains("runtime DB is not configured") || error.contains("DB"),
        "F03: 503 body must describe DB unavailability (all pre-DB gates passed); got: {j}"
    );
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F03: TV-04F gate must not fire when a valid policy is configured; got: {j}"
    );
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "capital_allocation_policy",
        "F03: TV-04A gate must not fire when policy is authorized; got: {j}"
    );
}

/// F04: LiveCapital + disabled policy (enabled=false) → 403 capital_allocation_policy.
///
/// Proves the gates are sequential:
/// - TV-04F: passes — the policy IS configured (not NotConfigured)
/// - TV-04A: fires — the policy is Denied (enabled=false)
///
/// This proves TV-04F is a presence check, not a validity check.
/// TV-04A handles validity once TV-04F confirms the policy exists.
#[tokio::test]
async fn f04_live_capital_disabled_policy_passes_tv04f_blocked_at_tv04a() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("f04", &disabled_policy("tv04f-f04-disabled"));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let token = "tv04f-f04-token";
    let st = armed_live_capital_state(token).await;
    let (status, body) = call(routes::build_router(st), post_start(Some(token))).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "F04: LiveCapital with disabled policy must be 403 at TV-04A gate; got: {status}"
    );
    let j = parse_json(body);
    assert_eq!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "capital_allocation_policy",
        "F04: gate must be capital_allocation_policy (TV-04A), not live_capital_policy_required \
         (TV-04F); disabled policy passes TV-04F (configured) but fails TV-04A; got: {j}"
    );
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F04: TV-04F must not fire when a policy IS configured (even if disabled); got: {j}"
    );
}

/// F05: Semantic separation proof.
///
/// Given the same absent capital policy:
///   - LiveShadow → 503 (permissive; gate not applicable)
///   - LiveCapital → 403 live_capital_policy_required (fail-closed)
///
/// This proves paper/live-shadow safety (no-policy = not enforced) and
/// live-capital authorization (no-policy = explicitly blocked) are semantically
/// distinct at the start boundary.  The same operator configuration yields
/// different outcomes by design.
#[tokio::test]
async fn f05_semantic_separation_paper_safety_vs_live_capital_authorization() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let live_shadow_st = armed_live_shadow_state().await;
    let (shadow_status, shadow_body) =
        call(routes::build_router(live_shadow_st), post_start(None)).await;
    let shadow_j = parse_json(shadow_body);

    let token = "tv04f-f05-token";
    let live_capital_st = armed_live_capital_state(token).await;
    let (capital_status, capital_body) = call(
        routes::build_router(live_capital_st),
        post_start(Some(token)),
    )
    .await;
    let capital_j = parse_json(capital_body);

    assert_eq!(
        shadow_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "F05 (shadow): LiveShadow without policy must reach DB gate (503); \
         live-shadow is permissive — absent policy is not an error; got: {shadow_status}"
    );
    assert_ne!(
        shadow_j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F05 (shadow): TV-04F must not fire for LiveShadow; got: {shadow_j}"
    );

    assert_eq!(
        capital_status,
        StatusCode::FORBIDDEN,
        "F05 (capital): LiveCapital without policy must be 403 at TV-04F gate; \
         live-capital is fail-closed — absent policy must block start; got: {capital_status}"
    );
    assert_eq!(
        capital_j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "live_capital_policy_required",
        "F05 (capital): TV-04F gate must fire for LiveCapital without policy; got: {capital_j}"
    );

    assert_ne!(
        shadow_status, capital_status,
        "F05: absent capital policy must produce DIFFERENT outcomes for LiveShadow vs \
         LiveCapital — this is the explicit semantic distinction between paper safety and \
         live-capital authorization"
    );
}
