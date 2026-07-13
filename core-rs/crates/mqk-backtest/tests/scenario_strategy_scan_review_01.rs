//! STRATEGY-SCANNER-PROMOTION-01B — pure scanner-candidate review model
//! scenario tests.
//!
//! Every test here operates on synthetic in-memory `StrategyScanCandidate`
//! values only. No file IO, no network IO, no DB access, and no broker/
//! provider environment variable is read by any test in this file.

use mqk_backtest::{
    build_review_decisions, evaluate_scan_review_decision, StrategyScanCandidate,
    StrategyScanMetrics, StrategyScanReasonCode, StrategyScanReviewPolicy,
    StrategyScanReviewState, StrategyScanTruthState,
};

/// A "clean" ranked candidate that comfortably clears every gate. Individual
/// tests override only the field(s) under test.
fn good_candidate(symbol: &str) -> StrategyScanCandidate {
    StrategyScanCandidate {
        symbol: symbol.to_string(),
        timeframe: "1D".to_string(),
        strategy_id: "swing_momentum".to_string(),
        bars_available: 400,
        truth_state: StrategyScanTruthState::CandidateRanked,
        reason_code: StrategyScanReasonCode::Ranked,
        score: Some(8.5),
        rank: Some(1),
        metrics: StrategyScanMetrics {
            total_return_pct: Some(12.0),
            benchmark_return_pct: Some(4.0),
            alpha_pct: Some(8.0),
            max_drawdown_pct: Some(10.0),
            trade_count: Some(20),
            win_rate_pct: Some(55.0),
            profit_factor: Some(1.8),
            fill_count: Some(40),
            bars_used: 400,
            data_start_ts: Some(1_700_000_000),
            data_end_ts: Some(1_735_000_000),
            halted: false,
        },
        warnings: Vec::new(),
        blockers: Vec::new(),
    }
}

fn skipped_candidate(symbol: &str, truth_state: StrategyScanTruthState) -> StrategyScanCandidate {
    StrategyScanCandidate {
        symbol: symbol.to_string(),
        timeframe: "1D".to_string(),
        strategy_id: "swing_momentum".to_string(),
        bars_available: 0,
        truth_state,
        reason_code: StrategyScanReasonCode::MissingBarsFile,
        score: None,
        rank: None,
        metrics: StrategyScanMetrics {
            bars_used: 0,
            ..Default::default()
        },
        warnings: Vec::new(),
        blockers: vec!["no local bars file found".to_string()],
    }
}

// 1. Ranked positive candidate can become `paper_candidate`.
#[test]
fn ranked_positive_candidate_becomes_paper_candidate() {
    let policy = StrategyScanReviewPolicy::default();
    let candidate = good_candidate("AAPL");
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::PaperCandidate);
    assert!(decision.reason_codes.contains(&"eligible_paper_candidate".to_string()));
    assert!(decision.blockers.is_empty());
    assert_eq!(decision.scanner_rank, Some(1));
    assert_eq!(decision.scanner_score, Some(8.5));
}

// 2. Negative absolute return blocks `paper_candidate` even if alpha is positive.
#[test]
fn negative_total_return_cannot_become_paper_candidate_even_with_positive_alpha() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("MSFT");
    candidate.metrics.total_return_pct = Some(-3.0);
    candidate.metrics.alpha_pct = Some(6.0); // beat benchmark, still lost money
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_ne!(decision.review_state, StrategyScanReviewState::PaperCandidate);
    assert_eq!(decision.review_state, StrategyScanReviewState::Rejected);
    assert!(decision.reason_codes.contains(&"negative_total_return".to_string()));
}

// 3. Missing score blocks.
#[test]
fn missing_score_blocks() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("NVDA");
    candidate.score = None;
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Blocked);
    assert!(decision.reason_codes.contains(&"missing_score".to_string()));
    assert!(!decision.blockers.is_empty());
}

// 4. Missing drawdown blocks.
#[test]
fn missing_drawdown_blocks() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("AMD");
    candidate.metrics.max_drawdown_pct = None;
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Blocked);
    assert!(decision.reason_codes.contains(&"missing_drawdown".to_string()));
}

// 5. Too few trades blocks or needs review.
#[test]
fn too_few_trades_needs_review() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("TSLA");
    candidate.metrics.trade_count = Some(policy.min_trade_count - 1);
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert!(matches!(
        decision.review_state,
        StrategyScanReviewState::NeedsReview | StrategyScanReviewState::Blocked
    ));
    assert_ne!(decision.review_state, StrategyScanReviewState::PaperCandidate);
    assert!(decision.reason_codes.contains(&"below_min_trade_count".to_string()));
}

// 6. Too few bars blocks.
#[test]
fn too_few_bars_blocks() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("GOOG");
    candidate.metrics.bars_used = policy.min_bars_used - 1;
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Blocked);
    assert!(decision.reason_codes.contains(&"below_min_bars".to_string()));
}

// 7. Excess drawdown rejects.
#[test]
fn excess_drawdown_rejects() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("META");
    candidate.metrics.max_drawdown_pct = Some(policy.max_drawdown_pct + 1.0);
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Rejected);
    assert!(decision.reason_codes.contains(&"excess_drawdown".to_string()));
}

// 8. Halted candidate rejects.
#[test]
fn halted_candidate_rejects() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("AMZN");
    candidate.metrics.halted = true;
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Rejected);
    assert!(decision.reason_codes.contains(&"halted".to_string()));
}

// 9. `data_missing` scanner candidate cannot promote.
#[test]
fn data_missing_candidate_cannot_promote() {
    let policy = StrategyScanReviewPolicy::default();
    let candidate = skipped_candidate("ZZZZ", StrategyScanTruthState::DataMissing);
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::Blocked);
    assert!(decision.reason_codes.contains(&"not_candidate_ranked".to_string()));
}

// 10. `watchlist_candidate` is possible for marginal evidence.
#[test]
fn marginal_profit_factor_becomes_watchlist_candidate() {
    let policy = StrategyScanReviewPolicy::default();
    let mut candidate = good_candidate("IBM");
    candidate.metrics.profit_factor = Some(policy.min_profit_factor - 0.1);
    let decision = evaluate_scan_review_decision(&candidate, &policy);
    assert_eq!(decision.review_state, StrategyScanReviewState::WatchlistCandidate);
    assert!(decision.reason_codes.contains(&"weak_profit_factor".to_string()));
}

// 11. Deterministic ordering of review decisions.
#[test]
fn review_decisions_are_deterministic_across_repeated_runs() {
    let policy = StrategyScanReviewPolicy::default();
    let candidates = vec![
        good_candidate("AAPL"),
        {
            let mut c = good_candidate("MSFT");
            c.metrics.total_return_pct = Some(-1.0);
            c
        },
        skipped_candidate("ZZZZ", StrategyScanTruthState::DataMissing),
    ];

    let run_1 = build_review_decisions(&candidates, &policy);
    let run_2 = build_review_decisions(&candidates, &policy);
    assert_eq!(run_1, run_2);
    // Order is preserved from the input (already-deterministic) candidate order.
    assert_eq!(run_1[0].symbol, "AAPL");
    assert_eq!(run_1[1].symbol, "MSFT");
    assert_eq!(run_1[2].symbol, "ZZZZ");
}

// 12. Serialization/deserialization stable.
#[test]
fn review_decision_serialization_round_trips() {
    let policy = StrategyScanReviewPolicy::default();
    let candidate = good_candidate("AAPL");
    let decision = evaluate_scan_review_decision(&candidate, &policy);

    let json = serde_json::to_string(&decision).expect("serialize");
    let round_tripped: mqk_backtest::StrategyScanReviewDecision =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decision, round_tripped);
    assert!(json.contains("\"paper_candidate\""));
}

// 13. Source-level proof: no broker/order/OMS/admission imports.
#[test]
fn source_imports_no_broker_order_oms_or_admission_type() {
    let source = include_str!("../src/strategy_scan_review.rs");
    let import_lines: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("use ") || line.starts_with("pub use "))
        .collect();

    let forbidden = [
        "mqk_broker",
        "mqk_execution",
        "mqk_oms",
        "outbox",
        "inbox",
        "admission",
        "strategy_router",
        "BrokerEvent",
        "OrderIntent",
    ];
    for line in &import_lines {
        for term in forbidden {
            assert!(
                !line.contains(term),
                "strategy_scan_review.rs import line must not reference '{term}': {line}"
            );
        }
    }

    // Sanity: the file does import something, so this test isn't vacuous.
    assert!(!import_lines.is_empty());
    assert!(import_lines.iter().any(|l| l.contains("serde")));
    assert!(import_lines.iter().any(|l| l.contains("strategy_scanner")));
}
