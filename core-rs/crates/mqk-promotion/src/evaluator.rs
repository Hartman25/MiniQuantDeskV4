use std::cmp::Ordering;

use mqk_backtest::BacktestReport;

use crate::types::{
    PromotionCandidate, PromotionDecision, PromotionMetrics, PromotionReport, PromotionThresholds,
    TieBreakOrder, TieBreakRules,
};

/// Evaluate a backtest report against promotion thresholds.
///
/// Notes:
/// - Gate = Pass/Fail + reasons.
/// - Metrics derived from equity_curve only.
/// - "Months" are approximated as fixed 30-day buckets (ts seconds / 2_592_000).
pub fn evaluate_promotion(report: &BacktestReport, thr: PromotionThresholds) -> PromotionReport {
    let metrics = compute_metrics(report);

    let mut reasons = Vec::new();

    if metrics.cagr < thr.cagr_min {
        reasons.push(format!(
            "CAGR below threshold: {:.6} < {:.6}",
            metrics.cagr, thr.cagr_min
        ));
    }
    if metrics.max_drawdown > thr.mdd_max {
        reasons.push(format!(
            "Max drawdown above threshold: {:.6} > {:.6}",
            metrics.max_drawdown, thr.mdd_max
        ));
    }
    if metrics.sharpe < thr.sharpe_min {
        reasons.push(format!(
            "Sharpe below threshold: {:.6} < {:.6}",
            metrics.sharpe, thr.sharpe_min
        ));
    }
    if metrics.profit_factor < thr.profit_factor_min {
        reasons.push(format!(
            "Profit factor below threshold: {:.6} < {:.6}",
            metrics.profit_factor, thr.profit_factor_min
        ));
    }
    if metrics.profitable_months_frac < thr.profitable_months_min {
        reasons.push(format!(
            "Profitable months below threshold: {:.6} < {:.6}",
            metrics.profitable_months_frac, thr.profitable_months_min
        ));
    }

    let decision = if reasons.is_empty() {
        PromotionDecision::Pass
    } else {
        PromotionDecision::Fail
    };

    PromotionReport {
        decision,
        thresholds: thr,
        metrics,
        reasons,
    }
}

/// Derive promotion metrics from the equity curve.
pub fn compute_metrics(report: &BacktestReport) -> PromotionMetrics {
    let eq = &report.equity_curve;
    if eq.len() < 2 {
        return PromotionMetrics {
            cagr: 0.0,
            max_drawdown: 0.0,
            sharpe: 0.0,
            profit_factor: 1.0,
            profitable_months_frac: 0.0,
        };
    }

    let start = eq.first().unwrap();
    let end = eq.last().unwrap();

    let start_eq = start.1.max(1) as f64;
    let end_eq = end.1.max(1) as f64;

    let duration_secs = (end.0 - start.0).max(1) as f64;
    let years = duration_secs / (365.25 * 24.0 * 3600.0);
    let cagr = if years <= 0.0 {
        0.0
    } else {
        (end_eq / start_eq).powf(1.0 / years) - 1.0
    };

    let max_drawdown = compute_max_drawdown(eq);

    let returns = compute_simple_returns(eq);
    let (mean, std) = mean_std(&returns);
    let sharpe = if std <= 0.0 {
        0.0
    } else {
        (mean / std) * (returns.len() as f64).sqrt()
    };

    let (gross_pos, gross_neg) = gross_pos_neg_deltas(eq);
    let profit_factor = if gross_neg <= 0.0 { 99.0 } else { gross_pos / gross_neg };

    let profitable_months_frac = compute_profitable_months_frac(eq);

    PromotionMetrics {
        cagr,
        max_drawdown,
        sharpe,
        profit_factor,
        profitable_months_frac,
    }
}

/// Compare candidates using a composite score, then tie-break rules when within tolerance.
pub fn compare_candidates(
    a: &PromotionCandidate,
    b: &PromotionCandidate,
    rules: &TieBreakRules,
) -> Ordering {
    let sa = score(&a.metrics);
    let sb = score(&b.metrics);

    let diff = (sa - sb).abs();
    if diff > rules.within_points {
        // Higher score wins.
        return sb.partial_cmp(&sa).unwrap_or(Ordering::Equal);
    }

    for rule in &rules.order {
        let ord = match rule {
            TieBreakOrder::LowerMdd => a
                .metrics
                .max_drawdown
                .partial_cmp(&b.metrics.max_drawdown)
                .unwrap_or(Ordering::Equal),
            TieBreakOrder::HigherCagr => b
                .metrics
                .cagr
                .partial_cmp(&a.metrics.cagr)
                .unwrap_or(Ordering::Equal),
            TieBreakOrder::HigherSharpe => b
                .metrics
                .sharpe
                .partial_cmp(&a.metrics.sharpe)
                .unwrap_or(Ordering::Equal),
            TieBreakOrder::HigherProfitFactor => b
                .metrics
                .profit_factor
                .partial_cmp(&a.metrics.profit_factor)
                .unwrap_or(Ordering::Equal),
            TieBreakOrder::HigherProfitableMonths => b
                .metrics
                .profitable_months_frac
                .partial_cmp(&a.metrics.profitable_months_frac)
                .unwrap_or(Ordering::Equal),
        };

        if ord != Ordering::Equal {
            return ord;
        }
    }

    Ordering::Equal
}

fn score(m: &PromotionMetrics) -> f64 {
    // Gate is thresholds; score is only for ranking/ties.
    100.0 * m.cagr
        + 10.0 * m.sharpe
        + 5.0 * (m.profit_factor.min(10.0))
        + 20.0 * m.profitable_months_frac
        - 80.0 * m.max_drawdown
}

fn compute_max_drawdown(eq: &[(i64, i64)]) -> f64 {
    let mut peak = eq[0].1 as f64;
    let mut max_dd = 0.0;

    for p in eq {
        let e = p.1 as f64;
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

fn compute_simple_returns(eq: &[(i64, i64)]) -> Vec<f64> {
    let mut out = Vec::with_capacity(eq.len().saturating_sub(1));

    for w in eq.windows(2) {
        let a = w[0].1.max(1) as f64;
        let b = w[1].1.max(1) as f64;
        out.push((b / a) - 1.0);
    }

    out
}

fn mean_std(xs: &[f64]) -> (f64, f64) {
    if xs.is_empty() {
        return (0.0, 0.0);
    }

    let mean = xs.iter().sum::<f64>() / (xs.len() as f64);
    let var = xs
        .iter()
        .map(|x| {
            let d = x - mean;
            d * d
        })
        .sum::<f64>()
        / (xs.len() as f64);

    (mean, var.sqrt())
}

fn gross_pos_neg_deltas(eq: &[(i64, i64)]) -> (f64, f64) {
    let mut pos = 0.0;
    let mut neg = 0.0;

    for w in eq.windows(2) {
        let d = (w[1].1 - w[0].1) as f64;
        if d >= 0.0 {
            pos += d;
        } else {
            neg += -d;
        }
    }

    (pos, neg)
}

fn compute_profitable_months_frac(eq: &[(i64, i64)]) -> f64 {
    const MONTH_SECS: i64 = 30 * 24 * 60 * 60;

    if eq.len() < 2 {
        return 0.0;
    }

    let mut buckets: Vec<(i64, i64)> = Vec::new(); // (month_id, end_equity)

    for p in eq {
        let month_id = p.0 / MONTH_SECS;
        if let Some(last) = buckets.last_mut() {
            if last.0 == month_id {
                last.1 = p.1;
            } else {
                buckets.push((month_id, p.1));
            }
        } else {
            buckets.push((month_id, p.1));
        }
    }

    if buckets.len() < 2 {
        return 0.0;
    }

    let mut prof = 0u32;
    let mut total = 0u32;

    for w in buckets.windows(2) {
        total += 1;
        if w[1].1 > w[0].1 {
            prof += 1;
        }
    }

    if total == 0 {
        0.0
    } else {
        (prof as f64) / (total as f64)
    }
}
