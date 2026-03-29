//! TV-04C: Position sizing realism under broker/account limits.
//!
//! Proves the repo can truthfully distinguish between:
//!
//! - A strategy that is enabled, deployable, and budget-authorized.
//! - A strategy whose implied or requested size is not executable under the
//!   broker/account limits stated in the capital allocation policy.
//!
//! The key distinction proven here:
//!
//! > **budget-authorized ≠ size-executable**
//!
//! A strategy with `budget_authorized=true` can still be refused at the signal
//! boundary because its implied position notional exceeds
//! `max_position_notional_usd`.
//!
//! # Design
//!
//! The sizing gate (Gate 1f) is a pure filesystem check inserted after the
//! budget gate (Gate 1e) and before the WS continuity gate (Gate 1b).  For
//! limit orders the implied notional is `qty × (limit_price / 1_000_000)`.
//! For market orders (no price reference) the outcome is `SizingUnverifiable`
//! — passed through honestly.
//!
//! All tests require no database and no network.
//!
//! # Proof matrix
//!
//! ## TV-04C — signal boundary (HTTP)
//!
//! | Test | What it proves                                                                           |
//! |------|------------------------------------------------------------------------------------------|
//! | C01  | No policy → sizing not applicable → proceeds past Gate 1f to Gate 1b (503 WS)          |
//! | C02  | Budget-authorized + limit order within notional cap → proceeds to Gate 1b (503 WS)      |
//! | C03  | Budget-authorized + limit order OVER notional cap → 403 sizing_denied (key proof)       |
//! | C04  | Budget-authorized + no max_position_notional_usd entry → no constraint → Gate 1b (503) |
//!
//! ## TV-04C — pure evaluator
//!
//! | Test | What it proves                                                                           |
//! |------|------------------------------------------------------------------------------------------|
//! | C05  | Pure: NotConfigured path → NotConfigured                                                 |
//! | C06  | Pure: limit order within cap → SizingAuthorized with correct implied_notional            |
//! | C07  | Pure: limit order over cap → SizingDenied with strategy_id and quantities                |
//! | C08  | Pure: market order + notional cap present → SizingUnverifiable (honest, not denied)      |
//! | C09  | Pure: entry missing max_position_notional_usd → NoSizingConstraint                       |
//! | C10  | Pure: same strategy + policy → BudgetAuthorized AND SizingDenied are simultaneously      |
//!         true; explicit proof that budget authorization ≠ sizing authorization              |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    capital_policy::{
        evaluate_position_sizing, evaluate_strategy_budget, PositionSizingOutcome,
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

/// Policy with a strategy entry that has `budget_authorized=true` and an
/// explicit `max_position_notional_usd` cap.
fn policy_with_notional_cap(strategy_id: &str, max_notional_usd: f64) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "tv04c-test-policy",
  "enabled": true,
  "max_portfolio_notional_usd": 50000,
  "per_strategy_budgets": [
    {{
      "strategy_id": "{strategy_id}",
      "budget_authorized": true,
      "max_position_notional_usd": {max_notional_usd},
      "risk_bucket": "equity_long_only"
    }}
  ]
}}"#
    )
}

/// Policy with a strategy entry that has `budget_authorized=true` but NO
/// `max_position_notional_usd` field.
fn policy_with_no_notional_cap(strategy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "tv04c-no-cap-policy",
  "enabled": true,
  "max_portfolio_notional_usd": 50000,
  "per_strategy_budgets": [
    {{
      "strategy_id": "{strategy_id}",
      "budget_authorized": true,
      "risk_bucket": "equity_long_only"
    }}
  ]
}}"#
    )
}

/// Write a policy file to a temp dir.  Returns `(policy_path, dir)`.
fn write_policy_dir(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04c_{tag}_{}_{}",
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

/// Build a Paper+Alpaca AppState for signal-boundary tests.
///
/// Paper+Alpaca wires ExternalSignalIngestion (Gate 1 passes).
/// WS continuity starts ColdStartUnproven (Gate 1b blocks at 503).
/// Gate 1e (budget) and Gate 1f (sizing) both fire before Gate 1b.
fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

/// POST a limit signal with explicit qty and limit_price_micros.
///
/// `limit_price_micros`: price in 1/1_000_000 USD (e.g., $100.00 = 100_000_000).
async fn post_limit_signal(
    st: Arc<state::AppState>,
    strategy_id: &str,
    qty: i64,
    limit_price_micros: i64,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "signal_id": format!("sig-tv04c-{}", next_id()),
        "strategy_id": strategy_id,
        "symbol": "AAPL",
        "side": "buy",
        "qty": qty,
        "order_type": "limit",
        "limit_price": limit_price_micros,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let (status, bytes) = call(routes::build_router(st), req).await;
    (status, parse_json(bytes))
}

// ===========================================================================
// TV-04C — Signal boundary tests (HTTP)
// ===========================================================================

/// C01: No policy configured → sizing gate not applicable → proceeds past
///      Gate 1f to Gate 1b (WS continuity unproven → 503).
///
/// Proves Gate 1f does not fire when no policy is configured.
#[tokio::test]
async fn c01_no_policy_sizing_not_applicable_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = paper_alpaca_state();
    // Limit order: if Gate 1f were wrongly firing, it might block here.
    // With no policy, sizing is NotConfigured → pass → Gate 1b fires.
    let (status, json) = post_limit_signal(st, "strat-no-policy", 100, 100_000_000).await;

    // Gate 1b fires: WS continuity unproven → 503.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_ne!(
        disposition, "sizing_denied",
        "sizing gate must not fire when no policy is configured; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must reference WS continuity (Gate 1b); got: {blockers}"
    );
}

/// C02: Budget-authorized + limit order WITHIN notional cap → proceeds past
///      Gate 1f to Gate 1b (503 WS continuity).
///
/// Proves sizing-authorized strategies are not blocked by Gate 1f.
///
/// Policy: max_position_notional_usd = $1000.
/// Signal: qty=5 × limit_price=$100 → implied_notional=$500 ≤ $1000.
#[tokio::test]
async fn c02_limit_order_within_cap_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let strat = "strat-sizing-ok";
    // max = $1000; qty=5 × $100 = $500 → under cap
    let (path, dir) = write_policy_dir("c02", &policy_with_notional_cap(strat, 1000.0));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // limit_price = $100.00 = 100_000_000 micros
    let (status, json) = post_limit_signal(st, strat, 5, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    // Sizing authorized ($500 ≤ $1000) → Gate 1f passes → Gate 1b fires.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_ne!(
        disposition, "sizing_denied",
        "sizing gate must not fire when implied notional is within cap; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must reference WS continuity (Gate 1b); got: {blockers}"
    );
}

/// C03: Budget-authorized + limit order OVER notional cap → 403 sizing_denied.
///
/// KEY PROOF: a strategy with budget_authorized=true can still be refused
/// because its implied position size is not realistic under the policy cap.
///
/// Policy: max_position_notional_usd = $100.
/// Signal: qty=5 × limit_price=$100 → implied_notional=$500 > $100.
#[tokio::test]
async fn c03_limit_order_over_cap_returns_403_sizing_denied() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let strat = "strat-over-cap";
    // max = $100; qty=5 × $100 = $500 → over cap
    let (path, dir) = write_policy_dir("c03", &policy_with_notional_cap(strat, 100.0));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // limit_price = $100.00 = 100_000_000 micros
    let (status, json) = post_limit_signal(st, strat, 5, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    // Sizing denied ($500 > $100) → Gate 1f blocks → 403 sizing_denied.
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "sizing gate must block over-cap signal with 403; got: {json}"
    );
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "sizing_denied",
        "disposition must be sizing_denied; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.contains(strat),
        "blocker must name the strategy; got: {blockers}"
    );
    assert!(
        blockers.contains("500") || blockers.contains("notional"),
        "blocker must reference the notional or quantity; got: {blockers}"
    );
}

/// C04: Budget-authorized + entry has NO max_position_notional_usd → no
///      sizing constraint → proceeds past Gate 1f to Gate 1b (503 WS).
///
/// Proves that absent notional cap is handled as NoSizingConstraint, not as
/// a denial.
#[tokio::test]
async fn c04_no_notional_cap_in_entry_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let strat = "strat-no-cap";
    let (path, dir) = write_policy_dir("c04", &policy_with_no_notional_cap(strat));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // Large order that would exceed any reasonable cap — but no cap is set.
    let (status, json) = post_limit_signal(st, strat, 10_000, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    // No sizing constraint → Gate 1f passes → Gate 1b fires.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_ne!(
        disposition, "sizing_denied",
        "sizing gate must not fire when no notional cap exists in entry; got: {json}"
    );
}

// ===========================================================================
// TV-04C — Pure evaluator tests
// ===========================================================================

fn make_policy_file(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04c_pure_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(&path, contents).unwrap();
    (path, dir)
}

/// C05: Pure: None path → NotConfigured.
#[test]
fn c05_pure_none_path_yields_not_configured() {
    let outcome = evaluate_position_sizing(None, "strat-x", 10, Some(100_000_000));
    assert_eq!(
        outcome,
        PositionSizingOutcome::NotConfigured,
        "None path must yield NotConfigured"
    );
}

/// C06: Pure: limit order within cap → SizingAuthorized with correct implied_notional.
///
/// qty=5 × limit_price=$100 → implied_notional=$500; cap=$1000.
#[test]
fn c06_pure_limit_order_within_cap_yields_sizing_authorized() {
    let strat = "strat-c06";
    let (path, dir) = make_policy_file("c06", &policy_with_notional_cap(strat, 1000.0));

    // limit_price = $100.00 = 100_000_000 micros; qty=5 → notional=$500
    let outcome = evaluate_position_sizing(Some(&path), strat, 5, Some(100_000_000));
    let _ = cleanup(&dir);

    match outcome {
        PositionSizingOutcome::SizingAuthorized {
            strategy_id,
            implied_notional_usd,
            max_position_notional_usd,
        } => {
            assert_eq!(strategy_id, strat);
            assert!(
                (implied_notional_usd - 500.0).abs() < 0.01,
                "implied_notional_usd must be ~500; got {implied_notional_usd}"
            );
            assert!(
                (max_position_notional_usd - 1000.0).abs() < 0.01,
                "max_position_notional_usd must be ~1000; got {max_position_notional_usd}"
            );
        }
        other => panic!("expected SizingAuthorized, got {other:?}"),
    }
}

/// C07: Pure: limit order over cap → SizingDenied with reason containing
///      strategy_id and relevant quantities.
///
/// qty=5 × limit_price=$100 → implied_notional=$500; cap=$100.
#[test]
fn c07_pure_limit_order_over_cap_yields_sizing_denied() {
    let strat = "strat-c07";
    let (path, dir) = make_policy_file("c07", &policy_with_notional_cap(strat, 100.0));

    let outcome = evaluate_position_sizing(Some(&path), strat, 5, Some(100_000_000));
    let _ = cleanup(&dir);

    match outcome {
        PositionSizingOutcome::SizingDenied { reason } => {
            assert!(
                reason.contains(strat),
                "SizingDenied reason must name strategy; got: {reason}"
            );
            // Reason must reference the implied or cap value.
            assert!(
                reason.contains("500") || reason.contains("notional"),
                "SizingDenied reason must reference the notional or quantities; got: {reason}"
            );
        }
        other => panic!("expected SizingDenied, got {other:?}"),
    }
}

/// C08: Pure: market order (limit_price=None) with notional cap → SizingUnverifiable.
///
/// Proves market orders are not silently authorized when a cap exists, but are
/// also not denied — they are explicitly surfaced as unverifiable.
#[test]
fn c08_pure_market_order_with_cap_yields_sizing_unverifiable() {
    let strat = "strat-c08";
    let (path, dir) = make_policy_file("c08", &policy_with_notional_cap(strat, 500.0));

    // No limit_price → market order
    let outcome = evaluate_position_sizing(Some(&path), strat, 100, None);
    let _ = cleanup(&dir);

    match outcome {
        PositionSizingOutcome::SizingUnverifiable { reason } => {
            assert!(
                reason.contains(strat),
                "SizingUnverifiable reason must name strategy; got: {reason}"
            );
            assert!(
                reason.to_lowercase().contains("market") || reason.to_lowercase().contains("price"),
                "SizingUnverifiable reason must explain why (market order / no price); got: {reason}"
            );
        }
        other => panic!("expected SizingUnverifiable, got {other:?}"),
    }
}

/// C09: Pure: budget entry exists but has no `max_position_notional_usd` →
///      NoSizingConstraint.
#[test]
fn c09_pure_no_notional_cap_in_entry_yields_no_sizing_constraint() {
    let strat = "strat-c09";
    let (path, dir) = make_policy_file("c09", &policy_with_no_notional_cap(strat));

    let outcome = evaluate_position_sizing(Some(&path), strat, 1_000_000, Some(100_000_000));
    let _ = cleanup(&dir);

    assert_eq!(
        outcome,
        PositionSizingOutcome::NoSizingConstraint,
        "entry without max_position_notional_usd must yield NoSizingConstraint"
    );
}

/// C10: Explicit proof that BudgetAuthorized ≠ SizingAuthorized.
///
/// Same policy file, same strategy.  The budget evaluator returns
/// `BudgetAuthorized` (budget_authorized=true) while the sizing evaluator
/// returns `SizingDenied` (implied notional exceeds the cap).
///
/// This is the core truth statement of TV-04C.
#[test]
fn c10_pure_budget_authorized_yet_sizing_denied_are_simultaneously_true() {
    let strat = "strat-c10";
    // cap = $100; qty=10 × $50 = $500 → over cap
    let (path, dir) = make_policy_file("c10", &policy_with_notional_cap(strat, 100.0));

    let budget = evaluate_strategy_budget(Some(&path), strat);
    // limit_price = $50.00 = 50_000_000 micros; qty=10 → notional=$500 > $100
    let sizing = evaluate_position_sizing(Some(&path), strat, 10, Some(50_000_000));
    let _ = cleanup(&dir);

    // Budget gate passes.
    assert!(
        matches!(budget, StrategyBudgetOutcome::BudgetAuthorized { .. }),
        "budget gate must pass (budget_authorized=true); got: {budget:?}"
    );

    // Sizing gate denies — the proof.
    assert!(
        matches!(sizing, PositionSizingOutcome::SizingDenied { .. }),
        "sizing gate must deny (notional=$500 > cap=$100); got: {sizing:?}"
    );

    // Verify both truth-states are distinct and correct.
    assert_eq!(budget.truth_state(), "authorized");
    assert_eq!(sizing.truth_state(), "sizing_denied");
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn blockers_str(json: &serde_json::Value) -> String {
    json.get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default()
}
