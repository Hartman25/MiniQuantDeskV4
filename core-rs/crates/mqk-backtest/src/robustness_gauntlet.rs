//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` — deterministic robustness evidence for a
//! candidate, built on top of `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s real
//! cost-stress scenarios.
//!
//! # Investigation summary (what already exists, what this module adds, what is deferred)
//!
//! The ledger's own required scope for P9 lists eight items. Audited before
//! writing any code:
//!
//! - **2x/3x cost stress** — already real and durable via
//!   [`crate::stress_suite::run_backtest_stress_suite`]
//!   (`PROMOTION-STRESS-SUITE-AUTHORITY-01`). This module does NOT
//!   duplicate that machinery; the stress suite result is a separate,
//!   already-committed authority a caller resolves alongside this one.
//! - **execution-delay stress** — implemented here as [`DelayedStrategy`],
//!   a strategy-layer decorator that buffers and re-emits the wrapped
//!   strategy's own decisions `delay_bars` bars late. Deliberately NOT
//!   implemented by touching `BacktestEngine::resolve_pending_orders_for_batch`'s
//!   `batch_end_ts > p.signal_ts` eligibility rule (the accepted
//!   `BKT-FUTURE-EXECUTION-01` causal-execution invariant) -- that is
//!   exactly the "rewriting accepted causal execution" the wave's hard-stop
//!   list forbids. The wrapper achieves a genuine, deterministic execution
//!   lag entirely at the strategy layer, leaving the engine untouched.
//! - **symbol leave-one-out** — implemented for genuinely multi-symbol
//!   candidates; a single-symbol candidate reports this scenario
//!   `applicable: false` with an honest reason rather than fabricating a
//!   result.
//! - **month/year concentration** — implemented as a pure analysis over the
//!   baseline `BacktestReport.equity_curve` (no re-run). Regime CONTEXT
//!   (not per-bucket regime CONCENTRATION, which would require a separate
//!   per-window classification design beyond this patch's scope) is
//!   reused from the existing [`crate::regime::detect_market_regime`] and
//!   reported alongside for audit context.
//! - **parameter-neighborhood execution** — reuses the existing
//!   [`crate::sweep::run_sweep`] machinery exactly as-is (a small grid
//!   around the candidate's own slippage configuration), rather than
//!   building a second sweep engine.
//! - **shuffled/random-label placebo** — this project deliberately avoids
//!   RNG everywhere (determinism is a core invariant), so a literal random
//!   shuffle is not the right primitive here. Implemented as a deterministic
//!   temporal-offset placebo: the SAME [`DelayedStrategy`] wrapper with a
//!   delay of roughly half the run length, so the wrapped strategy's
//!   decisions are applied against a materially different point in the
//!   timeline than the one that produced them. Per the ledger's explicit
//!   instruction, if this placebo scores as well as or better than the real
//!   signal, that is reported as a genuine finding (`passed: false`) --
//!   never tuned away.
//! - **conservative execution/capacity stress** — implemented here as
//!   [`conservative_capacity_stress_scenario`]: re-runs under the SAME
//!   accepted `BacktestConfig::conservative_defaults()` daily-loss/max-
//!   drawdown ratios `stress_suite::conservative_risk_limits` already uses,
//!   combined with a halved `max_gross_exposure_mult_micros` and any
//!   candidate-declared per-position caps (never a fabricated cap the
//!   candidate didn't already opt into) -- simulating reduced market
//!   capacity/liquidity on top of conservative risk limits.
//! - **DSR/PBO sensitivity** — PROMOTION-STRESS-AUTHORITY-REPAIR-01 wave
//!   correction: an earlier version of this module deferred this item,
//!   reasoning P7A/P7B were themselves still `OPEN`. That was WRONG -- the
//!   ledger records P7A (`RESEARCH-EXECUTION-PRICING-PARITY-01`) and P7B
//!   (`RESEARCH-WEIGHT-TO-SHARE-PARITY-01`) as `ACCEPTED`/`ACCEPTED_LOCALLY,
//!   PUSHED` and FROZEN. Implemented via cross-language orchestration
//!   (`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`), which
//!   shells out to `research-py`'s `mqk_research.ml.dsr_pbo_sensitivity_cli`
//!   -- itself only calling the existing, FROZEN
//!   `mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge`
//!   multiple times under different `cscv_target_block_count` values (never
//!   redefining DSR/PBO statistics in Rust). Deliberately NOT included in
//!   [`run_robustness_gauntlet`] itself (a pure, engine-only function) --
//!   it needs subprocess/filesystem I/O the caller must supply trusted
//!   application/config state for (Python executable, `research-py` root,
//!   registry path), exactly like every other Research-registry-touching
//!   seam in this codebase. Callers assembling the complete P9 artifact
//!   call it separately and merge the result via
//!   [`RobustnessGauntletOutput::merge_dsr_pbo_sensitivity`]; until merged,
//!   it remains honestly recorded in [`RobustnessGauntletOutput::deferred`]
//!   (never silently absent) and [`RobustnessGauntletOutput::is_complete`]
//!   reports `false`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::Datelike;
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use uuid::Uuid;

use crate::regime::{detect_market_regime, MarketRegimeInput, MarketRegimePolicy};
use crate::sweep::{run_sweep, SweepGrid};
use crate::types::{BacktestBar, BacktestConfig, BacktestReport};
use crate::BacktestEngine;

pub const ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION: &str = "bkt_robustness_gauntlet_v1";

/// Every scenario name required under [`ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`]
/// for a P9 artifact to be [`RobustnessGauntletOutput::is_complete`].
/// Deliberately excludes `cost_stress_2x`/`cost_stress_3x` -- those are
/// `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s own required scenarios
/// (`mqk_backtest::REQUIRED_SCENARIO_NAMES`), a separate durable authority
/// this module reuses rather than duplicates (see module docs above).
pub const REQUIRED_ROBUSTNESS_SCENARIO_NAMES: &[&str] = &[
    "execution_delay_stress",
    "symbol_leave_one_out",
    "month_and_regime_concentration",
    "parameter_neighborhood_execution",
    "placebo_temporal_offset",
    "conservative_capacity_stress",
    crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME,
];

/// The conservative max-drawdown ceiling every re-run scenario is judged
/// against, shared with `stress_suite`'s own pass criterion (computed from
/// `BacktestConfig::conservative_defaults()`, never a hardcoded literal).
fn conservative_max_drawdown_fraction() -> f64 {
    let c = BacktestConfig::conservative_defaults();
    if c.initial_cash_micros > 0 {
        c.max_drawdown_limit_micros as f64 / c.initial_cash_micros as f64
    } else {
        0.0
    }
}

fn max_drawdown_fraction(starting_equity: i64, curve: &[(i64, i64)]) -> f64 {
    let mut hwm = starting_equity;
    let mut max_dd: i64 = 0;
    for &(_, eq) in curve {
        if eq > hwm {
            hwm = eq;
        }
        let dd = hwm.saturating_sub(eq);
        if dd > max_dd {
            max_dd = dd;
        }
    }
    if hwm > 0 {
        max_dd as f64 / hwm as f64
    } else {
        0.0
    }
}

/// Bankruptcy/drawdown pass check shared by every re-run scenario below.
fn clears_conservative_bar(initial_cash: i64, curve: &[(i64, i64)]) -> (bool, String) {
    let final_equity = curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    if final_equity <= 0 {
        return (
            false,
            format!("bankruptcy: final_equity_micros={final_equity} <= 0"),
        );
    }
    let dd = max_drawdown_fraction(initial_cash, curve);
    let ceiling = conservative_max_drawdown_fraction();
    if dd > ceiling {
        return (
            false,
            format!("max_drawdown_fraction={dd:.6} exceeds conservative ceiling={ceiling:.6}"),
        );
    }
    (true, format!("max_drawdown_fraction={dd:.6}, final_equity_micros={final_equity}"))
}

// ---------------------------------------------------------------------------
// DelayedStrategy — strategy-layer execution-lag / temporal-offset decorator
// ---------------------------------------------------------------------------

/// Wraps any [`Strategy`] and re-emits its own decisions `delay_bars` bars
/// later than it made them. Every decision the inner strategy makes is
/// still driven by that strategy's own real, unmodified `on_bar` call (so
/// its internal state evolves exactly as it would un-wrapped) -- only the
/// OUTPUT is deferred. While the buffer is filling (the first `delay_bars`
/// calls), no targets are emitted (an empty `StrategyOutput`, which the
/// engine treats as "no new intent this bar" -- existing positions are
/// simply carried forward, never force-flattened).
struct DelayedStrategy {
    inner: Box<dyn Strategy>,
    delay_bars: usize,
    buffer: VecDeque<StrategyOutput>,
}

impl DelayedStrategy {
    fn new(inner: Box<dyn Strategy>, delay_bars: usize) -> Self {
        Self {
            inner,
            delay_bars,
            buffer: VecDeque::with_capacity(delay_bars + 1),
        }
    }
}

impl Strategy for DelayedStrategy {
    fn spec(&self) -> StrategySpec {
        self.inner.spec()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let real = self.inner.on_bar(ctx);
        self.buffer.push_back(real);
        if self.buffer.len() > self.delay_bars {
            self.buffer.pop_front().expect("just checked len > delay_bars >= 0")
        } else {
            StrategyOutput::new(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct RobustnessScenarioOutcome {
    pub name: String,
    /// `false` when this scenario's real-world precondition is not met by
    /// this candidate (e.g. leave-one-out on a single-symbol run) -- an
    /// honest "not applicable", never a fabricated pass or fail.
    pub applicable: bool,
    /// Meaningless when `applicable == false`.
    pub passed: bool,
    pub reason: Option<String>,
    pub detail: String,
    /// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: the exact Research
    /// `trial_id` this scenario's evidence was computed against, when the
    /// scenario is Research-registry-anchored (currently only
    /// `dsr_pbo_sensitivity` -- every pure-engine scenario leaves this
    /// `None`). Durably carried through the artifact so a later promotion
    /// decision can prove the SAME trial produced both this P9 evidence and
    /// the P7C/OOS evidence, never merely that both share a `strategy_id`.
    pub research_trial_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredScenario {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RobustnessGauntletOutput {
    pub protocol_version: String,
    pub run_id: Uuid,
    pub config_id: Uuid,
    pub strategy_name: String,
    pub scenarios: Vec<RobustnessScenarioOutcome>,
    pub deferred: Vec<DeferredScenario>,
}

impl RobustnessGauntletOutput {
    /// True iff every APPLICABLE scenario passed, and at least one scenario
    /// was applicable. A candidate for which every scenario was
    /// inapplicable (degenerate/empty run) is never vacuously "robust".
    pub fn all_applicable_passed(&self) -> bool {
        let applicable: Vec<&RobustnessScenarioOutcome> =
            self.scenarios.iter().filter(|s| s.applicable).collect();
        !applicable.is_empty() && applicable.iter().all(|s| s.passed)
    }

    /// True iff every scenario REQUIRED under [`ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`]
    /// ([`REQUIRED_ROBUSTNESS_SCENARIO_NAMES`]) is present in `scenarios` by
    /// name AND `deferred` is empty.
    ///
    /// Distinct from [`Self::all_applicable_passed`]: completeness is about
    /// EVIDENCE COVERAGE (every required slice was genuinely evaluated one
    /// way or another -- pass, fail, or an honest `applicable: false` -- not
    /// silently missing or still deferred), never about whether the
    /// candidate's own numbers were good. A candidate can be complete and
    /// still fail (`all_applicable_passed() == false`); it can never be
    /// incomplete and reported as a promotion-grade pass -- callers must
    /// check both.
    pub fn is_complete(&self) -> bool {
        if !self.deferred.is_empty() {
            return false;
        }
        let present: std::collections::BTreeSet<&str> =
            self.scenarios.iter().map(|s| s.name.as_str()).collect();
        REQUIRED_ROBUSTNESS_SCENARIO_NAMES
            .iter()
            .all(|required| present.contains(required))
    }

    /// Merge a separately-computed
    /// [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`] result
    /// into this output: appends it to `scenarios` and removes the matching
    /// entry from `deferred` (a no-op if none exists), so
    /// [`Self::is_complete`] can become `true` once every required scenario
    /// has genuinely been evaluated. Never call this with a scenario whose
    /// `name` is not [`crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME`]
    /// -- kept name-agnostic only to avoid a redundant assertion the type
    /// system doesn't otherwise need.
    pub fn merge_dsr_pbo_sensitivity(mut self, outcome: RobustnessScenarioOutcome) -> Self {
        self.deferred.retain(|d| d.name != outcome.name);
        self.scenarios.push(outcome);
        self
    }
}

// ---------------------------------------------------------------------------
// Scenario implementations
// ---------------------------------------------------------------------------

fn run_with_strategy(
    config: BacktestConfig,
    bars: &[BacktestBar],
    strategy: Box<dyn Strategy>,
) -> Result<BacktestReport, String> {
    let mut engine = BacktestEngine::new(config);
    engine
        .add_strategy(strategy)
        .map_err(|e| format!("add_strategy failed: {e}"))?;
    engine.run(bars).map_err(|e| format!("engine.run failed: {e}"))
}

fn execution_delay_scenario(
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "execution_delay_stress".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let delayed = DelayedStrategy::new(make_strategy(), 1);
    match run_with_strategy(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let (passed, detail) = clears_conservative_bar(initial_cash, &report.equity_curve);
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed { None } else { Some(detail.clone()) },
                detail,
                research_trial_id: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
        },
    }
}

fn symbol_leave_one_out_scenario(
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "symbol_leave_one_out".to_string();
    let symbols: BTreeSet<&str> = bars.iter().map(|b| b.symbol.as_str()).collect();
    if symbols.len() < 2 {
        return RobustnessScenarioOutcome {
            name,
            applicable: false,
            passed: false,
            reason: Some(
                "single-symbol backtest; leave-one-out requires 2+ distinct symbols".to_string(),
            ),
            detail: format!("distinct_symbols={}", symbols.len()),
            research_trial_id: None,
        };
    }

    let initial_cash = base_config.initial_cash_micros;
    let mut worst: Option<(String, f64)> = None;
    for symbol in &symbols {
        let filtered: Vec<BacktestBar> =
            bars.iter().filter(|b| b.symbol != *symbol).cloned().collect();
        let report = match run_with_strategy(base_config.clone(), &filtered, make_strategy()) {
            Ok(r) => r,
            Err(e) => {
                return RobustnessScenarioOutcome {
                    name,
                    applicable: true,
                    passed: false,
                    reason: Some(format!("excluding {symbol}: {e}")),
                    detail: format!("excluding {symbol}: {e}"),
                    research_trial_id: None,
                }
            }
        };
        let dd = max_drawdown_fraction(initial_cash, &report.equity_curve);
        if worst.as_ref().map(|(_, d)| dd > *d).unwrap_or(true) {
            worst = Some((symbol.to_string(), dd));
        }
        let (passed, _) = clears_conservative_bar(initial_cash, &report.equity_curve);
        if !passed {
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(format!(
                    "excluding symbol {symbol} breaches the conservative bar (max_drawdown_fraction={dd:.6})"
                )),
                detail: format!("worst excluded symbol so far: {symbol} dd={dd:.6}"),
                research_trial_id: None,
            };
        }
    }

    let (worst_symbol, worst_dd) = worst.expect("symbols.len() >= 2 checked above");
    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed: true,
        reason: None,
        detail: format!(
            "{} symbols tested; worst excluded symbol={worst_symbol} max_drawdown_fraction={worst_dd:.6}",
            symbols.len()
        ),
        research_trial_id: None,
    }
}

fn month_and_regime_concentration_scenario(baseline: &BacktestReport, bars: &[BacktestBar]) -> RobustnessScenarioOutcome {
    let name = "month_and_regime_concentration".to_string();

    let mut monthly_gain: BTreeMap<(i32, u32), i64> = BTreeMap::new();
    let mut prev_equity: Option<i64> = None;
    for &(ts, eq) in &baseline.equity_curve {
        if let Some(prev) = prev_equity {
            if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
                let key = (dt.year(), dt.month());
                *monthly_gain.entry(key).or_insert(0) += eq - prev;
            }
        }
        prev_equity = Some(eq);
    }

    // Concentration is only a meaningful signal across 2+ distinct calendar
    // months of equity-curve data -- a run confined to a single month is
    // trivially "100% concentrated" by construction, not evidence of
    // fragility. Reported honestly as not-applicable rather than a
    // fabricated pass or fail.
    if monthly_gain.len() < 2 {
        return RobustnessScenarioOutcome {
            name,
            applicable: false,
            passed: false,
            reason: Some(format!(
                "run spans only {} distinct calendar month(s); concentration analysis requires 2+",
                monthly_gain.len()
            )),
            detail: format!("distinct_months={}", monthly_gain.len()),
            research_trial_id: None,
        };
    }

    let total_positive: i64 = monthly_gain.values().filter(|&&v| v > 0).sum();
    let worst_month = monthly_gain
        .iter()
        .filter(|(_, &v)| v > 0)
        .max_by_key(|(_, &v)| v);

    let concentration_fraction = match (total_positive, worst_month) {
        (tp, Some((_, &max_gain))) if tp > 0 => max_gain as f64 / tp as f64,
        _ => 0.0,
    };
    let passed = total_positive <= 0 || concentration_fraction <= 0.5;

    let regime_context = {
        let policy = MarketRegimePolicy::conservative_defaults();
        let input = MarketRegimeInput::from_bars(bars.to_vec(), None::<String>, None::<String>);
        let classification = detect_market_regime(&input, &policy);
        format!("{:?}", classification.kind)
    };

    let month_desc = worst_month
        .map(|((y, m), &g)| format!("{y}-{m:02} (gain_micros={g})"))
        .unwrap_or_else(|| "none".to_string());

    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed,
        reason: if passed {
            None
        } else {
            Some(format!(
                "worst single month contributed {concentration_fraction:.4} of total positive \
                 gain (> 0.5 ceiling): {month_desc}"
            ))
        },
        detail: format!(
            "months_with_gain_data={}, worst_month={month_desc}, concentration_fraction={concentration_fraction:.4}, whole_run_regime={regime_context}",
            monthly_gain.len()
        ),
        research_trial_id: None,
    }
}

fn parameter_neighborhood_scenario(
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "parameter_neighborhood_execution".to_string();
    let base_slippage = base_config.stress.slippage_bps;
    let grid = SweepGrid {
        target_qty: vec![base_config.sizing.target_qty.max(1)],
        slippage_bps: vec![base_slippage, base_slippage + 5, base_slippage + 10],
        volatility_mult_bps: Vec::new(),
        max_target_qty: vec![base_config.sizing.max_target_qty],
        max_position_notional_usd: vec![base_config.sizing.max_position_notional_usd],
    };

    let rows = match run_sweep(bars, base_config, &grid, |_pt| Some(make_strategy())) {
        Ok(r) => r,
        Err(e) => {
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(e.to_string()),
                detail: e.to_string(),
                research_trial_id: None,
            }
        }
    };

    let ceiling = conservative_max_drawdown_fraction() * 100.0; // rows report pct, not fraction
    let worst_row = rows
        .iter()
        .max_by(|a, b| a.max_drawdown_pct.partial_cmp(&b.max_drawdown_pct).unwrap());
    let passed = rows
        .iter()
        .all(|r| r.max_drawdown_pct <= ceiling && !r.halted);

    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed,
        reason: if passed {
            None
        } else {
            Some(format!(
                "at least one neighboring parameter point breached the conservative drawdown \
                 ceiling ({ceiling:.4}%) or halted"
            ))
        },
        detail: format!(
            "{} neighborhood points tested; worst max_drawdown_pct={:.4}%",
            rows.len(),
            worst_row.map(|r| r.max_drawdown_pct).unwrap_or(0.0)
        ),
        research_trial_id: None,
    }
}

fn placebo_temporal_offset_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "placebo_temporal_offset".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let delay_bars = (bars.len() / 2).max(1);
    let delayed = DelayedStrategy::new(make_strategy(), delay_bars);

    let baseline_final = baseline
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash);

    match run_with_strategy(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let placebo_final = report
                .equity_curve
                .last()
                .map(|(_, eq)| *eq)
                .unwrap_or(initial_cash);
            // Per the ledger's explicit hard stop: if the placebo performs
            // as well as or better than the real signal, that is a genuine
            // finding to report -- never tuned away.
            let passed = placebo_final < baseline_final;
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed {
                    None
                } else {
                    Some(format!(
                        "placebo (delay={delay_bars} bars) final_equity_micros={placebo_final} \
                         is NOT worse than the real signal's baseline_final_equity_micros={baseline_final} \
                         -- this candidate's real signal may not be distinguishable from a \
                         temporally-decorrelated one; reported as found, not adjusted away"
                    ))
                },
                detail: format!(
                    "delay_bars={delay_bars}, baseline_final_equity_micros={baseline_final}, \
                     placebo_final_equity_micros={placebo_final}"
                ),
                research_trial_id: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
        },
    }
}

/// Build the `conservative_capacity_stress` adversarial config: the SAME
/// accepted conservative daily-loss/max-drawdown ratios
/// `stress_suite::conservative_risk_limits_config` uses, combined with a
/// halved `max_gross_exposure_mult_micros` and any candidate-declared
/// per-position caps -- reduced market capacity/liquidity on top of
/// conservative risk limits. Never fabricates a per-position cap the
/// candidate didn't already opt into (`None` stays `None`).
fn conservative_capacity_config(base: &BacktestConfig) -> BacktestConfig {
    let conservative = BacktestConfig::conservative_defaults();
    let daily_loss_fraction = if conservative.initial_cash_micros > 0 {
        conservative.daily_loss_limit_micros as f64 / conservative.initial_cash_micros as f64
    } else {
        0.0
    };
    let mut cfg = base.clone();
    cfg.daily_loss_limit_micros = (base.initial_cash_micros as f64 * daily_loss_fraction) as i64;
    cfg.max_drawdown_limit_micros =
        (base.initial_cash_micros as f64 * conservative_max_drawdown_fraction()) as i64;
    cfg.max_gross_exposure_mult_micros = (base.max_gross_exposure_mult_micros / 2).max(1);
    cfg.sizing.max_target_qty = base.sizing.max_target_qty.map(|q| (q / 2).max(1));
    cfg.sizing.max_position_notional_usd =
        base.sizing.max_position_notional_usd.map(|n| (n / 2).max(1));
    cfg
}

fn conservative_capacity_stress_scenario(
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "conservative_capacity_stress".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let cfg = conservative_capacity_config(base_config);
    match run_with_strategy(cfg, bars, make_strategy()) {
        Ok(report) => {
            let (passed, detail) = clears_conservative_bar(initial_cash, &report.equity_curve);
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed { None } else { Some(detail.clone()) },
                detail,
                research_trial_id: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the real, deterministic robustness gauntlet for `baseline`. See the
/// module docs for exactly which of P9's eight scoped items are
/// implemented here versus honestly deferred.
pub fn run_robustness_gauntlet(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: impl Fn() -> Box<dyn Strategy>,
) -> RobustnessGauntletOutput {
    let scenarios = vec![
        execution_delay_scenario(base_config, bars, &make_strategy),
        symbol_leave_one_out_scenario(base_config, bars, &make_strategy),
        month_and_regime_concentration_scenario(baseline, bars),
        parameter_neighborhood_scenario(base_config, bars, &make_strategy),
        placebo_temporal_offset_scenario(baseline, base_config, bars, &make_strategy),
        conservative_capacity_stress_scenario(base_config, bars, &make_strategy),
    ];

    let deferred = vec![DeferredScenario {
        name: crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
        reason: "requires subprocess/filesystem I/O (Python executable, research-py root, \
                 registry path) this pure, engine-only function does not accept as input -- \
                 call crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario separately and \
                 merge it in via RobustnessGauntletOutput::merge_dsr_pbo_sensitivity before \
                 treating this artifact as complete (see RobustnessGauntletOutput::is_complete)"
            .to_string(),
    }];

    RobustnessGauntletOutput {
        protocol_version: ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION.to_string(),
        run_id: baseline.run_id,
        config_id: baseline.config_id,
        strategy_name: baseline.strategy_name.clone(),
        scenarios,
        deferred,
    }
}
