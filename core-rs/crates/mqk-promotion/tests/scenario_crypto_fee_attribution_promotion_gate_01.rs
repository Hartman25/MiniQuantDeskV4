mod common;

use mqk_backtest::{
    derive_input_data_hash, derive_run_id, BacktestConfig, BacktestFill, BacktestReport,
};
use mqk_portfolio::{FeeAttributionStatus, Fill, QtyMicros, Side};

fn bf(inner: Fill) -> BacktestFill {
    let ns = uuid::Uuid::from_bytes([0u8; 16]);
    BacktestFill {
        fill_id: uuid::Uuid::new_v5(&ns, b"crypto_fee_gate_test_fill"),
        order_id: uuid::Uuid::new_v5(&ns, b"crypto_fee_gate_test_order"),
        signal_ts: 0,
        fill_ts: 0,
        inner,
    }
}

use mqk_promotion::{
    evaluate_promotion, ArtifactLock, PromotionConfig, PromotionInput, StressSuiteResult,
    REQUIRED_STRESS_PROTOCOL_VERSION,
};

/// Shared scaffolding: a monotonic equity curve + all non-fee evidence gates
/// satisfied, so the ONLY thing under test is the fee-attribution gate.
fn base_input(fills: Vec<BacktestFill>, strategy_name: &str) -> (PromotionConfig, PromotionInput) {
    let day = 86_400i64;
    let mut equity_curve = Vec::new();
    let daily_growth = 1.003;
    let mut equity = 1_000_000.0_f64;
    for d in 0..=180 {
        equity_curve.push((d * day, equity as i64));
        equity *= daily_growth;
    }

    let config_id = BacktestConfig::test_defaults().config_id();
    let input_data_hash = derive_input_data_hash(&[]);
    let run_id = derive_run_id(strategy_name, &config_id, &input_data_hash);
    let report = BacktestReport {
        equity_curve,
        strategy_name: strategy_name.to_string(),
        run_id,
        config_id,
        input_data_hash,
        fills,
        ..BacktestReport::test_fixture()
    };

    let config = PromotionConfig {
        min_sharpe: 0.5,
        max_mdd: 0.10,
        min_cagr: 0.10,
        min_profit_factor: 1.5,
        min_profitable_months_pct: 0.50,
        min_deflated_sharpe_ratio: 0.0,
        max_probability_backtest_overfitting: 1.0,
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000,
        report,
        stress_suite: Some(StressSuiteResult::pass(1, REQUIRED_STRESS_PROTOCOL_VERSION)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")),
        oos_evidence: Some(common::valid_oos_evidence_for_testing(strategy_name)),
        robustness_evidence: Some(common::valid_robustness_evidence_for_testing(strategy_name)),
    };

    (config, input)
}

/// CRYPTO-FEE-ATTRIBUTION-01: a candidate whose fills include even one
/// PendingAttribution fee must fail closed with an explicit cost-model
/// reason, regardless of how good its metrics otherwise look.
#[test]
fn candidate_with_pending_attribution_fee_fails_closed() {
    let fills = vec![
        bf(Fill::new(
            "AAPL",
            Side::Buy,
            QtyMicros::from_whole_units(100).unwrap(),
            10_000_000,
            0,
        )),
        bf(Fill::new(
            "AAPL",
            Side::Sell,
            QtyMicros::from_whole_units(100).unwrap(),
            12_000_000,
            0,
        )),
        // PROMOTION-FRACTIONAL-QTY-01 negative control:
        // use the actual minimum supported BTC quantity (0.0001 BTC,
        // QtyMicros raw=100). Promotion metric computation must accept this
        // canonical fractional quantity without whole-unit coercion or panic.
        bf(Fill::new_with_fee_attribution(
            "BTC/USD",
            Side::Buy,
            QtyMicros::new(100),
            50_000_000_000,
            0,
            FeeAttributionStatus::PendingAttribution,
        )),
        bf(Fill::new_with_fee_attribution(
            "BTC/USD",
            Side::Sell,
            QtyMicros::new(100),
            52_000_000_000,
            0,
            FeeAttributionStatus::PendingAttribution,
        )),
    ];
    let (config, input) = base_input(fills, "crypto_pending_fee_strategy");

    let decision = evaluate_promotion(&config, &input);

    assert!(
        !decision.passed,
        "a candidate with unattributed crypto fee evidence must not pass promotion"
    );
    assert!(
        decision
            .fail_reasons
            .iter()
            .any(|r| r.contains("Cost model not validated") && r.contains("BTC/USD")),
        "fail_reasons must name the cost-model gate and the affected symbol; got {:?}",
        decision.fail_reasons
    );
}

/// Negative control: an all-equity candidate (every fill Confirmed) must
/// never trip the cost-model gate — it is scoped to PendingAttribution
/// fills only, not a blanket new restriction on promotion.
#[test]
fn all_equity_candidate_is_unaffected_by_the_cost_model_gate() {
    let fills = vec![
        bf(Fill::new(
            "AAPL",
            Side::Buy,
            QtyMicros::from_whole_units(100).unwrap(),
            10_000_000,
            0,
        )),
        bf(Fill::new(
            "AAPL",
            Side::Sell,
            QtyMicros::from_whole_units(100).unwrap(),
            12_000_000,
            0,
        )),
    ];
    let (config, input) = base_input(fills, "all_equity_unaffected_strategy");

    let decision = evaluate_promotion(&config, &input);

    assert!(
        !decision
            .fail_reasons
            .iter()
            .any(|r| r.contains("Cost model not validated")),
        "an all-equity candidate must never trip the crypto cost-model gate; got {:?}",
        decision.fail_reasons
    );
}

/// A crypto fill whose fee IS Confirmed (e.g. a future validated cost model,
/// or a genuine broker-confirmed $0 activity) must not trip this gate --
/// proves the gate keys on attribution status, not on the symbol alone.
#[test]
fn crypto_fill_with_confirmed_fee_does_not_trip_the_gate() {
    let fills = vec![
        bf(Fill::new(
            "AAPL",
            Side::Buy,
            QtyMicros::from_whole_units(100).unwrap(),
            10_000_000,
            0,
        )),
        bf(Fill::new(
            "AAPL",
            Side::Sell,
            QtyMicros::from_whole_units(100).unwrap(),
            12_000_000,
            0,
        )),
        bf(Fill::new_with_fee_attribution(
            "BTC/USD",
            Side::Buy,
            QtyMicros::new(100),
            50_000_000_000,
            5_000,
            FeeAttributionStatus::Confirmed,
        )),
        bf(Fill::new_with_fee_attribution(
            "BTC/USD",
            Side::Sell,
            QtyMicros::new(100),
            52_000_000_000,
            5_000,
            FeeAttributionStatus::Confirmed,
        )),
    ];
    let (config, input) = base_input(fills, "crypto_confirmed_fee_strategy");

    let decision = evaluate_promotion(&config, &input);

    assert!(
        !decision
            .fail_reasons
            .iter()
            .any(|r| r.contains("Cost model not validated")),
        "a crypto fill with a Confirmed fee attribution must not trip the cost-model gate; \
         got {:?}",
        decision.fail_reasons
    );
}
