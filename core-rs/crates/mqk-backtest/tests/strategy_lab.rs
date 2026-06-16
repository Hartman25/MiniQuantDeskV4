use mqk_backtest::{
    evaluate_strategy_lab, rank_strategy_lab_evaluations, StrategyLabDecision, StrategyLabGrade,
    StrategyLabInput, StrategyLabMetrics, StrategyLabReasonCode,
};

fn input(metrics: StrategyLabMetrics) -> StrategyLabInput {
    StrategyLabInput {
        strategy_id: "mean-reversion-v1".to_string(),
        symbol: "SPY".to_string(),
        timeframe: "1d".to_string(),
        metrics,
    }
}

fn strong_metrics() -> StrategyLabMetrics {
    StrategyLabMetrics {
        total_return: Some(32.0),
        max_drawdown: Some(8.0),
        trade_count: Some(42),
        win_rate: Some(61.0),
        profit_factor: Some(2.1),
        expectancy: Some(0.8),
        trade_frequency: Some(3.5),
        exposure: Some(45.0),
        sharpe: Some(1.8),
        buy_hold_return: Some(20.0),
        alpha_vs_benchmark: Some(12.0),
    }
}

#[test]
fn strategy_lab_strong_result_research_passes() {
    let evaluation = evaluate_strategy_lab(&input(strong_metrics()));

    assert_eq!(evaluation.decision, StrategyLabDecision::ResearchPass);
    assert_eq!(evaluation.grade, StrategyLabGrade::A);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::ResearchPassMinimumsMet));
    assert_eq!(evaluation.strategy_id, "mean-reversion-v1");
    assert_eq!(evaluation.symbol, "SPY");
    assert_eq!(evaluation.timeframe, "1d");
}

#[test]
fn strategy_lab_no_trades_is_not_pass() {
    let mut metrics = strong_metrics();
    metrics.trade_count = Some(0);

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_eq!(evaluation.decision, StrategyLabDecision::InsufficientData);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::NoTrades));
    assert!(!evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::ResearchPassMinimumsMet));
}

#[test]
fn strategy_lab_too_few_trades_is_not_pass() {
    let mut metrics = strong_metrics();
    metrics.trade_count = Some(3);

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_eq!(evaluation.decision, StrategyLabDecision::ResearchWatch);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::TooFewTrades));
}

#[test]
fn strategy_lab_high_drawdown_is_not_pass() {
    let mut metrics = strong_metrics();
    metrics.max_drawdown = Some(33.0);

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_eq!(evaluation.decision, StrategyLabDecision::ResearchFail);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::DrawdownTooHigh));
}

#[test]
fn strategy_lab_negative_return_is_not_pass() {
    let mut metrics = strong_metrics();
    metrics.total_return = Some(-1.0);

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_eq!(evaluation.decision, StrategyLabDecision::ResearchFail);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::NegativeReturn));
}

#[test]
fn strategy_lab_benchmark_underperformance_emits_reason() {
    let mut metrics = strong_metrics();
    metrics.total_return = Some(8.0);
    metrics.buy_hold_return = Some(12.0);
    metrics.alpha_vs_benchmark = None;

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_eq!(evaluation.alpha_vs_benchmark, Some(-4.0));
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::BenchmarkUnderperform));
}

#[test]
fn strategy_lab_missing_optional_metrics_does_not_crash() {
    let mut metrics = strong_metrics();
    metrics.win_rate = None;
    metrics.expectancy = None;
    metrics.exposure = None;
    metrics.sharpe = None;

    let evaluation = evaluate_strategy_lab(&input(metrics));

    assert_ne!(evaluation.decision, StrategyLabDecision::InsufficientData);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::WinRateMissing));
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::ExpectancyMissing));
}

#[test]
fn strategy_lab_deterministic_ranking_uses_identity_tiebreaks() {
    let mut a = evaluate_strategy_lab(&input(strong_metrics()));
    let mut b = evaluate_strategy_lab(&input(strong_metrics()));
    let mut c = evaluate_strategy_lab(&input(strong_metrics()));

    a.strategy_id = "b".to_string();
    b.strategy_id = "a".to_string();
    c.strategy_id = "c".to_string();
    c.total_return = Some(10.0);
    c.score = 10.0;

    let mut rows = vec![a, c, b];
    rank_strategy_lab_evaluations(&mut rows);

    assert_eq!(rows[0].strategy_id, "a");
    assert_eq!(rows[1].strategy_id, "b");
    assert_eq!(rows[2].strategy_id, "c");
}
