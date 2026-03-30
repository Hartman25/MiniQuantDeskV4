//! TV-04A / TV-04B: Capital allocation policy and strategy budget enforcement.
//!
//! TV-04A: Portfolio-level capital allocation policy seam wired into
//!         `start_execution_runtime` before DB operations.
//! TV-04B: Per-strategy budget / risk-bucket enforcement wired into the
//!         `POST /api/v1/strategy/signal` gate sequence before DB operations.
//!
//! # What this proves
//!
//! The repo can now truthfully say:
//! - There is a concrete portfolio-level capital allocation policy seam.
//! - Per-strategy budget authorization is enforced as a separate truth from
//!   deployability and strategy enablement.
//! - A strategy can be deployable and enabled yet still be refused at the
//!   control/runtime seam because it is not capital-authorized.
//! - Absent budget entry is not authorization (fail-closed).
//! - Policy absent means "not enforced", not "silently authorized".
//!
//! # Proof matrix
//!
//! ## TV-04A — start boundary
//!
//! | Test | What it proves                                                                  |
//! |------|---------------------------------------------------------------------------------|
//! | A01  | No policy configured → gate not applicable → start proceeds to DB gate (503)   |
//! | A02  | Policy present + enabled=true → start proceeds to DB gate (503)                |
//! | A03  | Policy present + enabled=false → 403 blocked: capital_allocation_policy        |
//! | A04  | Policy file has invalid JSON → 403 blocked: capital_allocation_policy          |
//! | A05  | Policy path configured but file missing → 403 blocked: capital_allocation_policy|
//!
//! ## TV-04A — pure evaluator
//!
//! | Test | What it proves                                                                  |
//! |------|---------------------------------------------------------------------------------|
//! | A06  | Pure: NotConfigured path → NotConfigured outcome                                |
//! | A07  | Pure: enabled=false → Denied with reason containing policy_id                  |
//! | A08  | Pure: invalid schema_version → PolicyInvalid                                   |
//! | A09  | Pure: enabled=true → Authorized carrying policy_id                             |
//! | A10  | Pure: missing policy_id → PolicyInvalid                                        |
//!
//! ## TV-04B — signal boundary (pre-DB gate)
//!
//! | Test | What it proves                                                                  |
//! |------|---------------------------------------------------------------------------------|
//! | B01  | No policy configured → budget not enforced → proceeds to Gate 1b (503 WS)     |
//! | B02  | Policy + strategy budget_authorized=true → proceeds to Gate 1b (503 WS)       |
//! | B03  | Policy + strategy absent → 403 budget_denied before DB gate                   |
//! | B04  | Policy + strategy budget_authorized=false → 403 budget_denied before DB gate   |
//! | B05  | Policy invalid → 503 unavailable before DB gate                               |
//!
//! ## TV-04B — pure evaluator
//!
//! | Test | What it proves                                                                  |
//! |------|---------------------------------------------------------------------------------|
//! | B06  | Pure: strategy not in policy → BudgetDenied (absent entry ≠ authorized)       |
//! | B07  | Pure: strategy budget_authorized=true → BudgetAuthorized with risk_bucket      |
//! | B08  | Pure: strategy budget_authorized=false + deny_reason → BudgetDenied with reason|
//! | B09  | Pure: policy enabled=false → BudgetDenied                                     |
//! | B10  | Pure: per_strategy_budgets absent → BudgetDenied (absent array ≠ authorized)  |
//!
//! All tests require no database and no network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    capital_policy::{
        evaluate_capital_policy, evaluate_strategy_budget, CapitalPolicyOutcome,
        StrategyBudgetOutcome,
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

/// Minimal valid policy file content.
fn valid_policy(policy_id: &str, enabled: bool) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": {enabled},
  "max_portfolio_notional_usd": 25000,
  "per_strategy_budgets": []
}}"#
    )
}

/// Policy file with a strategy budget entry.
fn policy_with_strategy(
    policy_id: &str,
    strategy_id: &str,
    budget_authorized: bool,
    deny_reason: Option<&str>,
) -> String {
    let deny_field = if let Some(r) = deny_reason {
        format!(r#", "deny_reason": "{r}""#)
    } else {
        String::new()
    };
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "{policy_id}",
  "enabled": true,
  "max_portfolio_notional_usd": 25000,
  "per_strategy_budgets": [
    {{
      "strategy_id": "{strategy_id}",
      "budget_authorized": {budget_authorized},
      "risk_bucket": "equity_long_only"{deny_field}
    }}
  ]
}}"#
    )
}

/// Write a policy file to a temp dir.  Returns `(policy_path, dir)`.
fn write_policy_dir(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04_{tag}_{}_{}",
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
/// Gate ordering at start for this state:
///   1. deployment_readiness → passes
///   2. integrity → passes (armed)
///   3. [paper+alpaca WS gates] → not applicable for LiveShadow
///   4. [live-capital WS gate] → not applicable for LiveShadow
///   5. TV-02C artifact deployability gate → not triggered (no MQK_ARTIFACT_PATH)
///   6. TV-04A capital policy gate ← what A01..A05 exercise
///   7. db_pool() → ServiceUnavailable (no DB in test)
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

/// Build a Paper+Alpaca state for signal-boundary tests.
///
/// Paper+Alpaca wires ExternalSignalIngestion (Gate 1 passes).
/// WS continuity starts ColdStartUnproven, which blocks Gate 1b — but
/// Gate 1e (budget) fires before Gate 1b, so budget denial returns 403
/// before the WS continuity 503.
fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

async fn post_signal(
    st: Arc<state::AppState>,
    strategy_id: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "signal_id": format!("sig-test-{}", next_id()),
        "strategy_id": strategy_id,
        "symbol": "AAPL",
        "side": "buy",
        "qty": 1,
        "order_type": "market",
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let (status, bytes) = call(routes::build_router(st), req).await;
    let json = parse_json(bytes);
    (status, json)
}

// ===========================================================================
// TV-04A — Start boundary tests
// ===========================================================================

/// A01: No policy configured → gate not applicable → proceeds to DB gate (503).
#[tokio::test]
async fn a01_no_policy_configured_start_proceeds_to_db_gate() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    // DB gate fires (no DB in test) → 503.  Capital policy gate did not block.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_ne!(
        gate, "capital_allocation_policy",
        "policy gate must not fire when policy is not configured; got: {json}"
    );
}

/// A02: Policy present and enabled=true → start proceeds to DB gate (503).
#[tokio::test]
async fn a02_policy_authorized_start_proceeds_to_db_gate() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("a02", &valid_policy("test-policy-a02", true));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    // Policy authorized → gate passes → DB gate fires.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_ne!(
        gate, "capital_allocation_policy",
        "authorized policy must not block start; got: {json}"
    );
}

/// A03: Policy present but enabled=false → 403 blocked at capital_allocation_policy gate.
#[tokio::test]
async fn a03_policy_disabled_blocks_start() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("a03", &valid_policy("test-policy-a03", false));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "capital_allocation_policy",
        "disabled policy must block at capital_allocation_policy gate; got: {json}"
    );
    let err = json.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        err.contains("enabled=false"),
        "error field must mention enabled=false; got: {err}"
    );
}

/// A04: Policy file has invalid JSON → 403 blocked at capital_allocation_policy gate.
#[tokio::test]
async fn a04_policy_invalid_json_blocks_start() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("a04", "{ NOT VALID JSON");
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "capital_allocation_policy",
        "invalid JSON policy must block at capital_allocation_policy gate; got: {json}"
    );
}

/// A05: Policy path configured but file is missing → 403 blocked at capital_allocation_policy gate.
#[tokio::test]
async fn a05_policy_file_missing_blocks_start() {
    let _lock = env_lock().lock().await;
    let missing =
        std::env::temp_dir().join(format!("mqk_tv04_a05_missing_{}.json", std::process::id()));
    // Deliberately do NOT write the file.
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &missing);

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let gate = json.get("gate").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        gate, "capital_allocation_policy",
        "missing policy file must block at capital_allocation_policy gate; got: {json}"
    );
}

// ===========================================================================
// TV-04A — Pure evaluator tests
// ===========================================================================

/// A06: evaluate_capital_policy(None) → NotConfigured.
#[test]
fn a06_pure_not_configured() {
    assert_eq!(
        evaluate_capital_policy(None),
        CapitalPolicyOutcome::NotConfigured
    );
}

/// A07: evaluate_capital_policy → enabled=false → Denied with policy_id in reason.
#[test]
fn a07_pure_disabled_yields_denied() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_a07_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(&path, valid_policy("my-policy-id", false)).unwrap();

    let outcome = evaluate_capital_policy(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        CapitalPolicyOutcome::Denied { reason } => {
            assert!(
                reason.contains("my-policy-id"),
                "Denied reason must include policy_id; got: {reason}"
            );
            assert!(
                reason.contains("enabled=false"),
                "Denied reason must mention enabled=false; got: {reason}"
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

/// A08: evaluate_capital_policy with wrong schema_version → PolicyInvalid.
#[test]
fn a08_pure_wrong_schema_version_yields_invalid() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_a08_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"wrong-v99","policy_id":"p","enabled":true}"#,
    )
    .unwrap();

    let outcome = evaluate_capital_policy(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, CapitalPolicyOutcome::PolicyInvalid { .. }),
        "wrong schema_version must yield PolicyInvalid; got: {outcome:?}"
    );
}

/// A09: evaluate_capital_policy with enabled=true → Authorized carrying policy_id.
#[test]
fn a09_pure_enabled_yields_authorized() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_a09_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(&path, valid_policy("paper-q1-2026", true)).unwrap();

    let outcome = evaluate_capital_policy(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        CapitalPolicyOutcome::Authorized { policy_id } => {
            assert_eq!(policy_id, "paper-q1-2026");
        }
        other => panic!("expected Authorized, got {other:?}"),
    }
}

/// A10: evaluate_capital_policy missing policy_id → PolicyInvalid.
#[test]
fn a10_pure_missing_policy_id_yields_invalid() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_a10_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(&path, r#"{"schema_version":"policy-v1","enabled":true}"#).unwrap();

    let outcome = evaluate_capital_policy(Some(&path));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, CapitalPolicyOutcome::PolicyInvalid { .. }),
        "missing policy_id must yield PolicyInvalid; got: {outcome:?}"
    );
}

// ===========================================================================
// TV-04B — Signal boundary tests
// ===========================================================================

/// B01: No policy configured → budget not enforced → proceeds to Gate 1b
///      (WS continuity unproven → 503 with continuity blocker).
///
/// This proves Gate 1e did NOT block (no budget_denied disposition) and
/// Gate 1 DID pass (no "not configured" blocker), so we reached Gate 1b.
#[tokio::test]
async fn b01_no_policy_signal_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = paper_alpaca_state();
    let (status, json) = post_signal(st, "strat-any").await;

    // Gate 1b fires: WS continuity unproven → 503.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Must not be budget_denied (Gate 1e did not fire).
    assert_ne!(
        disposition, "budget_denied",
        "budget gate must not fire when policy is not configured; got: {json}"
    );
    // Blocker must mention WS continuity (Gate 1b fired, not Gate 1).
    let blockers = json
        .get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must mention WS continuity (Gate 1b); got: {blockers}"
    );
    assert!(
        !blockers.contains("not configured for this deployment"),
        "Gate 1 must not have fired; signal ingestion should be configured; got: {blockers}"
    );
}

/// B02: Policy present, strategy budget_authorized=true → proceeds to Gate 1b
///      (WS continuity unproven → 503 with continuity blocker).
///
/// Proves budget-authorized strategies pass Gate 1e and reach Gate 1b.
#[tokio::test]
async fn b02_authorized_strategy_signal_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().await;
    let strat = "strat-authorized";
    let (path, dir) = write_policy_dir(
        "b02",
        &policy_with_strategy("policy-b02", strat, true, None),
    );
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    let (status, json) = post_signal(st, strat).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    // Budget authorized → Gate 1b fires (WS continuity unproven → 503).
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_ne!(
        disposition, "budget_denied",
        "budget_authorized strategy must not be budget_denied; got: {json}"
    );
    let blockers = json
        .get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must mention WS continuity (Gate 1b fired, not Gate 1e); got: {blockers}"
    );
}

/// B03: Policy present, strategy has NO entry → 403 budget_denied before WS gate.
///
/// This is the key TV-04B proof: deployable + enabled ≠ budget-authorized.
/// An absent budget entry is refused fail-closed.
#[tokio::test]
async fn b03_absent_strategy_entry_is_budget_denied() {
    let _lock = env_lock().lock().await;
    // Policy exists but has no entry for this strategy.
    let (path, dir) = write_policy_dir("b03", &valid_policy("policy-b03", true));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    let (status, json) = post_signal(st, "strat-not-in-policy").await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "budget_denied",
        "absent strategy entry must yield budget_denied disposition; got: {json}"
    );
    let blockers = json
        .get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    assert!(
        blockers.contains("absent entry is not authorized"),
        "blocker must explain absent entry ≠ authorized; got: {blockers}"
    );
}

/// B04: Policy present, strategy budget_authorized=false → 403 budget_denied.
#[tokio::test]
async fn b04_budget_authorized_false_is_budget_denied() {
    let _lock = env_lock().lock().await;
    let strat = "strat-denied";
    let (path, dir) = write_policy_dir(
        "b04",
        &policy_with_strategy(
            "policy-b04",
            strat,
            false,
            Some("under review; budget not released for this run"),
        ),
    );
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    let (status, json) = post_signal(st, strat).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "budget_denied",
        "budget_authorized=false must yield budget_denied; got: {json}"
    );
    let blockers = json
        .get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    assert!(
        blockers.contains("under review"),
        "deny_reason must appear in blocker; got: {blockers}"
    );
}

/// B05: Policy file configured but invalid → 503 unavailable (fail-closed).
#[tokio::test]
async fn b05_invalid_policy_file_is_unavailable() {
    let _lock = env_lock().lock().await;
    let (path, dir) = write_policy_dir("b05", "NOT JSON AT ALL");
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    let (status, json) = post_signal(st, "strat-any").await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "unavailable",
        "invalid policy file must yield unavailable disposition; got: {json}"
    );
}

// ===========================================================================
// TV-04B — Pure evaluator tests
// ===========================================================================

/// B06: evaluate_strategy_budget — strategy not in policy → BudgetDenied.
///      Absent entry is not authorization.
#[test]
fn b06_pure_absent_strategy_entry_is_denied() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_b06_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    // Policy with no per_strategy_budgets for our strategy.
    std::fs::write(&path, valid_policy("p-b06", true)).unwrap();

    let outcome = evaluate_strategy_budget(Some(&path), "strat-missing");
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        StrategyBudgetOutcome::BudgetDenied { reason } => {
            assert!(
                reason.contains("absent entry is not authorized"),
                "BudgetDenied reason must explain absent entry; got: {reason}"
            );
        }
        other => panic!("expected BudgetDenied, got {other:?}"),
    }
}

/// B07: evaluate_strategy_budget — budget_authorized=true → BudgetAuthorized with risk_bucket.
#[test]
fn b07_pure_authorized_yields_budget_authorized() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_b07_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(
        &path,
        policy_with_strategy("p-b07", "strat-b07", true, None),
    )
    .unwrap();

    let outcome = evaluate_strategy_budget(Some(&path), "strat-b07");
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        StrategyBudgetOutcome::BudgetAuthorized {
            strategy_id,
            risk_bucket,
        } => {
            assert_eq!(strategy_id, "strat-b07");
            assert_eq!(
                risk_bucket,
                Some("equity_long_only".to_string()),
                "risk_bucket must be populated from policy entry"
            );
        }
        other => panic!("expected BudgetAuthorized, got {other:?}"),
    }
}

/// B08: evaluate_strategy_budget — budget_authorized=false with deny_reason →
///      BudgetDenied with reason included.
#[test]
fn b08_pure_denied_with_reason() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_b08_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(
        &path,
        policy_with_strategy(
            "p-b08",
            "strat-b08",
            false,
            Some("budget frozen pending quarterly review"),
        ),
    )
    .unwrap();

    let outcome = evaluate_strategy_budget(Some(&path), "strat-b08");
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        StrategyBudgetOutcome::BudgetDenied { reason } => {
            assert!(
                reason.contains("budget frozen pending quarterly review"),
                "BudgetDenied reason must include deny_reason; got: {reason}"
            );
        }
        other => panic!("expected BudgetDenied, got {other:?}"),
    }
}

/// B09: evaluate_strategy_budget — policy enabled=false → BudgetDenied.
#[test]
fn b09_pure_disabled_policy_yields_budget_denied() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_b09_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    // Policy disabled; strategy has an entry but it doesn't matter.
    std::fs::write(
        &path,
        r#"{
  "schema_version": "policy-v1",
  "policy_id": "p-b09",
  "enabled": false,
  "per_strategy_budgets": [
    {"strategy_id": "strat-b09", "budget_authorized": true}
  ]
}"#,
    )
    .unwrap();

    let outcome = evaluate_strategy_budget(Some(&path), "strat-b09");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, StrategyBudgetOutcome::BudgetDenied { .. }),
        "disabled policy must yield BudgetDenied even if strategy entry is authorized; \
         got: {outcome:?}"
    );
}

/// B10: evaluate_strategy_budget — per_strategy_budgets absent → BudgetDenied.
///      An array-less policy cannot authorize any strategy.
#[test]
fn b10_pure_absent_budgets_array_yields_denied() {
    let dir =
        std::env::temp_dir().join(format!("mqk_tv04_b10_{}_{}", std::process::id(), next_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(
        &path,
        r#"{"schema_version":"policy-v1","policy_id":"p-b10","enabled":true}"#,
    )
    .unwrap();

    let outcome = evaluate_strategy_budget(Some(&path), "strat-any");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, StrategyBudgetOutcome::BudgetDenied { .. }),
        "policy without per_strategy_budgets array must yield BudgetDenied; \
         got: {outcome:?}"
    );
}
