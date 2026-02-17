use mqk_backtest::BacktestReport;
use mqk_promotion::{evaluate_promotion, PromotionDecision, PromotionThresholds};
use std::collections::BTreeMap;

#[test]
fn fails_when_below_thresholds() {
    // Flat equity => CAGR ~0, Sharpe ~0
    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![(0, 1_000_000), (86_400, 1_000_000)],
        fills: vec![],
        last_prices: BTreeMap::new(),
    };

    let thr = PromotionThresholds {
        cagr_min: 0.05,
        mdd_max: 0.20,
        sharpe_min: 0.5,
        profit_factor_min: 1.2,
        profitable_months_min: 0.5,
    };

    let r = evaluate_promotion(&report, thr);
    assert_eq!(r.decision, PromotionDecision::Fail);
    assert!(!r.reasons.is_empty());
}
