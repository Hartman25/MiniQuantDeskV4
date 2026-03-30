//! TV-04E: Portfolio drift / exposure / capital exhaustion controls at the
//! signal ingestion boundary.
//!
//! Proves the repo can truthfully distinguish between:
//!
//! - A strategy that is budget-authorized, size-executable (TV-04B/C), AND
//!   within portfolio-level risk bounds.
//! - A strategy whose single-order implied notional breaches the portfolio
//!   exposure fraction or the capital exhaustion reserve.
//!
//! The key distinctions proven here:
//!
//! > **sizing-authorized ≠ exposure-safe**
//! > **budget-authorized + sizing-authorized ≠ exhaustion-safe**
//! > **drift is not measurable at signal time without runtime portfolio state**
//!
//! # New policy fields (policy-v1 schema extension)
//!
//! ```json
//! {
//!   "per_strategy_budgets": [
//!     {
//!       "strategy_id": "strat-x",
//!       "budget_authorized": true,
//!       "max_order_exposure_pct_of_portfolio": 0.05,
//!       "capital_exhaustion_reserve_usd": 2000
//!     }
//!   ]
//! }
//! ```
//!
//! Both fields are optional.  Absent fields mean the control is not enforced
//! for that strategy entry.
//!
//! # Design
//!
//! Gate 1g (portfolio risk, TV-04E) is placed after Gate 1f (sizing, TV-04C)
//! and before Gate 1b (WS continuity).  It is a pure filesystem check.
//!
//! All tests require no database and no network.
//!
//! # Proof matrix
//!
//! ## TV-04E — signal boundary (HTTP)
//!
//! | Test | What it proves                                                                         |
//! |------|----------------------------------------------------------------------------------------|
//! | E01  | No policy → not configured → proceeds past Gate 1g to Gate 1b (503 WS)               |
//! | E02  | Policy + order within exposure cap → authorized → proceeds to Gate 1b (503 WS)        |
//! | E03  | Policy + limit order OVER exposure cap → 403 exposure_denied (key proof)              |
//! | E04  | Policy + limit order over exhaustion reserve → 403 exhaustion_denied (key proof)      |
//! | E05  | Policy entry with no risk fields → NoRiskConstraints → proceeds to Gate 1b (503)      |
//!
//! ## TV-04E — pure evaluator
//!
//! | Test | What it proves                                                                         |
//! |------|----------------------------------------------------------------------------------------|
//! | E06  | Pure: None path → NotConfigured                                                        |
//! | E07  | Pure: limit order within exposure cap → Authorized                                     |
//! | E08  | Pure: limit order over exposure cap → ExposureDenied with strategy/values              |
//! | E09  | Pure: market order + risk cap present → RiskUnverifiable (honest pass; not denied)     |
//! | E10  | Pure: within exhaustion reserve → Authorized                                           |
//! | E11  | Pure: order exceeds exhaustion reserve → ExhaustionDenied with strategy/values         |
//! | E12  | Pure cross-gate: BudgetAuthorized + SizingAuthorized + ExposureDenied simultaneously   |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    capital_policy::{
        evaluate_portfolio_risk, evaluate_position_sizing, evaluate_strategy_budget,
        PortfolioRiskOutcome, PositionSizingOutcome, StrategyBudgetOutcome,
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

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Policy with budget_authorized=true, max_position_notional_usd cap, and
/// portfolio risk fields (exposure_pct + exhaustion_reserve).
fn policy_with_risk_fields(
    strategy_id: &str,
    portfolio_cap_usd: f64,
    max_position_notional_usd: f64,
    exposure_pct: f64,
    exhaustion_reserve_usd: f64,
) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "tv04e-risk-policy",
  "enabled": true,
  "max_portfolio_notional_usd": {portfolio_cap_usd},
  "per_strategy_budgets": [
    {{
      "strategy_id": "{strategy_id}",
      "budget_authorized": true,
      "max_position_notional_usd": {max_position_notional_usd},
      "max_order_exposure_pct_of_portfolio": {exposure_pct},
      "capital_exhaustion_reserve_usd": {exhaustion_reserve_usd},
      "risk_bucket": "equity_long_only"
    }}
  ]
}}"#
    )
}

/// Policy with budget_authorized=true and NO risk fields for the strategy.
fn policy_without_risk_fields(strategy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "tv04e-no-risk-policy",
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
        "mqk_tv04e_{tag}_{}_{}",
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
/// WS continuity starts ColdStartUnproven, which blocks Gate 1b at 503.
/// Gate 1e (budget), Gate 1f (sizing), and Gate 1g (portfolio risk) all fire
/// before Gate 1b.
fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

/// POST a limit signal with explicit qty and limit_price_micros.
async fn post_limit_signal(
    st: Arc<state::AppState>,
    strategy_id: &str,
    qty: i64,
    limit_price_micros: i64,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "signal_id": format!("sig-tv04e-{}", next_id()),
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

/// POST a market signal (no limit_price).
#[allow(dead_code)]
async fn post_market_signal(
    st: Arc<state::AppState>,
    strategy_id: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "signal_id": format!("sig-tv04e-mkt-{}", next_id()),
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
    (status, parse_json(bytes))
}

// ===========================================================================
// TV-04E — Signal boundary tests (HTTP)
// ===========================================================================

/// E01: No policy → Gate 1g not applicable → proceeds to Gate 1b (503 WS).
///
/// Proves Gate 1g does not fire when no policy is configured.
#[tokio::test]
async fn e01_no_policy_risk_not_applicable_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = paper_alpaca_state();
    let (status, json) = post_limit_signal(st, "strat-no-policy", 10, 100_000_000).await;

    // Gate 1b fires: WS continuity unproven → 503.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        disposition != "exposure_denied" && disposition != "exhaustion_denied",
        "portfolio risk gate must not fire when no policy is configured; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must reference WS continuity (Gate 1b); got: {blockers}"
    );
}

/// E02: Policy + limit order within exposure cap → Gate 1g passes →
///      proceeds to Gate 1b (503 WS).
///
/// Portfolio cap: $50,000.  Exposure cap: 5% = $2,500.
/// Order: qty=10 × $100 = $1,000 implied notional → 2% < 5% cap → pass.
#[tokio::test]
async fn e02_limit_order_within_exposure_cap_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().await;
    let strat = "strat-exposure-ok";
    // portfolio_cap=50000, max_position=5000, exposure_pct=0.05, reserve=2000
    let (path, dir) = write_policy_dir(
        "e02",
        &policy_with_risk_fields(strat, 50000.0, 5000.0, 0.05, 2000.0),
    );
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // qty=10 × $100 = $1000 implied notional; 1000/50000 = 2% < 5% cap
    // also 1000 < 50000 - 2000 = 48000 (exhaustion check passes)
    let (status, json) = post_limit_signal(st, strat, 10, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        disposition != "exposure_denied" && disposition != "exhaustion_denied",
        "risk gate must not fire when order is within all caps; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must reference WS continuity (Gate 1b); got: {blockers}"
    );
}

/// E03: Policy + limit order OVER exposure cap → 403 exposure_denied.
///
/// Portfolio cap: $50,000.  Exposure cap: 5% = $2,500.
/// Order: qty=100 × $100 = $10,000 implied notional → 20% > 5% cap → denied.
///
/// Key proof: budget-authorized + sizing-authorized + EXPOSURE DENIED.
#[tokio::test]
async fn e03_limit_order_over_exposure_cap_returns_403_exposure_denied() {
    let _lock = env_lock().lock().await;
    let strat = "strat-exposure-breach";
    // portfolio_cap=50000, max_position=15000 (sizing allows), exposure_pct=0.05, reserve=2000
    let (path, dir) = write_policy_dir(
        "e03",
        &policy_with_risk_fields(strat, 50000.0, 15000.0, 0.05, 2000.0),
    );
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // qty=100 × $100 = $10,000 implied notional; 10000/50000 = 20% > 5% cap → denied
    let (status, json) = post_limit_signal(st, strat, 100, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "exposure_denied",
        "must be refused with exposure_denied; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("exposure"),
        "blocker must mention exposure; got: {blockers}"
    );
}

/// E04: Policy + limit order over capital exhaustion reserve → 403 exhaustion_denied.
///
/// Portfolio cap: $50,000.  Exhaustion reserve: $45,000 → available = $5,000.
/// Order: qty=100 × $100 = $10,000 implied notional > $5,000 available → denied.
///
/// Key proof: budget-authorized + sizing-authorized + EXHAUSTION DENIED.
#[tokio::test]
async fn e04_limit_order_over_exhaustion_reserve_returns_403_exhaustion_denied() {
    let _lock = env_lock().lock().await;
    let strat = "strat-exhaustion-breach";
    // portfolio_cap=50000, max_position=15000 (sizing allows), exposure_pct=0.25 (allows 20%),
    // reserve=45000 → available=5000
    let (path, dir) = write_policy_dir(
        "e04",
        &policy_with_risk_fields(strat, 50000.0, 15000.0, 0.25, 45000.0),
    );
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    // qty=100 × $100 = $10,000 > available $5,000 → exhaustion denied
    let (status, json) = post_limit_signal(st, strat, 100, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        disposition, "exhaustion_denied",
        "must be refused with exhaustion_denied; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("exhaustion")
            || blockers.to_lowercase().contains("reserve"),
        "blocker must mention exhaustion or reserve; got: {blockers}"
    );
}

/// E05: Policy entry with no risk fields → NoRiskConstraints → proceeds to
///      Gate 1b (503 WS).
///
/// Proves Gate 1g does not fire when no risk fields are present in the entry.
#[tokio::test]
async fn e05_entry_without_risk_fields_proceeds_to_ws_gate() {
    let _lock = env_lock().lock().await;
    let strat = "strat-no-risk";
    let (path, dir) = write_policy_dir("e05", &policy_without_risk_fields(strat));
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = paper_alpaca_state();
    let (status, json) = post_limit_signal(st, strat, 100, 100_000_000).await;

    cleanup(&dir);
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = json
        .get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        disposition != "exposure_denied" && disposition != "exhaustion_denied",
        "risk gate must not fire when entry has no risk fields; got: {json}"
    );
    let blockers = blockers_str(&json);
    assert!(
        blockers.to_lowercase().contains("continuity")
            || blockers.to_lowercase().contains("ws")
            || blockers.to_lowercase().contains("cold start"),
        "blocker must reference WS continuity (Gate 1b); got: {blockers}"
    );
}

// ===========================================================================
// TV-04E — Pure evaluator tests
// ===========================================================================

fn write_tmp_policy(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv04e_pure_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(&path, contents).unwrap();
    (path, dir)
}

/// E06: None path → NotConfigured.
#[test]
fn e06_pure_none_path_returns_not_configured() {
    let outcome = evaluate_portfolio_risk(None, "strat-x", 10, Some(100_000_000));
    assert_eq!(outcome, PortfolioRiskOutcome::NotConfigured);
    assert!(outcome.is_signal_safe());
}

/// E07: Limit order within exposure cap → Authorized.
///
/// Portfolio cap: $10,000.  Exposure cap: 10% = $1,000.
/// Order: qty=5 × $100 = $500 → 5% < 10% cap → Authorized.
#[test]
fn e07_pure_within_exposure_cap_returns_authorized() {
    let strat = "strat-within-cap";
    let (path, dir) = write_tmp_policy(
        "e07",
        &policy_with_risk_fields(strat, 10000.0, 2000.0, 0.10, 500.0),
    );

    // qty=5 × $100 = $500; 500/10000 = 5% < 10% cap; 500 < 10000-500=9500 → Authorized
    let outcome = evaluate_portfolio_risk(Some(&path), strat, 5, Some(100_000_000));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, PortfolioRiskOutcome::Authorized { .. }),
        "must return Authorized when within all caps; got: {outcome:?}"
    );
    assert!(outcome.is_signal_safe());
}

/// E08: Limit order over exposure cap → ExposureDenied with reason.
///
/// Portfolio cap: $10,000.  Exposure cap: 5% = $500.
/// Order: qty=100 × $100 = $10,000 → 100% > 5% cap → ExposureDenied.
#[test]
fn e08_pure_over_exposure_cap_returns_exposure_denied() {
    let strat = "strat-over-exposure";
    let (path, dir) = write_tmp_policy(
        "e08",
        &policy_with_risk_fields(strat, 10000.0, 15000.0, 0.05, 500.0),
    );

    // qty=100 × $100 = $10,000; 10000/10000 = 100% > 5% → ExposureDenied
    let outcome = evaluate_portfolio_risk(Some(&path), strat, 100, Some(100_000_000));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, PortfolioRiskOutcome::ExposureDenied { .. }),
        "must return ExposureDenied when exposure cap is exceeded; got: {outcome:?}"
    );
    if let PortfolioRiskOutcome::ExposureDenied { reason } = &outcome {
        assert!(
            reason.contains(strat),
            "reason must include strategy_id; got: {reason}"
        );
        assert!(
            reason.contains("exposure"),
            "reason must mention exposure; got: {reason}"
        );
    }
    assert!(!outcome.is_signal_safe());
}

/// E09: Market order + risk cap present → RiskUnverifiable (honest pass-through).
///
/// Proves market orders are not denied when risk caps are present; they are
/// surfaced as unverifiable (analogous to SizingUnverifiable in TV-04C).
/// Portfolio drift is also covered here: it is never measurable at signal time.
#[test]
fn e09_pure_market_order_with_risk_cap_returns_risk_unverifiable() {
    let strat = "strat-market-order";
    let (path, dir) = write_tmp_policy(
        "e09",
        &policy_with_risk_fields(strat, 50000.0, 5000.0, 0.05, 2000.0),
    );

    // limit_price_micros = None → market order → RiskUnverifiable
    let outcome = evaluate_portfolio_risk(Some(&path), strat, 10, None);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, PortfolioRiskOutcome::RiskUnverifiable { .. }),
        "market order with risk cap must return RiskUnverifiable; got: {outcome:?}"
    );
    if let PortfolioRiskOutcome::RiskUnverifiable { reason } = &outcome {
        assert!(
            reason.contains(strat),
            "reason must include strategy_id; got: {reason}"
        );
        assert!(
            reason.to_lowercase().contains("market order")
                || reason.to_lowercase().contains("price reference"),
            "reason must explain why risk is unverifiable; got: {reason}"
        );
    }
    // Key: RiskUnverifiable is signal-safe (honest pass-through, not a denial).
    assert!(
        outcome.is_signal_safe(),
        "RiskUnverifiable must be signal-safe (honest pass-through)"
    );
}

/// E10: Limit order within exhaustion reserve → Authorized.
///
/// Portfolio cap: $10,000.  Reserve: $2,000 → available = $8,000.
/// Order: qty=50 × $100 = $5,000 < $8,000 → Authorized.
#[test]
fn e10_pure_within_exhaustion_reserve_returns_authorized() {
    let strat = "strat-within-reserve";
    let (path, dir) = write_tmp_policy(
        "e10",
        &policy_with_risk_fields(strat, 10000.0, 8000.0, 0.60, 2000.0),
    );

    // qty=50 × $100 = $5000; 5000/10000 = 50% < 60% exposure cap; 5000 < 10000-2000=8000 → Auth
    let outcome = evaluate_portfolio_risk(Some(&path), strat, 50, Some(100_000_000));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, PortfolioRiskOutcome::Authorized { .. }),
        "must return Authorized when within exhaustion reserve; got: {outcome:?}"
    );
    assert!(outcome.is_signal_safe());
}

/// E11: Limit order exceeds exhaustion reserve → ExhaustionDenied with reason.
///
/// Portfolio cap: $10,000.  Reserve: $8,000 → available = $2,000.
/// Order: qty=50 × $100 = $5,000 > $2,000 → ExhaustionDenied.
#[test]
fn e11_pure_exceeds_exhaustion_reserve_returns_exhaustion_denied() {
    let strat = "strat-over-reserve";
    // exposure_pct=0.60 so exposure check passes; reserve=8000 so exhaustion check fails
    let (path, dir) = write_tmp_policy(
        "e11",
        &policy_with_risk_fields(strat, 10000.0, 8000.0, 0.60, 8000.0),
    );

    // qty=50 × $100 = $5000; 5000/10000 = 50% < 60% exposure ok; 5000 > 10000-8000=2000 → denied
    let outcome = evaluate_portfolio_risk(Some(&path), strat, 50, Some(100_000_000));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(outcome, PortfolioRiskOutcome::ExhaustionDenied { .. }),
        "must return ExhaustionDenied when reserve would be breached; got: {outcome:?}"
    );
    if let PortfolioRiskOutcome::ExhaustionDenied { reason } = &outcome {
        assert!(
            reason.contains(strat),
            "reason must include strategy_id; got: {reason}"
        );
        assert!(
            reason.to_lowercase().contains("exhaustion")
                || reason.to_lowercase().contains("reserve"),
            "reason must mention exhaustion or reserve; got: {reason}"
        );
    }
    assert!(!outcome.is_signal_safe());
}

/// E12: Cross-gate independence proof.
///
/// Same policy file → same strategy → same order:
///   TV-04B: BudgetAuthorized   (budget_authorized=true)
///   TV-04C: SizingAuthorized   (implied notional within max_position_notional_usd)
///   TV-04E: ExposureDenied     (implied notional exceeds portfolio exposure cap)
///
/// Proves the three gates are independent:
///   budget-authorized + sizing-authorized ≠ exposure-safe.
#[test]
fn e12_pure_cross_gate_budget_and_sizing_authorized_but_exposure_denied() {
    let strat = "strat-cross-gate";
    // Portfolio cap: $10,000.  Exposure cap: 5% = $500.
    // max_position_notional_usd: $5,000 (sizing gate allows up to $5,000)
    // Order: qty=100 × $100 = $10,000 → within... wait
    // Let me be precise:
    //   qty=20 × $100 = $2,000 implied notional
    //   max_position_notional_usd=5000 → $2,000 < $5,000 → SizingAuthorized
    //   exposure_pct=0.05 → 2000/10000=20% > 5% cap → ExposureDenied
    //   reserve=500 → available=9500 → $2,000 < $9,500 → exhaustion ok (but exposure fires first)
    let (path, dir) = write_tmp_policy(
        "e12",
        &policy_with_risk_fields(strat, 10000.0, 5000.0, 0.05, 500.0),
    );

    // qty=20, limit_price=$100 → implied=$2000
    let budget = evaluate_strategy_budget(Some(&path), strat);
    let sizing = evaluate_position_sizing(Some(&path), strat, 20, Some(100_000_000));
    let risk = evaluate_portfolio_risk(Some(&path), strat, 20, Some(100_000_000));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(budget, StrategyBudgetOutcome::BudgetAuthorized { .. }),
        "TV-04B must be BudgetAuthorized; got: {budget:?}"
    );
    assert!(
        matches!(sizing, PositionSizingOutcome::SizingAuthorized { .. }),
        "TV-04C must be SizingAuthorized ($2000 < $5000 cap); got: {sizing:?}"
    );
    assert!(
        matches!(risk, PortfolioRiskOutcome::ExposureDenied { .. }),
        "TV-04E must be ExposureDenied (20% > 5% exposure cap); got: {risk:?}"
    );

    // All three gates are independent: authorized at two levels ≠ safe at portfolio level.
    assert!(budget.is_signal_safe(), "budget must be signal-safe");
    assert!(sizing.is_signal_safe(), "sizing must be signal-safe");
    assert!(
        !risk.is_signal_safe(),
        "risk must NOT be signal-safe (exposure denied)"
    );
}
