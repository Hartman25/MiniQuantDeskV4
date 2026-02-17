use mqk_backtest::BacktestReport;
use mqk_promotion::{evaluate_promotion, PromotionDecision, PromotionThresholds};
use std::collections::BTreeMap;

#[test]
fn passes_when_above_thresholds() {
    let day = 86_400i64;
    let month = 30 * day;

    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![
            (0, 1_000_000),
            (month, 1_100_000),
            (2 * month, 1_210_000),
            (3 * month, 1_331_000),
        ],
        fills: vec![],
        last_prices: BTreeMap::new(),
    };

    let thr = PromotionThresholds {
        cagr_min: 0.01,
        mdd_max: 0.20,
        sharpe_min: 0.0,
        profit_factor_min: 1.0,
        profitable_months_min: 0.5,
    };

    let r = evaluate_promotion(&report, thr);
    assert_eq!(r.decision, PromotionDecision::Pass);
    assert!(r.reasons.is_empty());
}
