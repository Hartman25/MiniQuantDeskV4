//! MULTI-SYMBOL-CAPITAL-CAPS-01: Proof tests for caps #2, #3, and #5
//! (design doc §6, "Patch 6").
//!
//! All three caps are optional (`Option`-typed, default `None`/disabled) and
//! follow the same lightweight env-var pattern as cap #4
//! (`MULTI-SYMBOL-DAY-ORDER-CAP-01` / `MQK_PER_SYMBOL_DAY_ORDER_LIMIT`) rather
//! than the `MultiSymbolRiskCaps` JSON schema sketched in design doc §4.7.
//!
//! - **Cap #2** (`per_symbol_max_position_qty`, `MQK_PER_SYMBOL_MAX_POSITION_QTY`):
//!   B1C target-qty clamp. `AppState::clamp_targets_to_per_symbol_position_cap`
//!   clamps `|qty| > cap` to `±cap`, sign preserved.
//! - **Cap #3** (`per_symbol_max_notional_usd`, `MQK_PER_SYMBOL_MAX_NOTIONAL_USD`):
//!   Gate 1g in `submit_internal_strategy_decision`, between Gate 1e (capital
//!   budget) and Gate 2 (DB presence). Limit orders only; market orders are
//!   `SizingUnverifiable` (honest pass-through).
//! - **Cap #5** (`aggregate_gross_exposure_cap_usd`, `MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD`):
//!   tightens `evaluate_portfolio_risk`'s effective portfolio notional cap to
//!   `min(max_portfolio_notional_usd, aggregate_gross_exposure_cap_usd)` when
//!   the aggregate cap is set, positive, and smaller. Does not introduce a
//!   constraint on its own — `max_portfolio_notional_usd` must already be
//!   configured (`NoRiskConstraints` early-exit at step 4 is unaffected).
//!
//! # Proof matrix
//!
//! ## Cap #2 — `clamp_targets_to_per_symbol_position_cap` (pure)
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C2-01 | Target within cap (`|qty| <= cap`) is left unchanged; no clamp recorded |
//! | C2-02 | Target exactly at the cap boundary is NOT clamped (only strict `>`)     |
//! | C2-03 | Positive target over cap clamped to `+cap`, sign preserved             |
//! | C2-04 | Negative (short) target over cap clamped to `-cap`, sign preserved     |
//! | C2-05 | Multi-target: only over-cap targets clamped/reported, in order         |
//!
//! ## Cap #2 — `AppState` config + Discord alert dedup
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C2-06 | Default `per_symbol_max_position_qty` is `None` (clamp disabled)        |
//! | C2-07 | `set_per_symbol_max_position_qty_for_test` overrides the configured cap |
//! | C2-08 | Alert dedup: first claim per symbol succeeds, repeat claim fails        |
//!
//! ## Cap #3 — `evaluate_per_symbol_notional_cap` (pure)
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C3-01 | `cap_usd = None` → `NoSizingConstraint` (disabled, the default)        |
//! | C3-02 | Non-positive `cap_usd` (0.0, -100.0) treated as disabled                |
//! | C3-03 | Market order with active cap → `SizingUnverifiable` (honest pass)      |
//! | C3-04 | Limit order under cap → `NoSizingConstraint` (checked, not denied)     |
//! | C3-05 | Limit order over cap → `SizingDeniedPerSymbolCap` with exact values    |
//!
//! ## Cap #3 — Gate 1g integration
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C3-06 | Limit-order `InternalStrategyDecision` over `MQK_PER_SYMBOL_MAX_NOTIONAL_USD` is `disposition="rejected"`, blocker names symbol + cap, fires before Gate 2 (DB) |
//!
//! ## Cap #5 — `evaluate_portfolio_risk` aggregate gross exposure cap (pure)
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C5-01 | Aggregate cap < `max_portfolio_notional_usd` and binding: an order that is `Authorized` under `None` becomes `ExposureDenied` (reason names `aggregate_gross_exposure_cap_usd`) |
//! | C5-02 | Aggregate cap > `max_portfolio_notional_usd`: `max_portfolio_notional_usd` remains binding; outcome unchanged from `None` |
//! | C5-03 | Non-positive aggregate cap (0.0, -100.0) treated as disabled, same as `None` |
//! | C5-04 | Aggregate cap binding for the exhaustion check: `Authorized` under `None` becomes `ExhaustionDenied` |
//! | C5-05 | `max_portfolio_notional_usd` absent → `NoRiskConstraints` regardless of aggregate cap (cap #5 introduces no constraint on its own) |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mqk_daemon::{
    capital_policy::{
        evaluate_per_symbol_notional_cap, evaluate_portfolio_risk, PortfolioRiskOutcome,
        PositionSizingOutcome,
    },
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state,
};
use mqk_strategy::TargetPosition;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// RAII guard: sets an env var on construction, restores the previous value
/// (or removes it) on drop.
///
/// Used only by C3-06, the single test in this file that reads
/// `MQK_PER_SYMBOL_MAX_NOTIONAL_USD` (via
/// `evaluate_per_symbol_notional_cap_from_env` inside Gate 1g). No other test
/// in this file reads that env var, so no cross-test `ENV_LOCK` is needed.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

// ---------------------------------------------------------------------------
// Cap #5 fixture helpers
// ---------------------------------------------------------------------------

/// Policy with `budget_authorized=true` and portfolio risk fields (exposure
/// pct + exhaustion reserve) for `strategy_id`.
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
  "policy_id": "caps01-risk-policy",
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

/// Policy with NO `max_portfolio_notional_usd` field at all — proves the
/// step-4 `NoRiskConstraints` early-exit (cap #5 introduces no constraint on
/// its own).
fn policy_without_portfolio_cap(strategy_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "policy-v1",
  "policy_id": "caps01-no-portfolio-cap-policy",
  "enabled": true,
  "per_strategy_budgets": [
    {{
      "strategy_id": "{strategy_id}",
      "budget_authorized": true,
      "max_order_exposure_pct_of_portfolio": 0.50,
      "capital_exhaustion_reserve_usd": 1000,
      "risk_bucket": "equity_long_only"
    }}
  ]
}}"#
    )
}

fn write_tmp_policy(tag: &str, contents: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_caps01_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create policy dir");
    let path = dir.join("capital_allocation_policy.json");
    std::fs::write(&path, contents).expect("write policy file");
    (path, dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ===========================================================================
// Cap #2 — clamp_targets_to_per_symbol_position_cap (pure)
// ===========================================================================

/// C2-01: target qty within the cap is left unchanged; no clamp recorded.
#[test]
fn c2_01_target_within_cap_unchanged() {
    let mut targets = vec![TargetPosition {
        symbol: "AAPL".to_string(),
        qty: 300,
    }];
    let clamped = state::AppState::clamp_targets_to_per_symbol_position_cap(&mut targets, 500);
    assert!(clamped.is_empty(), "no clamp expected; got: {clamped:?}");
    assert_eq!(targets[0].qty, 300);
}

/// C2-02: target qty exactly at the cap boundary is NOT clamped — only
/// strict `> cap` clamps.
#[test]
fn c2_02_target_at_cap_boundary_unchanged() {
    let mut targets = vec![TargetPosition {
        symbol: "AAPL".to_string(),
        qty: 500,
    }];
    let clamped = state::AppState::clamp_targets_to_per_symbol_position_cap(&mut targets, 500);
    assert!(
        clamped.is_empty(),
        "exact-boundary qty must not be clamped; got: {clamped:?}"
    );
    assert_eq!(targets[0].qty, 500);
}

/// C2-03: positive target over the cap is clamped to `+cap`, sign preserved.
#[test]
fn c2_03_positive_target_over_cap_clamped_preserving_sign() {
    let mut targets = vec![TargetPosition {
        symbol: "AAPL".to_string(),
        qty: 1000,
    }];
    let clamped = state::AppState::clamp_targets_to_per_symbol_position_cap(&mut targets, 500);
    assert_eq!(clamped, vec![("AAPL".to_string(), 1000, 500)]);
    assert_eq!(targets[0].qty, 500);
}

/// C2-04: negative (short) target over the cap in magnitude is clamped to
/// `-cap`, sign preserved.
#[test]
fn c2_04_negative_target_over_cap_clamped_preserving_sign() {
    let mut targets = vec![TargetPosition {
        symbol: "TSLA".to_string(),
        qty: -1000,
    }];
    let clamped = state::AppState::clamp_targets_to_per_symbol_position_cap(&mut targets, 500);
    assert_eq!(clamped, vec![("TSLA".to_string(), -1000, -500)]);
    assert_eq!(targets[0].qty, -500);
}

/// C2-05: multi-target — only over-cap targets are clamped and reported, in
/// `targets` order; unaffected targets are untouched.
#[test]
fn c2_05_multi_target_only_over_cap_clamped_in_order() {
    let mut targets = vec![
        TargetPosition {
            symbol: "AAPL".to_string(),
            qty: 1000,
        },
        TargetPosition {
            symbol: "MSFT".to_string(),
            qty: 200,
        },
        TargetPosition {
            symbol: "TSLA".to_string(),
            qty: -2000,
        },
    ];
    let clamped = state::AppState::clamp_targets_to_per_symbol_position_cap(&mut targets, 500);
    assert_eq!(
        clamped,
        vec![
            ("AAPL".to_string(), 1000, 500),
            ("TSLA".to_string(), -2000, -500),
        ]
    );
    assert_eq!(targets[0].qty, 500);
    assert_eq!(targets[1].qty, 200, "MSFT within cap must be untouched");
    assert_eq!(targets[2].qty, -500);
}

// ===========================================================================
// Cap #2 — AppState config + Discord alert dedup
// ===========================================================================

/// C2-06: `per_symbol_max_position_qty` defaults to `None` (B1C clamp
/// disabled) when `MQK_PER_SYMBOL_MAX_POSITION_QTY` is unset — the default
/// per design doc §6.
#[tokio::test]
async fn c2_06_default_per_symbol_max_position_qty_is_none() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert_eq!(
        st.per_symbol_max_position_qty().await,
        None,
        "default per-symbol position qty cap must be None (clamp disabled)"
    );
}

/// C2-07: `set_per_symbol_max_position_qty_for_test` overrides the
/// configured cap, independent of env state.
#[tokio::test]
async fn c2_07_test_seam_overrides_cap() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.set_per_symbol_max_position_qty_for_test(Some(500));
    assert_eq!(st.per_symbol_max_position_qty().await, Some(500));
}

/// C2-08: cap #2 Discord alert dedup —
/// `try_claim_per_symbol_position_cap_alert_for_test` returns `true` on the
/// first claim for a symbol this run, `false` on a repeat claim for the same
/// symbol, and `true` again (independently) for a different symbol's first
/// claim — mirroring the B5 alert-dedup shape.
#[tokio::test]
async fn c2_08_alert_dedup_per_symbol_first_claim_only() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    assert!(
        st.try_claim_per_symbol_position_cap_alert_for_test("AAPL")
            .await,
        "first claim for AAPL must succeed"
    );
    assert!(
        !st.try_claim_per_symbol_position_cap_alert_for_test("AAPL")
            .await,
        "second claim for AAPL must fail (already alerted this run)"
    );
    assert!(
        st.try_claim_per_symbol_position_cap_alert_for_test("MSFT")
            .await,
        "first claim for a different symbol (MSFT) must succeed independently"
    );
}

// ===========================================================================
// Cap #3 — evaluate_per_symbol_notional_cap (pure)
// ===========================================================================

/// C3-01: `cap_usd = None` → `NoSizingConstraint` regardless of order shape —
/// the default per design doc §6.
#[test]
fn c3_01_cap_disabled_returns_no_sizing_constraint() {
    let outcome = evaluate_per_symbol_notional_cap("AAPL", 999_999, Some(100_000_000), None);
    assert_eq!(outcome, PositionSizingOutcome::NoSizingConstraint);
    assert!(outcome.is_signal_safe());
}

/// C3-02: non-positive `cap_usd` (0.0, -100.0) is treated as disabled, same
/// as `None`.
#[test]
fn c3_02_non_positive_cap_treated_as_disabled() {
    for bad_cap in [0.0, -100.0] {
        let outcome =
            evaluate_per_symbol_notional_cap("AAPL", 999_999, Some(100_000_000), Some(bad_cap));
        assert_eq!(
            outcome,
            PositionSizingOutcome::NoSizingConstraint,
            "cap_usd={bad_cap} must disable the check"
        );
    }
}

/// C3-03: market order (`limit_price_micros = None`) with an active cap →
/// `SizingUnverifiable` — honest pass-through (same shape as
/// `evaluate_position_sizing`'s market-order gap).
#[test]
fn c3_03_market_order_with_active_cap_is_unverifiable() {
    let outcome = evaluate_per_symbol_notional_cap("AAPL", 100, None, Some(1000.0));
    match &outcome {
        PositionSizingOutcome::SizingUnverifiable { reason } => {
            assert!(
                reason.contains("AAPL"),
                "reason must name the symbol; got: {reason}"
            );
            assert!(
                reason.to_lowercase().contains("market order"),
                "reason must explain the market-order gap; got: {reason}"
            );
        }
        other => panic!("expected SizingUnverifiable; got: {other:?}"),
    }
    assert!(
        outcome.is_signal_safe(),
        "SizingUnverifiable is an honest pass-through"
    );
    assert_eq!(outcome.truth_state(), "unverifiable");
}

/// C3-04: limit order with implied notional under the cap →
/// `NoSizingConstraint` (checked, not denied).
#[test]
fn c3_04_limit_order_under_cap_returns_no_sizing_constraint() {
    // qty=5 x $100 = $500 < $1000 cap.
    let outcome = evaluate_per_symbol_notional_cap("AAPL", 5, Some(100_000_000), Some(1000.0));
    assert_eq!(outcome, PositionSizingOutcome::NoSizingConstraint);
    assert!(outcome.is_signal_safe());
}

/// C3-05: limit order with implied notional over the cap →
/// `SizingDeniedPerSymbolCap`, fail-closed.
#[test]
fn c3_05_limit_order_over_cap_returns_denied_per_symbol_cap() {
    // qty=15 x $100 = $1500 > $1000 cap.
    let outcome = evaluate_per_symbol_notional_cap("AAPL", 15, Some(100_000_000), Some(1000.0));
    assert_eq!(
        outcome,
        PositionSizingOutcome::SizingDeniedPerSymbolCap {
            symbol: "AAPL".to_string(),
            implied_notional_usd: 1500.0,
            cap_usd: 1000.0,
        }
    );
    assert!(!outcome.is_signal_safe());
    assert_eq!(outcome.truth_state(), "sizing_denied_per_symbol_cap");
}

// ===========================================================================
// Cap #3 — Gate 1g integration in submit_internal_strategy_decision
// ===========================================================================

/// C3-06: a limit-order `InternalStrategyDecision` whose implied notional
/// exceeds `MQK_PER_SYMBOL_MAX_NOTIONAL_USD` is rejected by Gate 1g before
/// Gate 2 (DB) — `disposition = "rejected"` with a blocker naming the symbol
/// and the cap (design doc §6 cap #3 test).
#[tokio::test]
async fn c3_06_gate_1g_rejects_limit_order_over_per_symbol_notional_cap() {
    let _env = EnvVarGuard::set("MQK_PER_SYMBOL_MAX_NOTIONAL_USD", "1000");

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let d = InternalStrategyDecision {
        decision_id: unique_id("dec"),
        strategy_id: "strat-a".to_string(),
        symbol: "AAPL".to_string(),
        side: "buy".to_string(),
        qty: 15, // x $100 = $1500 > $1000 cap
        order_type: "limit".to_string(),
        time_in_force: "day".to_string(),
        limit_price: Some(100_000_000), // $100.00
    };

    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "Gate 1g denial must produce disposition=rejected (design doc §6 cap #3 test); \
         got: {:?} / blockers={:?}",
        out.disposition, out.blockers
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("AAPL") && b.contains("per_symbol_max_notional_usd")),
        "blocker must name the symbol and the cap; got: {:?}",
        out.blockers
    );
}

// ===========================================================================
// Cap #5 — evaluate_portfolio_risk aggregate gross exposure cap (pure)
// ===========================================================================

/// C5-01: aggregate cap `< max_portfolio_notional_usd` and binding — an order
/// that is `Authorized` under `None` becomes `ExposureDenied`, with the
/// reason naming `aggregate_gross_exposure_cap_usd` as the binding cap
/// source.
///
/// Portfolio cap: $10,000. Exposure cap: 50%. Aggregate cap: $5,000.
/// Order: qty=30 x $100 = $3,000.
/// - Under `None`: 3000/10000 = 30% <= 50% -> Authorized.
/// - Under `Some(5000.0)`: effective cap = 5000 (aggregate binds);
///   3000/5000 = 60% > 50% -> ExposureDenied.
#[test]
fn c5_01_aggregate_cap_binding_turns_authorized_into_exposure_denied() {
    let strat = "strat-c5-01";
    let (path, dir) = write_tmp_policy(
        "c5_01",
        &policy_with_risk_fields(strat, 10_000.0, 50_000.0, 0.50, 0.0),
    );

    let without_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), None);
    let with_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), Some(5000.0));
    cleanup(&dir);

    assert!(
        matches!(without_aggregate, PortfolioRiskOutcome::Authorized { .. }),
        "without an aggregate cap, 30% exposure must be Authorized (<=50% cap); got: {without_aggregate:?}"
    );

    assert!(
        matches!(with_aggregate, PortfolioRiskOutcome::ExposureDenied { .. }),
        "aggregate cap of $5000 must become the binding cap (60% > 50%); got: {with_aggregate:?}"
    );
    if let PortfolioRiskOutcome::ExposureDenied { reason } = &with_aggregate {
        assert!(
            reason.contains(strat),
            "reason must include strategy_id; got: {reason}"
        );
        assert!(
            reason.contains("aggregate_gross_exposure_cap_usd"),
            "reason must name the binding cap source; got: {reason}"
        );
    }
    assert!(!with_aggregate.is_signal_safe());
}

/// C5-02: aggregate cap `> max_portfolio_notional_usd` — `max_portfolio_notional_usd`
/// remains binding; outcome is unchanged from `None`.
///
/// Portfolio cap: $10,000. Exposure cap: 50%. Aggregate cap: $50,000 (looser).
/// Order: qty=30 x $100 = $3,000 -> 30% <= 50% -> Authorized either way.
#[test]
fn c5_02_aggregate_cap_looser_than_portfolio_cap_unchanged() {
    let strat = "strat-c5-02";
    let (path, dir) = write_tmp_policy(
        "c5_02",
        &policy_with_risk_fields(strat, 10_000.0, 50_000.0, 0.50, 0.0),
    );

    let without_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), None);
    let with_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), Some(50_000.0));
    cleanup(&dir);

    assert!(
        matches!(without_aggregate, PortfolioRiskOutcome::Authorized { .. }),
        "got: {without_aggregate:?}"
    );
    assert!(
        matches!(with_aggregate, PortfolioRiskOutcome::Authorized { .. }),
        "a looser aggregate cap must not change an Authorized outcome; got: {with_aggregate:?}"
    );
    assert_eq!(
        without_aggregate, with_aggregate,
        "outcome must be identical when the aggregate cap is not binding"
    );
}

/// C5-03: non-positive aggregate cap (0.0, -100.0) is treated as disabled,
/// same as `None`.
#[test]
fn c5_03_non_positive_aggregate_cap_treated_as_disabled() {
    let strat = "strat-c5-03";
    let (path, dir) = write_tmp_policy(
        "c5_03",
        &policy_with_risk_fields(strat, 10_000.0, 50_000.0, 0.50, 0.0),
    );

    let baseline = evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), None);
    for bad_cap in [0.0, -100.0] {
        let outcome =
            evaluate_portfolio_risk(Some(&path), strat, 30, Some(100_000_000), Some(bad_cap));
        assert_eq!(
            outcome, baseline,
            "aggregate_gross_exposure_cap_usd={bad_cap} must behave identically to None"
        );
    }
    cleanup(&dir);
}

/// C5-04: aggregate cap binding for the exhaustion check — an order that is
/// `Authorized` under `None` becomes `ExhaustionDenied`.
///
/// Portfolio cap: $10,000. Exposure cap: 100% (not the binding constraint).
/// Exhaustion reserve: $1,000. Aggregate cap: $4,000.
/// Order: qty=40 x $100 = $4,000.
/// - Under `None`: available = 10000-1000=9000; 4000 <= 9000 -> Authorized.
/// - Under `Some(4000.0)`: effective cap = 4000 (aggregate binds);
///   exposure 4000/4000=100% <= 100% ok; available = 4000-1000=3000;
///   4000 > 3000 -> ExhaustionDenied.
#[test]
fn c5_04_aggregate_cap_binding_turns_authorized_into_exhaustion_denied() {
    let strat = "strat-c5-04";
    let (path, dir) = write_tmp_policy(
        "c5_04",
        &policy_with_risk_fields(strat, 10_000.0, 50_000.0, 1.0, 1000.0),
    );

    let without_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 40, Some(100_000_000), None);
    let with_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 40, Some(100_000_000), Some(4000.0));
    cleanup(&dir);

    assert!(
        matches!(without_aggregate, PortfolioRiskOutcome::Authorized { .. }),
        "without an aggregate cap, $4000 implied notional must be Authorized \
         (available = 10000-1000=9000); got: {without_aggregate:?}"
    );

    assert!(
        matches!(
            with_aggregate,
            PortfolioRiskOutcome::ExhaustionDenied { .. }
        ),
        "aggregate cap of $4000 must become the binding cap, leaving only \
         $3000 available against a $4000 order; got: {with_aggregate:?}"
    );
    if let PortfolioRiskOutcome::ExhaustionDenied { reason } = &with_aggregate {
        assert!(
            reason.contains(strat),
            "reason must include strategy_id; got: {reason}"
        );
        assert!(
            reason.contains("aggregate_gross_exposure_cap_usd"),
            "reason must name the binding cap source; got: {reason}"
        );
        assert!(
            reason.to_lowercase().contains("exhaustion")
                || reason.to_lowercase().contains("reserve"),
            "reason must mention exhaustion or reserve; got: {reason}"
        );
    }
    assert!(!with_aggregate.is_signal_safe());
}

/// C5-05: `max_portfolio_notional_usd` absent -> `NoRiskConstraints`
/// regardless of the aggregate cap value. Proves cap #5 does not introduce a
/// constraint on its own — the step-4 early-exit in `evaluate_portfolio_risk`
/// happens before `aggregate_gross_exposure_cap_usd` is even considered.
#[test]
fn c5_05_no_portfolio_cap_means_no_risk_constraints_regardless_of_aggregate() {
    let strat = "strat-c5-05";
    let (path, dir) = write_tmp_policy("c5_05", &policy_without_portfolio_cap(strat));

    let without_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 40, Some(100_000_000), None);
    let with_aggregate =
        evaluate_portfolio_risk(Some(&path), strat, 40, Some(100_000_000), Some(1000.0));
    cleanup(&dir);

    assert_eq!(without_aggregate, PortfolioRiskOutcome::NoRiskConstraints);
    assert_eq!(
        with_aggregate,
        PortfolioRiskOutcome::NoRiskConstraints,
        "aggregate cap must not introduce a constraint when max_portfolio_notional_usd is absent"
    );
    assert!(without_aggregate.is_signal_safe());
    assert!(with_aggregate.is_signal_safe());
}
