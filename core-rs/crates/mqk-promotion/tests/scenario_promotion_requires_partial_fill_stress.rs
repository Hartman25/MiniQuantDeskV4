//! Patch B2 — Stress suite gate tests for promotion evaluation.
//!
//! Validates:
//! - Promotion is blocked when `stress_suite` is `None` (suite not run).
//! - Promotion is blocked when the suite ran with 0 scenarios (invalid).
//! - Promotion is blocked when the suite failed (some scenarios failed).
//! - Promotion passes when the suite passed and metrics meet thresholds.
//! - Profit factor is computed correctly from partial fills.
//! - No phantom PnL is generated for the uncancelled portion of a partial-fill.

use std::collections::BTreeMap;

use mqk_backtest::BacktestReport;
use mqk_portfolio::{Fill, Side};
use mqk_promotion::{
    evaluate_promotion, ArtifactLock, PromotionConfig, PromotionInput, StressSuiteResult,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 180-day monotonically growing equity curve (good metrics).
fn good_equity_curve() -> Vec<(i64, i64)> {
    let day = 86_400i64;
    let mut curve = Vec::new();
    let mut equity = 1_000_000_000.0_f64; // 1000 USD in micros
    for d in 0..=180 {
        curve.push((d * day, equity as i64));
        equity *= 1.003; // ~0.3% per day
    }
    curve
}

/// Profitable round-trip fills that yield a high profit factor.
fn good_fills() -> Vec<Fill> {
    vec![
        Fill::new("SPY", Side::Buy, 100, 10_000_000, 0),
        Fill::new("SPY", Side::Sell, 100, 12_000_000, 0),
    ]
}

/// Lenient thresholds — the good equity curve + fills should comfortably clear these.
fn lenient_config() -> PromotionConfig {
    PromotionConfig {
        min_sharpe: 0.5,
        max_mdd: 0.10,
        min_cagr: 0.05,
        min_profit_factor: 1.0,
        min_profitable_months_pct: 0.40,
    }
}

fn good_report() -> BacktestReport {
    BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: good_equity_curve(),
        fills: good_fills(),
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    }
}

// ---------------------------------------------------------------------------
// Gate: stress suite not run
// ---------------------------------------------------------------------------

/// Promotion must be blocked when `stress_suite` is `None` (suite was never run).
#[test]
fn stress_suite_not_run_blocks_promotion() {
    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(),
        stress_suite: None,  // not run
        artifact_lock: None, // B6: not locked; test expects failure
    };

    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "promotion must be blocked when stress suite is not run"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("Stress suite not run"),
        "fail reason must mention 'Stress suite not run', got: {reasons}"
    );
}

// ---------------------------------------------------------------------------
// Gate: zero scenarios run is invalid
// ---------------------------------------------------------------------------

/// Promotion must be blocked when the suite ran with 0 scenarios.
/// `pass(0)` is syntactically constructible but semantically invalid.
#[test]
fn zero_scenarios_run_is_invalid_stress_suite() {
    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(),
        stress_suite: Some(StressSuiteResult::pass(0)), // 0 scenarios — invalid
        artifact_lock: None,                            // B6: not locked; test expects failure
    };

    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "promotion must be blocked when stress suite ran 0 scenarios"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("0 scenarios"),
        "fail reason must mention '0 scenarios', got: {reasons}"
    );
}

// ---------------------------------------------------------------------------
// Gate: failed stress suite blocks promotion
// ---------------------------------------------------------------------------

/// Promotion must be blocked when the stress suite ran but some scenarios failed.
#[test]
fn stress_suite_failed_scenarios_block_promotion() {
    let suite = StressSuiteResult::fail(
        5,
        3,
        vec![
            "partial-fill partial-cancel disagreement".to_string(),
            "cancel-replace sequence mis-routed".to_string(),
        ],
    );

    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(),
        stress_suite: Some(suite),
        artifact_lock: None, // B6: not locked; test expects failure
    };

    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "promotion must be blocked when stress suite failed"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("Stress suite failed"),
        "fail reason must mention 'Stress suite failed', got: {reasons}"
    );
    // Failure details are propagated into the reason string.
    assert!(
        reasons.contains("3/5"),
        "fail reason should show 3/5 pass ratio, got: {reasons}"
    );
    assert!(
        reasons.contains("partial-fill partial-cancel disagreement"),
        "fail reason should include first failed scenario description, got: {reasons}"
    );
}

// ---------------------------------------------------------------------------
// Gate: passed suite + good metrics allows promotion
// ---------------------------------------------------------------------------

/// Promotion must pass when the stress suite passed (≥1 scenario) and all
/// metric thresholds are met.
#[test]
fn stress_suite_passed_with_good_metrics_allows_promotion() {
    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(),
        stress_suite: Some(StressSuiteResult::pass(3)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")), // B6
    };

    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        decision.passed,
        "promotion must pass when stress suite passed and metrics clear thresholds; \
         fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(
        decision.fail_reasons.is_empty(),
        "no fail reasons expected, got: {:?}",
        decision.fail_reasons
    );
}

// ---------------------------------------------------------------------------
// Profit factor: partial fills
// ---------------------------------------------------------------------------

/// Validates that profit factor is computed correctly when fills represent
/// multiple partial closes of the same position.
///
/// Scenario:
///   buy  100 @ $10.00
///   sell  60 @ $12.00  → profit = (12-10) × 60 = 120 (units × price diff)
///   sell  40 @ $ 8.00  → loss   = (10- 8) × 40 =  80 (units × price diff)
///
/// Expected: PF = 120 / 80 = 1.5, num_trades = 2
#[test]
fn partial_fills_profit_factor_computed_correctly() {
    let fills = vec![
        Fill::new("SPY", Side::Buy, 100, 10_000_000, 0),
        Fill::new("SPY", Side::Sell, 60, 12_000_000, 0), // partial close at profit
        Fill::new("SPY", Side::Sell, 40, 8_000_000, 0),  // remaining at loss
    ];

    let day = 86_400i64;
    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![(0, 1_000_000_000), (180 * day, 1_100_000_000)],
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    let config = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 1.0,
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1)),
        artifact_lock: None, // B6: only checking metrics; decision.passed not tested
    };

    let decision = evaluate_promotion(&config, &input);

    assert_eq!(
        decision.metrics.num_trades, 2,
        "should count 2 partial closes as 2 trades, got {}",
        decision.metrics.num_trades
    );

    let expected_pf = 120.0_f64 / 80.0_f64; // = 1.5
    let pf = decision.metrics.profit_factor;
    assert!(
        (pf - expected_pf).abs() < 0.001,
        "profit factor should be ~1.5 for partial closes, got {pf}"
    );
}

// ---------------------------------------------------------------------------
// Profit factor: no phantom PnL after partial fill + cancel
// ---------------------------------------------------------------------------

/// Validates that no phantom PnL is generated for the unexecuted remainder of
/// a cancelled order. The fill list must only contain executed shares; the
/// promotion evaluator must not invent extra profits or losses.
///
/// Scenario: ordered 100, only 10 executed (rest cancelled — not in fills).
///   buy   10 @ $10.00
///   sell  10 @ $11.00  → profit = (11-10) × 10  (only executed qty counts)
///
/// Expected: PF = +∞ (all profit, no loss), num_trades = 1
#[test]
fn cancel_after_partial_fill_no_phantom_pnl() {
    let fills = vec![
        Fill::new("SPY", Side::Buy, 10, 10_000_000, 0), // only 10 of 100 executed
        Fill::new("SPY", Side::Sell, 10, 11_000_000, 0), // close position
    ];

    let day = 86_400i64;
    let report = BacktestReport {
        halted: false,
        halt_reason: None,
        equity_curve: vec![(0, 1_000_000_000), (180 * day, 1_100_000_000)],
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    };

    let config = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 1.0,
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1)),
        artifact_lock: None, // B6: only checking metrics; decision.passed not tested
    };

    let decision = evaluate_promotion(&config, &input);

    assert_eq!(
        decision.metrics.num_trades, 1,
        "exactly 1 round-trip (10 executed shares), got {}",
        decision.metrics.num_trades
    );
    assert!(
        decision.metrics.profit_factor.is_infinite() && decision.metrics.profit_factor > 0.0,
        "PF should be +inf (all profit, no phantom loss from cancelled qty), got {}",
        decision.metrics.profit_factor
    );
}
