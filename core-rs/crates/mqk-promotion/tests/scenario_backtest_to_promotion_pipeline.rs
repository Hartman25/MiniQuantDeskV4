//! PATCH 15e — Backtest → promotion pipeline integration test
//!
//! Validates: End-to-end: backtest engine → report → promotion evaluator
//!
//! GREEN when:
//! - A profitable backtest report passes promotion with correct metrics.
//! - An unprofitable backtest report fails promotion with correct reason codes.
//! - Metrics computed from BacktestReport are consistent with evaluator output.

use std::collections::BTreeMap;

use mqk_backtest::BacktestReport;
use mqk_portfolio::{Fill, Side};
use mqk_promotion::{evaluate_promotion, PromotionConfig, PromotionInput, StressSuiteResult};

/// Build a profitable BacktestReport: steady equity growth, profitable fills.
fn make_profitable_report() -> BacktestReport {
    let day = 86_400i64;

    // 180 days of steady ~0.3% daily growth (monotonic => MDD ≈ 0)
    let mut equity_curve = Vec::new();
    let mut equity = 1_000_000.0_f64;
    for d in 0..=180 {
        equity_curve.push((d * day, equity as i64));
        equity *= 1.003;
    }

    // Profitable round-trip trades
    let fills = vec![
        Fill::new("AAPL", Side::Buy, 100, 10_000_000, 0),
        Fill::new("AAPL", Side::Sell, 100, 12_000_000, 0),
        Fill::new("MSFT", Side::Buy, 50, 20_000_000, 0),
        Fill::new("MSFT", Side::Sell, 50, 25_000_000, 0),
    ];

    BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve,
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    }
}

/// Build an unprofitable BacktestReport: declining equity, losing fills.
fn make_unprofitable_report() -> BacktestReport {
    let day = 86_400i64;

    // 180 days of declining equity (lose ~0.2% per day)
    let mut equity_curve = Vec::new();
    let mut equity = 1_000_000.0_f64;
    for d in 0..=180 {
        equity_curve.push((d * day, equity as i64));
        equity *= 0.998;
    }

    // Losing round-trip trades
    let fills = vec![
        Fill::new("AAPL", Side::Buy, 100, 12_000_000, 0),
        Fill::new("AAPL", Side::Sell, 100, 10_000_000, 0), // loss
        Fill::new("MSFT", Side::Buy, 50, 25_000_000, 0),
        Fill::new("MSFT", Side::Sell, 50, 20_000_000, 0), // loss
    ];

    BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve,
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    }
}

/// Lenient config that profitable report should easily pass.
fn lenient_config() -> PromotionConfig {
    PromotionConfig {
        min_sharpe: 0.5,
        max_mdd: 0.10,
        min_cagr: 0.05,
        min_profit_factor: 1.0,
        min_profitable_months_pct: 0.40,
    }
}

/// Strict config that unprofitable report should clearly fail.
fn strict_config() -> PromotionConfig {
    PromotionConfig {
        min_sharpe: 1.0,
        max_mdd: 0.05,
        min_cagr: 0.10,
        min_profit_factor: 1.5,
        min_profitable_months_pct: 0.60,
    }
}

#[test]
fn profitable_backtest_passes_promotion() {
    let report = make_profitable_report();
    let config = lenient_config();

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1)),
    };

    let decision = evaluate_promotion(&config, &input);

    assert!(
        decision.passed,
        "profitable backtest should pass lenient promotion, fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(
        decision.fail_reasons.is_empty(),
        "no fail reasons expected: {:?}",
        decision.fail_reasons
    );

    // Verify metrics are reasonable for a profitable equity curve
    assert!(
        decision.metrics.cagr > 0.05,
        "CAGR should exceed min_cagr, got {}",
        decision.metrics.cagr
    );
    assert!(
        decision.metrics.mdd < 0.01,
        "MDD should be near zero for monotonic curve, got {}",
        decision.metrics.mdd
    );
    assert!(
        decision.metrics.profit_factor >= 1.0,
        "profit factor should be >= 1.0 for profitable trades, got {}",
        decision.metrics.profit_factor
    );
    assert!(
        decision.metrics.num_trades > 0,
        "should have at least 1 round-trip trade"
    );
}

#[test]
fn unprofitable_backtest_fails_promotion() {
    let report = make_unprofitable_report();
    let config = strict_config();

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
        stress_suite: None,
    };

    let decision = evaluate_promotion(&config, &input);

    assert!(
        !decision.passed,
        "unprofitable backtest should fail strict promotion"
    );
    assert!(
        !decision.fail_reasons.is_empty(),
        "should have at least one fail reason"
    );

    // Check that the correct reasons are provided
    let reasons_str = decision.fail_reasons.join("; ");

    // Negative CAGR should fail min_cagr
    assert!(
        decision.metrics.cagr < strict_config().min_cagr,
        "CAGR should be below min threshold"
    );

    // Declining curve should have MDD > 0
    assert!(
        decision.metrics.mdd > 0.0,
        "MDD should be > 0 for declining equity"
    );

    // Losing trades: profit factor < 1.0
    assert!(
        decision.metrics.profit_factor < strict_config().min_profit_factor,
        "profit factor should be below min threshold, got {}",
        decision.metrics.profit_factor
    );

    // Verify reason codes reference the right metrics
    // At minimum, CAGR and Sharpe should fail
    assert!(
        reasons_str.contains("CAGR")
            || reasons_str.contains("Sharpe")
            || reasons_str.contains("Profit factor"),
        "fail reasons should mention specific metric failures, got: {reasons_str}"
    );
}

#[test]
fn halted_backtest_metrics_computed_from_partial_curve() {
    let day = 86_400i64;

    // Equity curve that rises then the backtest halted early
    let equity_curve = vec![
        (0_i64, 1_000_000),
        (day, 1_003_000),
        (2 * day, 1_006_000),
        // Halted at day 3
    ];

    let fills = vec![
        Fill::new("AAPL", Side::Buy, 10, 10_000_000, 0),
        Fill::new("AAPL", Side::Sell, 10, 10_500_000, 0),
    ];

    let report = BacktestReport {
        halted: true,
        halt_reason: Some("daily_loss_limit".to_string()),
        equity_curve,
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    let config = lenient_config();
    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1)),
    };

    let decision = evaluate_promotion(&config, &input);

    // Metrics should be computed from partial curve (not crash)
    assert!(
        decision.metrics.num_trades > 0,
        "should compute metrics from partial run"
    );
    assert!(
        decision.metrics.duration_days > 0.0,
        "duration should be > 0 from partial curve"
    );
}
