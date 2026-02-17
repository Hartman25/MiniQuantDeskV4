use std::collections::BTreeMap;

use mqk_portfolio::{Fill, Side};

use crate::types::{
    Candidate, PromotionConfig, PromotionDecision, PromotionInput, PromotionMetrics,
    PromotionReport,
};

// ============================================================================
// Public API
// ============================================================================

/// Evaluate a single candidate against promotion thresholds.
pub fn evaluate_promotion(
    config: &PromotionConfig,
    input: &PromotionInput,
) -> PromotionDecision {
    let metrics = compute_metrics(input);
    let mut fail_reasons = Vec::new();

    // Gate checks — stable ordering matches field order in PromotionConfig.
    if metrics.sharpe < config.min_sharpe {
        fail_reasons.push(format!(
            "Sharpe {:.6} < min {:.6}",
            metrics.sharpe, config.min_sharpe
        ));
    }
    if metrics.mdd > config.max_mdd {
        fail_reasons.push(format!(
            "MDD {:.6} > max {:.6}",
            metrics.mdd, config.max_mdd
        ));
    }
    if metrics.cagr < config.min_cagr {
        fail_reasons.push(format!(
            "CAGR {:.6} < min {:.6}",
            metrics.cagr, config.min_cagr
        ));
    }
    if metrics.profit_factor < config.min_profit_factor {
        fail_reasons.push(format!(
            "Profit factor {:.6} < min {:.6}",
            metrics.profit_factor, config.min_profit_factor
        ));
    }
    if metrics.profitable_months_pct < config.min_profitable_months_pct {
        fail_reasons.push(format!(
            "Profitable months {:.6} < min {:.6}",
            metrics.profitable_months_pct, config.min_profitable_months_pct
        ));
    }

    let passed = fail_reasons.is_empty();
    PromotionDecision {
        passed,
        fail_reasons,
        metrics,
    }
}

/// Compare two passed candidates. Returns the winner id by tie-break rules:
/// 1. Higher Sharpe
/// 2. Lower MDD
/// 3. Higher CAGR
/// 4. Higher Profit Factor
/// 5. Higher profitable_months_pct
/// 6. Lexicographic candidate id
pub fn pick_winner<'a>(
    a_id: &'a str,
    a_metrics: &PromotionMetrics,
    b_id: &'a str,
    b_metrics: &PromotionMetrics,
) -> &'a str {
    // 1. Higher Sharpe
    match partial_cmp_f64(a_metrics.sharpe, b_metrics.sharpe) {
        std::cmp::Ordering::Greater => return a_id,
        std::cmp::Ordering::Less => return b_id,
        std::cmp::Ordering::Equal => {}
    }
    // 2. Lower MDD
    match partial_cmp_f64(a_metrics.mdd, b_metrics.mdd) {
        std::cmp::Ordering::Less => return a_id,
        std::cmp::Ordering::Greater => return b_id,
        std::cmp::Ordering::Equal => {}
    }
    // 3. Higher CAGR
    match partial_cmp_f64(a_metrics.cagr, b_metrics.cagr) {
        std::cmp::Ordering::Greater => return a_id,
        std::cmp::Ordering::Less => return b_id,
        std::cmp::Ordering::Equal => {}
    }
    // 4. Higher Profit Factor
    match partial_cmp_f64(a_metrics.profit_factor, b_metrics.profit_factor) {
        std::cmp::Ordering::Greater => return a_id,
        std::cmp::Ordering::Less => return b_id,
        std::cmp::Ordering::Equal => {}
    }
    // 5. Higher profitable_months_pct
    match partial_cmp_f64(
        a_metrics.profitable_months_pct,
        b_metrics.profitable_months_pct,
    ) {
        std::cmp::Ordering::Greater => return a_id,
        std::cmp::Ordering::Less => return b_id,
        std::cmp::Ordering::Equal => {}
    }
    // 6. Lexicographic
    if a_id <= b_id {
        a_id
    } else {
        b_id
    }
}

/// Select the best candidate from a list. Only passed candidates compete.
/// Returns (winner_id, winner_decision) or None if no candidate passes.
pub fn select_best(
    config: &PromotionConfig,
    candidates: &[Candidate],
) -> Option<(String, PromotionDecision)> {
    let mut best: Option<(String, PromotionDecision)> = None;

    for c in candidates {
        let decision = evaluate_promotion(config, &c.input);
        if !decision.passed {
            continue;
        }
        best = Some(match best {
            None => (c.id.clone(), decision),
            Some((prev_id, prev_decision)) => {
                let winner_id =
                    pick_winner(&prev_id, &prev_decision.metrics, &c.id, &decision.metrics);
                if winner_id == c.id {
                    (c.id.clone(), decision)
                } else {
                    (prev_id, prev_decision)
                }
            }
        });
    }

    best
}

/// Build a full PromotionReport from a decision (convenience for single eval).
pub fn build_report(
    config: &PromotionConfig,
    decision: &PromotionDecision,
    winner_id: Option<String>,
) -> PromotionReport {
    PromotionReport {
        config: *config,
        metrics: decision.metrics.clone(),
        decision: decision.clone(),
        winner_id,
    }
}

// ============================================================================
// Metric computation
// ============================================================================

/// Compute all promotion metrics from a PromotionInput.
pub fn compute_metrics(input: &PromotionInput) -> PromotionMetrics {
    let eq = &input.report.equity_curve;
    let fills = &input.report.fills;

    let (start_eq, end_eq) = if eq.is_empty() {
        (input.initial_equity_micros, input.initial_equity_micros)
    } else {
        (eq.first().unwrap().1, eq.last().unwrap().1)
    };

    let duration_secs = if eq.len() >= 2 {
        (eq.last().unwrap().0 - eq.first().unwrap().0) as f64
    } else {
        0.0
    };
    let duration_days = duration_secs / 86_400.0;
    let years = duration_secs / (365.25 * 86_400.0);

    // CAGR
    let cagr = if years <= 0.0 || start_eq <= 0 {
        0.0
    } else {
        let ratio = end_eq as f64 / start_eq as f64;
        if ratio <= 0.0 {
            0.0
        } else {
            ratio.powf(1.0 / years) - 1.0
        }
    };

    // MDD
    let mdd = compute_max_drawdown(eq);

    // Sharpe (daily-bucketed)
    let sharpe = compute_sharpe_daily(eq);

    // Profit factor (FIFO lot matcher)
    let (pf, num_trades) = compute_profit_factor(fills);

    // Profitable months % (UTC month bucketing)
    let (profitable_months_pct, num_months) = compute_profitable_months_pct(eq);

    PromotionMetrics {
        sharpe,
        mdd,
        cagr,
        profit_factor: pf,
        profitable_months_pct,
        start_equity_micros: start_eq,
        end_equity_micros: end_eq,
        duration_days,
        num_months,
        num_trades,
    }
}

// ============================================================================
// MDD
// ============================================================================

fn compute_max_drawdown(eq: &[(i64, i64)]) -> f64 {
    if eq.len() < 2 {
        return 0.0;
    }
    let mut peak = eq[0].1 as f64;
    let mut max_dd = 0.0_f64;
    for &(_, e) in eq {
        let e = e as f64;
        if e > peak {
            peak = e;
        }
        if peak > 0.0 {
            let dd = (peak - e) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

// ============================================================================
// Sharpe — daily bucketed
// ============================================================================

/// Bucket equity curve to UTC days (last equity per day), compute daily returns,
/// annualize with sqrt(252). If std == 0, Sharpe = 0.
fn compute_sharpe_daily(eq: &[(i64, i64)]) -> f64 {
    if eq.len() < 2 {
        return 0.0;
    }

    // Bucket by UTC day: day_id = ts / 86400
    let mut day_equity: BTreeMap<i64, i64> = BTreeMap::new();
    for &(ts, equity) in eq {
        let day = ts / 86_400;
        day_equity.insert(day, equity);
    }

    let day_values: Vec<i64> = day_equity.values().copied().collect();
    if day_values.len() < 2 {
        return 0.0;
    }

    // Daily returns
    let mut returns = Vec::with_capacity(day_values.len() - 1);
    for w in day_values.windows(2) {
        let prev = w[0] as f64;
        let curr = w[1] as f64;
        if prev > 0.0 {
            returns.push(curr / prev - 1.0);
        }
    }

    if returns.is_empty() {
        return 0.0;
    }

    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance = returns.iter().map(|r| (r - mean) * (r - mean)).sum::<f64>()
        / returns.len() as f64;
    let std = variance.sqrt();

    if std <= 0.0 {
        0.0
    } else {
        (mean / std) * 252.0_f64.sqrt()
    }
}

// ============================================================================
// Profit factor — FIFO lot matcher
// ============================================================================

/// Minimal FIFO lot matcher operating on fills. Returns (profit_factor, num_trades).
/// A "trade" = a round-trip: open + close.
/// PF = sum(profits) / abs(sum(losses)). No losses & profits > 0 => +INF. No trades => 0.
fn compute_profit_factor(fills: &[Fill]) -> (f64, usize) {
    // Per-symbol FIFO lots: (qty_signed, entry_price_micros)
    // Positive qty_signed = long lots, negative = short lots.
    let mut positions: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    let mut total_profit: i128 = 0;
    let mut total_loss: i128 = 0;
    let mut num_trades: usize = 0;

    for fill in fills {
        let lots = positions.entry(fill.symbol.clone()).or_default();
        let fill_qty = fill.qty; // always positive
        let fill_price = fill.price_micros;
        let fee = fill.fee_micros as i128;

        let fill_signed_qty: i64 = match fill.side {
            Side::Buy => fill_qty,
            Side::Sell => -fill_qty,
        };

        // Check if this fill closes existing lots (opposite direction)
        let existing_direction = lots.first().map(|(q, _)| q.signum()).unwrap_or(0);

        if existing_direction != 0 && existing_direction != fill_signed_qty.signum() {
            // This fill closes (partially or fully) existing lots
            let mut remaining = fill_qty; // unsigned qty to close

            while remaining > 0 && !lots.is_empty() {
                let lot = &mut lots[0];
                let lot_abs = lot.0.unsigned_abs() as i64;
                let close_qty = remaining.min(lot_abs);

                // PnL for this partial close
                let pnl: i128 = if lot.0 > 0 {
                    // Was long, now selling
                    (fill_price as i128 - lot.1 as i128) * close_qty as i128
                } else {
                    // Was short, now buying
                    (lot.1 as i128 - fill_price as i128) * close_qty as i128
                };

                if pnl > 0 {
                    total_profit += pnl;
                } else if pnl < 0 {
                    total_loss += -pnl; // store as positive
                }
                num_trades += 1;

                remaining -= close_qty;
                if close_qty == lot_abs {
                    lots.remove(0);
                } else {
                    // Reduce lot size, keep direction
                    let sign = lot.0.signum();
                    lot.0 = sign * (lot_abs - close_qty);
                }
            }

            // Subtract fee from profit side (or add to loss side)
            // Simple: treat fee as loss
            total_loss += fee;

            // If remaining > 0, this fill also opens in the new direction
            if remaining > 0 {
                lots.push((fill_signed_qty.signum() * remaining, fill_price));
            }
        } else {
            // Same direction or flat — opens new lot
            lots.push((fill_signed_qty, fill_price));
            // Fee on open reduces eventual profit (absorbed into cost basis
            // in a proper system, but here we just charge it as loss)
            total_loss += fee;
        }
    }

    if num_trades == 0 {
        return (0.0, 0);
    }

    let pf = if total_loss == 0 {
        if total_profit > 0 {
            f64::INFINITY
        } else {
            0.0
        }
    } else {
        total_profit as f64 / total_loss as f64
    };

    (pf, num_trades)
}

// ============================================================================
// Profitable months % — UTC month bucketing
// ============================================================================

/// Bucket equity curve by UTC month (year*12 + month). Last equity per month.
/// Monthly return > 0 counts as profitable. If < 2 months, result is 0.
fn compute_profitable_months_pct(eq: &[(i64, i64)]) -> (f64, usize) {
    if eq.len() < 2 {
        return (0.0, 0);
    }

    // Bucket by UTC month
    let mut month_equity: BTreeMap<i32, i64> = BTreeMap::new();
    for &(ts, equity) in eq {
        let month_id = utc_month_id(ts);
        month_equity.insert(month_id, equity);
    }

    let month_values: Vec<i64> = month_equity.values().copied().collect();
    let num_months = month_values.len();

    if num_months < 2 {
        return (0.0, num_months);
    }

    let mut profitable = 0u32;
    let mut total = 0u32;

    for w in month_values.windows(2) {
        total += 1;
        if w[1] > w[0] {
            profitable += 1;
        }
    }

    let pct = if total == 0 {
        0.0
    } else {
        profitable as f64 / total as f64
    };

    (pct, num_months)
}

/// Convert epoch seconds to a UTC month identifier (year*12 + month).
/// Minimal civil calendar conversion (no leap-second precision needed).
fn utc_month_id(epoch_secs: i64) -> i32 {
    // Days since epoch
    let days = epoch_secs.div_euclid(86_400);
    // Civil date from days since 1970-01-01 (algorithm from Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y * 12 + m) as i32
}

// ============================================================================
// Helpers
// ============================================================================

fn partial_cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}
