//! TV-04D: Deployment economics gate at the runtime start boundary.
//!
//! Proves the repo can truthfully distinguish between:
//!
//! - A capital policy that is enabled (TV-04A passes).
//! - A capital policy that is enabled AND specifies valid portfolio-level
//!   economics bounds (`max_portfolio_notional_usd`).
//!
//! The key distinction proven here:
//!
//! > **policy enabled ≠ deployment economics specified**
//!
//! A policy with `enabled=true` can still be refused at the start boundary
//! because `max_portfolio_notional_usd` is absent or not a positive number.
//!
//! # Design
//!
//! The deployment economics gate (TV-04D) is placed immediately after the
//! capital policy gate (TV-04A) and before `db_pool()`.  It is a pure
//! filesystem check and requires no database.
//!
//! All tests require no database and no network.
//!
//! # Proof matrix
//!
//! ## TV-04D — start boundary (HTTP)
//!
//! | Test | What it proves                                                                        |
//! |------|---------------------------------------------------------------------------------------|
//! | D01  | No policy → not configured → proceeds past TV-04D to db_pool() (503)                 |
//! | D02  | Policy enabled + max_portfolio_notional_usd present → proceeds to db_pool() (503)    |
//! | D03  | Policy enabled + max_portfolio_notional_usd absent → 403 deployment_economics        |
//! | D04  | Policy enabled + max_portfolio_notional_usd = 0 → 403 deployment_economics           |
//! | D05  | Key proof: TV-04A passes (enabled=true) but TV-04D fails (no economics) — independent|
//!
//! ## TV-04D — pure evaluator
//!
//! | Test | What it proves                                                                        |
//! |------|---------------------------------------------------------------------------------------|
//! | D06  | Pure: None path → NotConfigured                                                       |
//! | D07  | Pure: enabled=false → PolicyDisabled (is_start_safe=true)                             |
//! | D08  | Pure: enabled=true + max_portfolio_notional_usd present → EconomicsSpecified          |
//! | D09  | Pure: enabled=true + max_portfolio_notional_usd absent → EconomicsNotSpecified        |
//! | D10  | Pure: enabled=true + max_portfolio_notional_usd = 0 → EconomicsNotSpecified           |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    capital_policy::{
        evaluate_capital_policy, evaluate_deployment_economics, CapitalPolicyOutcome,
        DeploymentEconomicsOutcome,
    },
    routes, state,
};
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

/// Policy with enabled=true and a valid max_portfolio_notional_usd.
///
/// Both TV-04A and TV-04D pass on this fixture.
fn policy_with_economics(policy_id: &str) -> String {
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

/// Policy with enabled=true but NO max_portfolio_notional_usd field.
///
/// TV-04A: Authorized (enabled=true).
/// TV-04D: EconomicsNotSpecified → fail-closed.
fn policy_without_economics(policy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": true,
  "per_strategy_budgets": []
}}"#
    )
}

/// Policy with enabled=true and max_portfolio_notional_usd = 0 (invalid bound).
///
/// TV-04A: Authorized (enabled=true).
/// TV-04D: EconomicsNotSpecified → fail-closed.
fn policy_with_zero_economics(policy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": true,
  "max_portfolio_notional_usd": 0,
  "per_strategy_budgets": []
}}"#
    )
}

/// Write a policy file to a temp dir.  Returns `(policy_path, dir)`.
fn write_policy_dir(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04d_{tag}_{}_{}",
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

/// Build an armed LiveShadow+Alpaca AppState for start-boundary tests.
///
/// Gate ordering for this state:
///   1. deployment_readiness       → passes (LiveShadow+Alpaca is valid)
///   2. integrity_armed            → passes (armed below)
///   3. [paper WS / live-capital gates] → not applicable for LiveShadow
///   4. TV-02C artifact deployability  → not triggered (no MQK_ARTIFACT_PATH)
///   5. TV-03C parity evidence         → not triggered (no MQK_PARITY_EVIDENCE_PATH)
///   6. TV-04A capital policy          → passes or not triggered
///   7. **TV-04D deployment economics** ← what D01..D05 exercise
///   8. db_pool()                      → ServiceUnavailable (no DB in test)
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

async fn post_start(st: Arc<state::AppState>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);
    (status, json)
}

// ===========================================================================
// TV-04D — Start boundary tests (HTTP)
// ===========================================================================

/// D01: No policy configured → TV-04D not applicable → proceeds to db_pool() (503).
///
/// Proves TV-04D does not fire when no policy is configured.
#[tokio::test]
async fn d01_no_policy_economics_not_applicable_proceeds_to_db_gate() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    // DB gate fires (no DB in test) → 503.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_ne!(
        gate, "deployment_economics",
        "economics gate must not fire when no policy is configured; got: {json}"
    );
}

/// D02: Policy enabled + max_portfolio_notional_usd present → TV-04D passes →
///      proceeds to db_pool() (503).
///
/// Proves a fully-specified economics policy is not blocked by TV-04D.
#[tokio::test]
async fn d02_policy_with_economics_proceeds_to_db_gate() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (path, dir) = write_policy_dir("d02", &policy_with_economics("tv04d-d02"));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_ne!(
        gate, "deployment_economics",
        "economics gate must not fire when economics are fully specified; got: {json}"
    );
}

/// D03: Policy enabled + max_portfolio_notional_usd absent → 403 deployment_economics.
///
/// Proves an enabled policy without a portfolio economics bound is refused.
#[tokio::test]
async fn d03_policy_without_economics_refused_at_start() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (path, dir) = write_policy_dir("d03", &policy_without_economics("tv04d-d03"));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "deployment_economics",
        "economics gate must fire when max_portfolio_notional_usd is absent; got: {json}"
    );
}

/// D04: Policy enabled + max_portfolio_notional_usd = 0 → 403 deployment_economics.
///
/// Proves a zero economics bound is treated as absent (not a valid bound).
#[tokio::test]
async fn d04_policy_with_zero_economics_refused_at_start() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (path, dir) = write_policy_dir("d04", &policy_with_zero_economics("tv04d-d04"));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "deployment_economics",
        "economics gate must fire when max_portfolio_notional_usd = 0; got: {json}"
    );
}

/// D05: Key independence proof.
///
/// TV-04A: Authorized (enabled=true, valid structure) — passes.
/// TV-04D: EconomicsNotSpecified (no max_portfolio_notional_usd) — fails with 403.
///
/// Proves the two gates are independent: capital policy authorization does not
/// imply deployment economics authorization.
#[tokio::test]
async fn d05_tv04a_passes_but_tv04d_fails_independent_gates() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let contents = policy_without_economics("tv04d-d05");
    let (path, dir) = write_policy_dir("d05", &contents);
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    // Confirm TV-04A would pass on this same policy file.
    let tv04a = evaluate_capital_policy(Some(&path));
    assert!(
        matches!(tv04a, CapitalPolicyOutcome::Authorized { .. }),
        "TV-04A must pass on enabled policy; got: {tv04a:?}"
    );

    // Confirm TV-04D fails on the same file.
    let tv04d = evaluate_deployment_economics(Some(&path));
    assert!(
        matches!(
            tv04d,
            DeploymentEconomicsOutcome::EconomicsNotSpecified { .. }
        ),
        "TV-04D must fail on policy without max_portfolio_notional_usd; got: {tv04d:?}"
    );
    assert!(
        !tv04d.is_start_safe(),
        "EconomicsNotSpecified must not be start-safe"
    );

    // HTTP proof: start is refused at the economics gate (not the policy gate).
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "deployment_economics",
        "start must be refused at deployment_economics gate, not capital_allocation_policy; \
         got: {json}"
    );
}

// ===========================================================================
// TV-04D — Pure evaluator tests
// ===========================================================================

/// D06: None path → NotConfigured.
#[test]
fn d06_pure_none_path_returns_not_configured() {
    let outcome = evaluate_deployment_economics(None);
    assert_eq!(outcome, DeploymentEconomicsOutcome::NotConfigured);
    assert!(outcome.is_start_safe());
}

/// D07: enabled=false → PolicyDisabled (is_start_safe=true; TV-04A handles refusal).
#[test]
fn d07_pure_disabled_policy_returns_policy_disabled() {
    let id = next_id();
    let dir = std::env::temp_dir().join(format!("mqk_tv04d_d07_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"policy-v1","policy_id":"disabled-policy","enabled":false,"max_portfolio_notional_usd":25000,"per_strategy_budgets":[]}"#,
    )
    .unwrap();

    let outcome = evaluate_deployment_economics(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(outcome, DeploymentEconomicsOutcome::PolicyDisabled);
    assert!(
        outcome.is_start_safe(),
        "PolicyDisabled must be start-safe (TV-04A handles the refusal)"
    );
}

/// D08: enabled=true + max_portfolio_notional_usd = 25000 → EconomicsSpecified.
#[test]
fn d08_pure_valid_economics_returns_economics_specified() {
    let id = next_id();
    let dir = std::env::temp_dir().join(format!("mqk_tv04d_d08_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"policy-v1","policy_id":"econ-policy","enabled":true,"max_portfolio_notional_usd":25000,"per_strategy_budgets":[]}"#,
    )
    .unwrap();

    let outcome = evaluate_deployment_economics(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(
            &outcome,
            DeploymentEconomicsOutcome::EconomicsSpecified {
                max_portfolio_notional_usd,
                ..
            } if (*max_portfolio_notional_usd - 25000.0).abs() < 0.01
        ),
        "must return EconomicsSpecified with correct cap; got: {outcome:?}"
    );
    assert!(outcome.is_start_safe());
}

/// D09: enabled=true + max_portfolio_notional_usd absent → EconomicsNotSpecified.
#[test]
fn d09_pure_missing_economics_returns_economics_not_specified() {
    let id = next_id();
    let dir = std::env::temp_dir().join(format!("mqk_tv04d_d09_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"policy-v1","policy_id":"no-econ-policy","enabled":true,"per_strategy_budgets":[]}"#,
    )
    .unwrap();

    let outcome = evaluate_deployment_economics(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(
            outcome,
            DeploymentEconomicsOutcome::EconomicsNotSpecified { .. }
        ),
        "must return EconomicsNotSpecified when max_portfolio_notional_usd is absent; \
         got: {outcome:?}"
    );
    assert!(!outcome.is_start_safe());
}

/// D10: enabled=true + max_portfolio_notional_usd = 0 → EconomicsNotSpecified.
#[test]
fn d10_pure_zero_economics_returns_economics_not_specified() {
    let id = next_id();
    let dir = std::env::temp_dir().join(format!("mqk_tv04d_d10_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"policy-v1","policy_id":"zero-econ-policy","enabled":true,"max_portfolio_notional_usd":0,"per_strategy_budgets":[]}"#,
    )
    .unwrap();

    let outcome = evaluate_deployment_economics(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(
            outcome,
            DeploymentEconomicsOutcome::EconomicsNotSpecified { .. }
        ),
        "must return EconomicsNotSpecified when max_portfolio_notional_usd = 0; \
         got: {outcome:?}"
    );
    assert!(!outcome.is_start_safe());
}
