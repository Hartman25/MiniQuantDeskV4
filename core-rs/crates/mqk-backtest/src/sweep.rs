//! Deterministic parameter sweep engine for single-symbol backtests.
//!
//! Generates a Cartesian product of config dimensions, runs each combination
//! through the backtest engine, and returns ranked `SweepRowResult` values.
//!
//! # Rules
//! - All combinations are enumerated deterministically (stable order).
//! - `run_sweep` refuses an empty grid or one that exceeds `SWEEP_MAX_COMBINATIONS`.
//! - No auto-promotion. Callers decide what to do with the ranked results.
//! - No lookahead. Each run uses the same bar sequence.

use std::collections::{HashMap, VecDeque};

use mqk_portfolio::Side;
use mqk_strategy::Strategy;

use crate::engine::BacktestEngine;
use crate::types::{
    BacktestBar, BacktestConfig, BacktestFill, BacktestReport, StrategySizingConfig, StressProfile,
};

/// Hard cap on sweep combinations. Refuse grids larger than this.
pub const SWEEP_MAX_COMBINATIONS: usize = 100;

// ---------------------------------------------------------------------------
// SweepGrid — declarative dimensions
// ---------------------------------------------------------------------------

/// Grid of sweep dimensions. The Cartesian product is the run set.
///
/// At minimum, one of `target_qty` or `slippage_bps` must be non-empty.
/// Dimensions default to a single "no-op" value when omitted or empty:
/// - `max_target_qty`: defaults to `[None]`
/// - `max_position_notional_usd`: defaults to `[None]`
/// - `volatility_mult_bps`: defaults to `[config default]` (set via base_config)
#[derive(Clone, Debug)]
pub struct SweepGrid {
    /// Target qty values to sweep (must be ≥ 1 each; must be non-empty if slippage_bps is also empty).
    pub target_qty: Vec<i64>,
    /// Slippage floor values to sweep (bps; must be ≥ 0 each).
    pub slippage_bps: Vec<i64>,
    /// Volatility multiplier values to sweep (bps; must be ≥ 0 each).
    /// If empty, the base_config's `volatility_mult_bps` is used as the single value.
    pub volatility_mult_bps: Vec<i64>,
    /// max_target_qty values to sweep. If empty, defaults to `[None]`.
    pub max_target_qty: Vec<Option<i64>>,
    /// max_position_notional_usd values to sweep. If empty, defaults to `[None]`.
    pub max_position_notional_usd: Vec<Option<i64>>,
}

impl SweepGrid {
    /// Number of combinations (Cartesian product size).
    pub fn combination_count(&self, base_config: &BacktestConfig) -> usize {
        let tq = self.target_qty.len().max(1);
        let slip = self.slippage_bps.len().max(1);
        let vol = if self.volatility_mult_bps.is_empty() {
            1
        } else {
            self.volatility_mult_bps.len()
        };
        let mtq = if self.max_target_qty.is_empty() {
            1
        } else {
            self.max_target_qty.len()
        };
        let mno = if self.max_position_notional_usd.is_empty() {
            1
        } else {
            self.max_position_notional_usd.len()
        };
        let _ = base_config; // used only for vol default selection
        tq * slip * vol * mtq * mno
    }

    /// Enumerate all combinations in stable, deterministic order.
    ///
    /// Order: target_qty × slippage_bps × volatility_mult_bps × max_target_qty × max_position_notional_usd.
    pub fn combinations(&self, base_config: &BacktestConfig) -> Vec<SweepPoint> {
        let tq_vals: &[i64] = if self.target_qty.is_empty() {
            &[1]
        } else {
            &self.target_qty
        };
        let slip_vals: &[i64] = if self.slippage_bps.is_empty() {
            &[0]
        } else {
            &self.slippage_bps
        };

        let default_vol = base_config.stress.volatility_mult_bps;
        let vol_default_slice = [default_vol];
        let vol_vals: &[i64] = if self.volatility_mult_bps.is_empty() {
            &vol_default_slice
        } else {
            &self.volatility_mult_bps
        };

        let mtq_none: &[Option<i64>] = &[None];
        let mtq_vals: &[Option<i64>] = if self.max_target_qty.is_empty() {
            mtq_none
        } else {
            &self.max_target_qty
        };

        let mno_none: &[Option<i64>] = &[None];
        let mno_vals: &[Option<i64>] = if self.max_position_notional_usd.is_empty() {
            mno_none
        } else {
            &self.max_position_notional_usd
        };

        let mut out = Vec::new();
        for &tq in tq_vals {
            for &slip in slip_vals {
                for &vol in vol_vals {
                    for &mtq in mtq_vals {
                        for &mno in mno_vals {
                            out.push(SweepPoint {
                                target_qty: tq,
                                max_target_qty: mtq,
                                max_position_notional_usd: mno,
                                slippage_bps: slip,
                                volatility_mult_bps: vol,
                            });
                        }
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// SweepPoint — one combination
// ---------------------------------------------------------------------------

/// One point in the sweep grid — a specific combination of config parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepPoint {
    pub target_qty: i64,
    pub max_target_qty: Option<i64>,
    pub max_position_notional_usd: Option<i64>,
    /// Flat slippage floor in basis points (must be ≥ 0).
    pub slippage_bps: i64,
    /// Volatility multiplier in basis points (must be ≥ 0).
    pub volatility_mult_bps: i64,
}

// ---------------------------------------------------------------------------
// SweepRowResult — one run's output
// ---------------------------------------------------------------------------

/// Metrics summary for one sweep run, suitable for ranking.
#[derive(Clone, Debug)]
pub struct SweepRowResult {
    /// 1-based rank after sorting (set by `rank_sweep_results`).
    pub rank: usize,
    pub run_id: String,
    pub config_id: String,
    pub target_qty: i64,
    pub max_target_qty: Option<i64>,
    pub max_position_notional_usd: Option<i64>,
    pub slippage_bps: i64,
    pub volatility_mult_bps: i64,
    pub total_return_pct: f64,
    pub buy_and_hold_return_pct: Option<f64>,
    pub alpha_pct: Option<f64>,
    pub max_drawdown_pct: f64,
    /// Total fills executed.
    pub fill_count: usize,
    /// Completed FIFO round-trip trades.
    pub trade_count: usize,
    pub win_rate_pct: Option<f64>,
    pub profit_factor: Option<f64>,
    pub halted: bool,
    /// Path to the individual run artifact directory (if artifacts were written).
    pub artifact_path: Option<String>,
}

// ---------------------------------------------------------------------------
// SweepError
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SweepError {
    EmptyGrid,
    TooManyCombinations { count: usize, limit: usize },
    InvalidTargetQty { value: i64 },
    NegativeSlippage { field: &'static str, value: i64 },
    RunFailed { point_index: usize, reason: String },
}

impl core::fmt::Display for SweepError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SweepError::EmptyGrid => {
                write!(f, "sweep grid is empty — at least one combination required")
            }
            SweepError::TooManyCombinations { count, limit } => {
                write!(
                    f,
                    "sweep grid has {count} combinations which exceeds the limit of {limit}"
                )
            }
            SweepError::InvalidTargetQty { value } => {
                write!(f, "invalid target_qty {value}: must be ≥ 1")
            }
            SweepError::NegativeSlippage { field, value } => {
                write!(
                    f,
                    "negative slippage rejected: {field} = {value} bps (must be ≥ 0)"
                )
            }
            SweepError::RunFailed {
                point_index,
                reason,
            } => {
                write!(f, "sweep run at point_index={point_index} failed: {reason}")
            }
        }
    }
}

impl std::error::Error for SweepError {}

// ---------------------------------------------------------------------------
// run_sweep
// ---------------------------------------------------------------------------

/// Run a deterministic parameter sweep over a bar sequence.
///
/// # Arguments
/// - `bars`: The full bar sequence. Every combination runs over the same bars (no leakage).
/// - `base_config`: Template config. Sizing and stress fields are overwritten per combination.
///   All other fields (integrity, calendar, risk limits, commission) are preserved.
/// - `grid`: Sweep dimensions. Must produce ≤ `SWEEP_MAX_COMBINATIONS` combinations.
/// - `strategy_factory`: Called once per combination to produce a fresh strategy instance.
///   Must return `None` if the strategy cannot be constructed for this point (run is skipped).
///
/// # Returns
/// Ranked `Vec<SweepRowResult>` sorted by `alpha_pct desc`, then `max_drawdown_pct asc`,
/// then `run_id` as tie-breaker. If alpha is unavailable (no benchmark), ranking falls back
/// to `total_return_pct desc`.
pub fn run_sweep(
    bars: &[BacktestBar],
    base_config: &BacktestConfig,
    grid: &SweepGrid,
    strategy_factory: impl Fn(&SweepPoint) -> Option<Box<dyn Strategy>>,
) -> Result<Vec<SweepRowResult>, SweepError> {
    // --- validate grid ---
    let combos = grid.combinations(base_config);
    if combos.is_empty() {
        return Err(SweepError::EmptyGrid);
    }
    if combos.len() > SWEEP_MAX_COMBINATIONS {
        return Err(SweepError::TooManyCombinations {
            count: combos.len(),
            limit: SWEEP_MAX_COMBINATIONS,
        });
    }
    for (i, pt) in combos.iter().enumerate() {
        if pt.target_qty < 1 {
            return Err(SweepError::InvalidTargetQty {
                value: pt.target_qty,
            });
        }
        if pt.slippage_bps < 0 {
            return Err(SweepError::NegativeSlippage {
                field: "slippage_bps",
                value: pt.slippage_bps,
            });
        }
        if pt.volatility_mult_bps < 0 {
            return Err(SweepError::NegativeSlippage {
                field: "volatility_mult_bps",
                value: pt.volatility_mult_bps,
            });
        }
        let _ = i;
    }

    // --- run each combination ---
    let mut rows: Vec<SweepRowResult> = Vec::with_capacity(combos.len());
    for (i, pt) in combos.iter().enumerate() {
        let strategy = match strategy_factory(pt) {
            Some(s) => s,
            None => {
                return Err(SweepError::RunFailed {
                    point_index: i,
                    reason: "strategy_factory returned None".to_string(),
                });
            }
        };

        let mut cfg = base_config.clone();
        cfg.sizing = StrategySizingConfig {
            target_qty: pt.target_qty,
            max_target_qty: pt.max_target_qty,
            max_position_notional_usd: pt.max_position_notional_usd,
        };
        cfg.stress = StressProfile {
            slippage_bps: pt.slippage_bps,
            volatility_mult_bps: pt.volatility_mult_bps,
            // BKT-BAR-VOLUME-IMPACT-STRESS-01: not a swept dimension --
            // carried through from the caller's base config rather than
            // silently reset to 0.
            participation_impact_bps: base_config.stress.participation_impact_bps,
        };

        let mut engine = BacktestEngine::new(cfg);
        engine
            .add_strategy(strategy)
            .map_err(|e| SweepError::RunFailed {
                point_index: i,
                reason: format!("{e:?}"),
            })?;

        let report = engine.run(bars).map_err(|e| SweepError::RunFailed {
            point_index: i,
            reason: e.to_string(),
        })?;

        rows.push(sweep_row_from_report(&report, pt, None));
    }

    rank_sweep_results(&mut rows);
    Ok(rows)
}

// ---------------------------------------------------------------------------
// sweep_row_from_report — extract metrics from a BacktestReport
// ---------------------------------------------------------------------------

/// Extract `SweepRowResult` from a completed `BacktestReport`.
///
/// `artifact_path`: optional path to the run artifact directory (set by CLI after writing).
pub fn sweep_row_from_report(
    report: &BacktestReport,
    point: &SweepPoint,
    artifact_path: Option<String>,
) -> SweepRowResult {
    let starting = report.equity_curve.first().map(|(_, _)| 0i64).unwrap_or(0);
    let _ = starting;

    // total_return from equity curve
    let (total_return_pct, max_drawdown_pct) =
        equity_metrics(&report.equity_curve, report.fills.len());

    // buy-and-hold benchmark
    let bah = buy_and_hold_return_pct(report.first_bar_open_micros, report.last_bar_close_micros);
    let alpha = bah.map(|b| total_return_pct - b);

    // FIFO trade metrics
    let trade_pnls = fifo_pnls(&report.fills);
    let trade_count = trade_pnls.len();
    let (win_count, gross_profit, gross_loss) = win_loss_stats(&trade_pnls);
    let win_rate_pct = if trade_count > 0 {
        Some(win_count as f64 / trade_count as f64 * 100.0)
    } else {
        None
    };
    let profit_factor = if gross_loss < 0 {
        Some(gross_profit as f64 / (-gross_loss) as f64)
    } else {
        None // no losing trades (infinite profit factor) or no trades — both represented as None
    };

    SweepRowResult {
        rank: 0, // set by rank_sweep_results
        run_id: report.run_id.to_string(),
        config_id: report.config_id.to_string(),
        target_qty: point.target_qty,
        max_target_qty: point.max_target_qty,
        max_position_notional_usd: point.max_position_notional_usd,
        slippage_bps: point.slippage_bps,
        volatility_mult_bps: point.volatility_mult_bps,
        total_return_pct,
        buy_and_hold_return_pct: bah,
        alpha_pct: alpha,
        max_drawdown_pct,
        fill_count: report.fills.len(),
        trade_count,
        win_rate_pct,
        profit_factor,
        halted: report.halted,
        artifact_path,
    }
}

// ---------------------------------------------------------------------------
// Deterministic ranking
// ---------------------------------------------------------------------------

/// Sort and assign 1-based ranks to sweep results.
///
/// Primary: alpha_pct desc (if all rows have alpha); else total_return_pct desc.
/// Secondary: max_drawdown_pct asc.
/// Tie-breaker: run_id asc (lexicographic, deterministic).
pub fn rank_sweep_results(rows: &mut [SweepRowResult]) {
    let all_have_alpha = rows.iter().all(|r| r.alpha_pct.is_some());

    rows.sort_by(|a, b| {
        // Primary: higher alpha (or return if no alpha) is better → sort desc.
        let primary = if all_have_alpha {
            let alpha_a = a.alpha_pct.unwrap_or(f64::NEG_INFINITY);
            let alpha_b = b.alpha_pct.unwrap_or(f64::NEG_INFINITY);
            alpha_b
                .partial_cmp(&alpha_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            b.total_return_pct
                .partial_cmp(&a.total_return_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        // Secondary: lower drawdown is better → sort asc.
        let secondary = a
            .max_drawdown_pct
            .partial_cmp(&b.max_drawdown_pct)
            .unwrap_or(std::cmp::Ordering::Equal);
        // Tie-breaker: run_id asc (deterministic string order).
        primary.then(secondary).then(a.run_id.cmp(&b.run_id))
    });

    for (i, row) in rows.iter_mut().enumerate() {
        row.rank = i + 1;
    }
}

// ---------------------------------------------------------------------------
// Private metric helpers
// ---------------------------------------------------------------------------

fn equity_metrics(curve: &[(i64, i64)], _fill_count: usize) -> (f64, f64) {
    if curve.is_empty() {
        return (0.0, 0.0);
    }
    // For total_return we need starting equity; we don't carry it separately in the report,
    // but we can use the first curve entry as a proxy. The curve starts at bar 0's ending
    // equity, so we can't recover the exact initial cash here. Instead we look at the
    // total_return from the curve extremes relative to the baseline.
    // NOTE: BacktestReport carries fills but not initial_cash. We compute total_return from
    // the first vs last curve point as a close approximation (correct when cash is const).
    let first_eq = curve.first().map(|(_, eq)| *eq).unwrap_or(0);
    let last_eq = curve.last().map(|(_, eq)| *eq).unwrap_or(first_eq);
    let total_return_pct = if first_eq != 0 {
        (last_eq - first_eq) as f64 / first_eq as f64 * 100.0
    } else {
        0.0
    };

    let mut hwm = first_eq;
    let mut max_dd_pct: f64 = 0.0;
    for &(_, eq) in curve {
        if eq > hwm {
            hwm = eq;
        }
        if hwm > 0 {
            let dd_pct = (hwm - eq) as f64 / hwm as f64 * 100.0;
            if dd_pct > max_dd_pct {
                max_dd_pct = dd_pct;
            }
        }
    }
    (total_return_pct, max_dd_pct)
}

fn buy_and_hold_return_pct(first_open: Option<i64>, last_close: Option<i64>) -> Option<f64> {
    let first = first_open?;
    let last = last_close?;
    if first <= 0 {
        return None;
    }
    Some((last as f64 / first as f64 - 1.0) * 100.0)
}

/// Minimal FIFO round-trip PnL computation (mirrors mqk-artifacts private logic).
fn fifo_pnls(fills: &[BacktestFill]) -> Vec<i64> {
    type Lot = (i64, i64, i64); // (qty, price_micros, fee_micros)
    let mut long_lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut short_lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut pnls: Vec<i64> = Vec::new();

    for fill in fills {
        let sym = fill.symbol.clone();
        let qty = fill.qty;
        let price = fill.price_micros;
        let fee = fill.fee_micros;

        match &fill.inner.side {
            Side::Buy => {
                let mut rem_qty = qty;
                let mut rem_fee = fee;
                let shorts = short_lots.entry(sym.clone()).or_default();
                while rem_qty > 0 && !shorts.is_empty() {
                    let lot = shorts.front_mut().unwrap();
                    let cq = rem_qty.min(lot.0);
                    let open_fee_part = if lot.0 > 0 { lot.2 * cq / lot.0 } else { 0 };
                    let close_fee_part = if qty > 0 { fee * cq / qty } else { 0 };
                    pnls.push(
                        (lot.1 - price)
                            .saturating_mul(cq)
                            .saturating_sub(open_fee_part)
                            .saturating_sub(close_fee_part),
                    );
                    lot.0 -= cq;
                    lot.2 -= open_fee_part;
                    rem_qty -= cq;
                    rem_fee = rem_fee.saturating_sub(close_fee_part);
                    if lot.0 == 0 {
                        shorts.pop_front();
                    }
                }
                if rem_qty > 0 {
                    long_lots
                        .entry(sym)
                        .or_default()
                        .push_back((rem_qty, price, rem_fee));
                }
            }
            Side::Sell => {
                let mut rem_qty = qty;
                let mut rem_fee = fee;
                let longs = long_lots.entry(sym.clone()).or_default();
                while rem_qty > 0 && !longs.is_empty() {
                    let lot = longs.front_mut().unwrap();
                    let cq = rem_qty.min(lot.0);
                    let open_fee_part = if lot.0 > 0 { lot.2 * cq / lot.0 } else { 0 };
                    let close_fee_part = if qty > 0 { fee * cq / qty } else { 0 };
                    pnls.push(
                        (price - lot.1)
                            .saturating_mul(cq)
                            .saturating_sub(open_fee_part)
                            .saturating_sub(close_fee_part),
                    );
                    lot.0 -= cq;
                    lot.2 -= open_fee_part;
                    rem_qty -= cq;
                    rem_fee = rem_fee.saturating_sub(close_fee_part);
                    if lot.0 == 0 {
                        longs.pop_front();
                    }
                }
                if rem_qty > 0 {
                    short_lots
                        .entry(sym)
                        .or_default()
                        .push_back((rem_qty, price, rem_fee));
                }
            }
        }
    }
    pnls
}

fn win_loss_stats(pnls: &[i64]) -> (usize, i64, i64) {
    let mut wins = 0usize;
    let mut gross_profit: i64 = 0;
    let mut gross_loss: i64 = 0;
    for &p in pnls {
        if p > 0 {
            wins += 1;
            gross_profit = gross_profit.saturating_add(p);
        } else if p < 0 {
            gross_loss = gross_loss.saturating_add(p);
        }
    }
    (wins, gross_profit, gross_loss)
}
