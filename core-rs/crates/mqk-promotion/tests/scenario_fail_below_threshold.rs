use std::collections::BTreeMap;

use mqk_backtest::BacktestReport;
use mqk_promotion::{evaluate_promotion, PromotionConfig, PromotionInput};

/// Synthetic equity curve that clearly violates at least 2 thresholds:
/// - Flat equity => CAGR ≈ 0 (fails min_cagr = 0.10)
/// - Flat equity => Sharpe = 0 (fails min_sharpe = 0.50)
/// - No fills => profit factor = 0 (fails min_profit_factor = 1.0)
/// - Only 1 day => profitable_months_pct = 0 (fails min_profitable_months_pct = 0.50)
#[test]
fn fails_when_below_multiple_thresholds() {
    let day = 86_400i64;

    // Flat equity for 2 days — no growth, no drawdown
    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![(0, 1_000_000), (day, 1_000_000), (2 * day, 1_000_000)],
        fills: vec![],
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    let config = PromotionConfig {
        min_sharpe: 0.50,
        max_mdd: 0.20,
        min_cagr: 0.10,
        min_profit_factor: 1.0,
        min_profitable_months_pct: 0.50,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
    };

    let decision = evaluate_promotion(&config, &input);

    assert!(!decision.passed, "should fail with flat equity");
    assert!(
        decision.fail_reasons.len() >= 2,
        "expected at least 2 fail reasons, got {}: {:?}",
        decision.fail_reasons.len(),
        decision.fail_reasons
    );

    // Check specific failures are present
    let reasons_joined = decision.fail_reasons.join("; ");
    assert!(
        reasons_joined.contains("Sharpe"),
        "should mention Sharpe failure: {reasons_joined}"
    );
    assert!(
        reasons_joined.contains("CAGR"),
        "should mention CAGR failure: {reasons_joined}"
    );
}

/// Equity that drops significantly — violates MDD and CAGR thresholds.
#[test]
fn fails_with_large_drawdown() {
    let day = 86_400i64;
    let month = 30 * day;

    // Equity drops from 1M to 600K (40% drawdown), then recovers to 800K
    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![
            (0, 1_000_000),
            (month, 600_000), // 40% drawdown
            (2 * month, 700_000),
            (3 * month, 800_000), // end below start
        ],
        fills: vec![],
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    let config = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 0.20, // max 20% drawdown
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
    };

    let decision = evaluate_promotion(&config, &input);

    assert!(!decision.passed, "should fail with large drawdown");
    assert!(
        decision.fail_reasons.iter().any(|r| r.contains("MDD")),
        "should mention MDD failure: {:?}",
        decision.fail_reasons
    );
    // Verify MDD is approximately 0.40
    assert!(
        decision.metrics.mdd > 0.35,
        "MDD should be ~0.40, got {}",
        decision.metrics.mdd
    );
}
