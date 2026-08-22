//! PROMOTION-STRESS-SUITE-AUTHORITY-01 — real, deterministic Backtest
//! stress-scenario execution.
//!
//! # Investigation summary (why these three scenarios)
//!
//! [`mqk_promotion::StressSuiteResult`]'s doc comments name "adversarial
//! partial-fill + cancel/replace" stress as the original (Patch B2) intent,
//! but the current `BacktestEngine` has no partial-fill model at all (every
//! order intent is filled in full or not filled -- see
//! `OrderStatus`/allocation-cap comments in `types.rs`) and no order
//! lifecycle that could be "canceled and replaced" mid-flight. Building
//! either would be a broad rewrite of the accepted, extensively-tested
//! causal execution model (`scenario_future_execution_bkt01*.rs`,
//! `scenario_no_same_bar_fill_by_default.rs`,
//! `scenario_lookahead_bias_proof.rs`) -- out of scope for this patch per
//! the mission's own hard-stop list ("rewriting accepted causal execution").
//!
//! Instead, this module builds real adversarial evidence from execution
//! knobs the engine genuinely supports today, covering the *intent* behind
//! the two named adversities with the closest real analogue:
//!
//! - **Cost adversity** (stands in for "fills are worse than expected"):
//!   `cost_stress_2x`/`cost_stress_3x` re-run the exact same strategy/bars
//!   with `StressProfile`/`CommissionModel` scaled 2x/3x. These are also
//!   the first two scenarios P9 (`BKT-ROBUSTNESS-GAUNTLET-01`) is scoped to
//!   require, so this patch does not duplicate machinery P9 will need.
//! - **Cancel/halt adversity** (stands in for "an order does not execute as
//!   intended"): `conservative_risk_limits` re-runs under
//!   `BacktestConfig::conservative_defaults()`'s own accepted
//!   daily-loss/max-drawdown ratios (2% / 18%, from `base.yaml`) instead of
//!   the candidate's own limits -- genuinely exercising the real
//!   `OrderStatus::{HaltTriggered, CanceledOnHalt}` / flatten-on-halt path,
//!   using an already-accepted production config rather than an invented
//!   threshold.
//!
//! All three scenarios share one pass criterion, applied uniformly:
//! the run must complete without a `BacktestError`, end with positive
//! equity (no bankruptcy), and never draw down past the accepted
//! conservative max-drawdown fraction (computed from
//! `BacktestConfig::conservative_defaults()`, never hardcoded here).

use uuid::Uuid;

use crate::types::{BacktestBar, BacktestConfig, BacktestReport};
use crate::BacktestEngine;
use mqk_strategy::Strategy;

/// Protocol version for [`StressSuiteRunOutput`]. Bump whenever the scenario
/// set or pass criterion changes in a way that makes an older result
/// incomparable to a newer one.
pub const STRESS_SUITE_PROTOCOL_VERSION: &str = "bkt_stress_suite_v1";

/// The exact scenario names required under [`STRESS_SUITE_PROTOCOL_VERSION`].
/// PROMOTION-STRESS-AUTHORITY-REPAIR-01: bound 1:1 to the protocol version --
/// a durable stress artifact must carry every one of these names to be
/// accepted as evidence for that protocol. If the scenario set ever changes,
/// bump `STRESS_SUITE_PROTOCOL_VERSION` and update this list in the same
/// patch so old artifacts are never silently reinterpreted under a new set.
pub const REQUIRED_SCENARIO_NAMES: &[&str] =
    &["cost_stress_2x", "cost_stress_3x", "conservative_risk_limits"];

/// Outcome of one adversarial re-run.
#[derive(Debug, Clone, PartialEq)]
pub struct StressScenarioOutcome {
    pub name: String,
    pub passed: bool,
    /// Present exactly when `passed` is `false`.
    pub reason: Option<String>,
    pub final_equity_micros: i64,
    /// Fraction (0.0..=1.0+) of high-water-mark equity lost at the worst point.
    pub max_drawdown_fraction: f64,
}

/// Full stress suite result for one candidate run.
#[derive(Debug, Clone, PartialEq)]
pub struct StressSuiteRunOutput {
    pub protocol_version: String,
    /// Candidate/run binding -- the baseline `BacktestReport` this suite
    /// stresses. A stress result for run A must never be accepted as
    /// evidence for run B.
    pub run_id: Uuid,
    pub config_id: Uuid,
    pub strategy_name: String,
    pub scenarios: Vec<StressScenarioOutcome>,
}

impl StressSuiteRunOutput {
    /// True iff every scenario passed and at least one scenario ran.
    pub fn all_passed(&self) -> bool {
        !self.scenarios.is_empty() && self.scenarios.iter().all(|s| s.passed)
    }
}

/// Peak-to-trough drawdown as a fraction of the high-water mark (0.0 when
/// the curve never draws down). Mirrors
/// `mqk_artifacts::compute_equity_metrics`'s `max_dd_pct` convention
/// (drawdown / high-water-mark), expressed as a fraction rather than a
/// percentage.
fn max_drawdown_fraction(starting_equity: i64, curve: &[(i64, i64)]) -> f64 {
    let mut hwm = starting_equity;
    let mut max_dd_micros: i64 = 0;
    for &(_, eq) in curve {
        if eq > hwm {
            hwm = eq;
        }
        let dd = hwm.saturating_sub(eq);
        if dd > max_dd_micros {
            max_dd_micros = dd;
        }
    }
    if hwm > 0 {
        max_dd_micros as f64 / hwm as f64
    } else {
        0.0
    }
}

/// The accepted conservative max-drawdown ceiling, as a fraction of equity.
/// Computed from `BacktestConfig::conservative_defaults()` (currently
/// 18_000_000_000 / 100_000_000_000 = 0.18) rather than duplicated as a
/// literal, so this module tracks that config if it is ever revised.
fn conservative_max_drawdown_fraction() -> f64 {
    let c = BacktestConfig::conservative_defaults();
    if c.initial_cash_micros > 0 {
        c.max_drawdown_limit_micros as f64 / c.initial_cash_micros as f64
    } else {
        0.0
    }
}

/// Build the `cost_stress_2x` / `cost_stress_3x` adversarial config: scales
/// `stress` (slippage/volatility) and `commission` by `factor`, leaving
/// every other field (risk limits, sizing, integrity, corporate actions)
/// identical to `base`.
fn cost_stress_config(base: &BacktestConfig, factor: i64) -> BacktestConfig {
    let mut cfg = base.clone();
    cfg.stress.slippage_bps = cfg.stress.slippage_bps.saturating_mul(factor);
    cfg.stress.volatility_mult_bps = cfg.stress.volatility_mult_bps.saturating_mul(factor);
    cfg.commission.per_share_micros = cfg.commission.per_share_micros.saturating_mul(factor);
    cfg.commission.bps_of_notional = cfg.commission.bps_of_notional.saturating_mul(factor);
    cfg
}

/// Build the `conservative_risk_limits` adversarial config: replaces the
/// daily-loss / max-drawdown limits with the accepted conservative ratios
/// (from `BacktestConfig::conservative_defaults()`) applied to `base`'s own
/// `initial_cash_micros`, leaving every other field identical to `base`.
fn conservative_risk_limits_config(base: &BacktestConfig) -> BacktestConfig {
    let conservative = BacktestConfig::conservative_defaults();
    let daily_loss_fraction = if conservative.initial_cash_micros > 0 {
        conservative.daily_loss_limit_micros as f64 / conservative.initial_cash_micros as f64
    } else {
        0.0
    };
    let mut cfg = base.clone();
    cfg.daily_loss_limit_micros =
        (base.initial_cash_micros as f64 * daily_loss_fraction) as i64;
    cfg.max_drawdown_limit_micros =
        (base.initial_cash_micros as f64 * conservative_max_drawdown_fraction()) as i64;
    cfg
}

fn evaluate_scenario(
    name: &str,
    config: BacktestConfig,
    bars: &[BacktestBar],
    strategy: Box<dyn Strategy>,
) -> StressScenarioOutcome {
    let initial_cash = config.initial_cash_micros;
    let mut engine = BacktestEngine::new(config);

    let fail = |reason: String| StressScenarioOutcome {
        name: name.to_string(),
        passed: false,
        reason: Some(reason),
        final_equity_micros: 0,
        max_drawdown_fraction: 0.0,
    };

    if let Err(e) = engine.add_strategy(strategy) {
        return fail(format!("add_strategy failed: {e}"));
    }

    let report = match engine.run(bars) {
        Ok(r) => r,
        Err(e) => return fail(format!("engine.run failed: {e}")),
    };

    let final_equity_micros = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash);
    let dd_fraction = max_drawdown_fraction(initial_cash, &report.equity_curve);
    let ceiling = conservative_max_drawdown_fraction();

    if final_equity_micros <= 0 {
        return StressScenarioOutcome {
            name: name.to_string(),
            passed: false,
            reason: Some(format!(
                "bankruptcy: final_equity_micros={final_equity_micros} <= 0"
            )),
            final_equity_micros,
            max_drawdown_fraction: dd_fraction,
        };
    }
    if dd_fraction > ceiling {
        return StressScenarioOutcome {
            name: name.to_string(),
            passed: false,
            reason: Some(format!(
                "max_drawdown_fraction={dd_fraction:.6} exceeds conservative ceiling={ceiling:.6}"
            )),
            final_equity_micros,
            max_drawdown_fraction: dd_fraction,
        };
    }

    StressScenarioOutcome {
        name: name.to_string(),
        passed: true,
        reason: None,
        final_equity_micros,
        max_drawdown_fraction: dd_fraction,
    }
}

/// Run the real, deterministic backtest stress suite for `baseline`.
///
/// `make_strategy` must return a **fresh** strategy instance each call
/// (mirrors the existing CLI/daemon `StrategyRegistry::instantiate` factory
/// pattern) -- each scenario runs its own independent `BacktestEngine`.
///
/// Deterministic: no RNG, no wall-clock dependence. Identical
/// `(base_config, bars, make_strategy)` inputs always produce an identical
/// `StressSuiteRunOutput`.
pub fn run_backtest_stress_suite(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: impl Fn() -> Box<dyn Strategy>,
) -> StressSuiteRunOutput {
    let scenarios = vec![
        evaluate_scenario(
            "cost_stress_2x",
            cost_stress_config(base_config, 2),
            bars,
            make_strategy(),
        ),
        evaluate_scenario(
            "cost_stress_3x",
            cost_stress_config(base_config, 3),
            bars,
            make_strategy(),
        ),
        evaluate_scenario(
            "conservative_risk_limits",
            conservative_risk_limits_config(base_config),
            bars,
            make_strategy(),
        ),
    ];

    StressSuiteRunOutput {
        protocol_version: STRESS_SUITE_PROTOCOL_VERSION.to_string(),
        run_id: baseline.run_id,
        config_id: baseline.config_id,
        strategy_name: baseline.strategy_name.clone(),
        scenarios,
    }
}
