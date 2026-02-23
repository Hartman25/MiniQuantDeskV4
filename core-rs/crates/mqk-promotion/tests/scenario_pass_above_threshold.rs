use std::collections::BTreeMap;

use mqk_backtest::BacktestReport;
use mqk_portfolio::{Fill, Side};
use mqk_promotion::{
    build_report, evaluate_promotion, write_promotion_report_json, ArtifactLock, PromotionConfig,
    PromotionInput, StressSuiteResult,
};

/// Create an equity curve + fills that pass all thresholds.
/// Equity grows monotonically with daily granularity over 6 months.
/// Fills create profitable round-trip trades => good profit factor.
#[test]
fn passes_all_thresholds() {
    let day = 86_400i64;

    // Build daily equity curve over 180 days with steady ~0.33% daily growth
    // (compounding to ~177% annualized). Monotonically increasing => MDD = 0.
    let mut equity_curve = Vec::new();
    let daily_growth = 1.003; // ~0.3% per day
    let mut equity = 1_000_000.0_f64;
    for d in 0..=180 {
        equity_curve.push((d * day, equity as i64));
        equity *= daily_growth;
    }

    // Create fills: 3 profitable round-trip trades (buy then sell at higher price)
    let fills = vec![
        // Trade 1: buy 100 @ 10.00, sell 100 @ 12.00 => profit 200
        Fill::new("AAPL", Side::Buy, 100, 10_000_000, 0),
        Fill::new("AAPL", Side::Sell, 100, 12_000_000, 0),
        // Trade 2: buy 50 @ 20.00, sell 50 @ 25.00 => profit 250
        Fill::new("MSFT", Side::Buy, 50, 20_000_000, 0),
        Fill::new("MSFT", Side::Sell, 50, 25_000_000, 0),
        // Trade 3: short 80 @ 15.00, cover 80 @ 12.00 => profit 240
        Fill::new("GOOG", Side::Sell, 80, 15_000_000, 0),
        Fill::new("GOOG", Side::Buy, 80, 12_000_000, 0),
    ];

    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve,
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    // Set thresholds that the above data should comfortably clear
    let config = PromotionConfig {
        min_sharpe: 0.5,
        max_mdd: 0.10,          // no drawdown in monotonic curve
        min_cagr: 0.10,         // >10% annualized
        min_profit_factor: 1.5, // all trades profitable => PF = +inf
        min_profitable_months_pct: 0.50,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")), // B6
    };

    let decision = evaluate_promotion(&config, &input);

    assert!(
        decision.passed,
        "should pass all thresholds, but got fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(
        decision.fail_reasons.is_empty(),
        "fail_reasons should be empty: {:?}",
        decision.fail_reasons
    );

    // Verify metrics are reasonable
    assert!(decision.metrics.cagr > 0.10, "CAGR should be high");
    assert!(
        decision.metrics.mdd < 0.001,
        "MDD should be ~0 for monotonic curve, got {}",
        decision.metrics.mdd
    );
    assert!(
        decision.metrics.profit_factor > 1.5,
        "PF should be high (all wins), got {}",
        decision.metrics.profit_factor
    );
    assert!(
        decision.metrics.profitable_months_pct >= 0.50,
        "profitable months should be >= 50%"
    );
    assert_eq!(decision.metrics.num_trades, 3, "should have 3 round trips");

    // Also test the report JSON artifact writer
    let report = build_report(&config, &decision, None);
    let tmp_dir = std::env::temp_dir().join(format!("mqk_promo_test_pass_{}", std::process::id()));
    let path = write_promotion_report_json(&tmp_dir, &report).unwrap();
    assert!(path.exists(), "report file should exist");

    // Verify it's valid JSON
    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(parsed["decision"]["passed"], true);

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
