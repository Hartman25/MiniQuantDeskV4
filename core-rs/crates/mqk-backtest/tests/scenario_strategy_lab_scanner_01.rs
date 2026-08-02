//! STRATEGY-LAB-SCANNER-01B — pure scanner core scenario tests.
//!
//! Every test here operates on synthetic in-memory bars only. No file IO,
//! no network IO, no DB access, and no broker/provider environment
//! variable is read by any test in this file.

use mqk_backtest::{
    evaluate_scan_candidate, rank_scan_candidates, resolve_timeframe_secs, BacktestBar,
    StrategyScanPolicy, StrategyScanReasonCode, StrategyScanTruthState,
};
use mqk_strategy::engines::SwingMomentumStrategy;
use mqk_strategy::Strategy;

const SWING_TIMEFRAME_SECS: i64 = 86_400; // matches mqk_strategy::engines::swing_momentum

fn synthetic_daily_bars(symbol: &str, count: usize) -> Vec<BacktestBar> {
    let mut bars = Vec::with_capacity(count);
    let start_ts: i64 = 1_700_000_000; // arbitrary fixed epoch anchor (deterministic)
    for i in 0..count {
        let end_ts = start_ts + (i as i64) * SWING_TIMEFRAME_SECS;
        // Deterministic gentle uptrend with small oscillation so the
        // strategy has real signal to act on, not flat/degenerate bars.
        let base = 100_000_000i64 + (i as i64) * 100_000; // whole-dollar micros, drifting up
        let wiggle = if i % 2 == 0 { 50_000 } else { -50_000 };
        let open = base;
        let close = base + wiggle;
        let high = open.max(close) + 20_000;
        let low = open.min(close) - 20_000;
        bars.push(BacktestBar::new(
            symbol, end_ts, open, high, low, close, 1_000_000,
        ));
    }
    bars
}

fn swing_strategy(symbol: &str) -> Box<dyn Strategy> {
    Box::new(SwingMomentumStrategy::new(symbol))
}

// 1. Missing data candidate becomes `data_missing`.
#[test]
fn missing_bars_file_becomes_data_missing() {
    let policy = StrategyScanPolicy::default();
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        None, // no bars file found
        &policy,
    );
    assert_eq!(candidate.truth_state, StrategyScanTruthState::DataMissing);
    assert_eq!(
        candidate.reason_code,
        StrategyScanReasonCode::MissingBarsFile
    );
    assert_eq!(candidate.bars_available, 0);
    assert!(candidate.score.is_none());
    assert!(candidate.rank.is_none());
    assert!(!candidate.blockers.is_empty());
}

// 2. Too few bars becomes `insufficient_data`.
#[test]
fn too_few_bars_becomes_insufficient_data() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", policy.min_bars - 1);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );
    assert_eq!(
        candidate.truth_state,
        StrategyScanTruthState::InsufficientData
    );
    assert_eq!(candidate.reason_code, StrategyScanReasonCode::NotEnoughBars);
    assert_eq!(candidate.bars_available, policy.min_bars - 1);
}

// 3. Unsupported strategy becomes `unsupported_strategy`.
#[test]
fn unknown_strategy_id_becomes_unsupported_strategy() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "not_a_real_strategy",
        None, // scanner does not know this strategy_id
        None,
        Some(&bars),
        &policy,
    );
    assert_eq!(
        candidate.truth_state,
        StrategyScanTruthState::UnsupportedStrategy
    );
    assert_eq!(
        candidate.reason_code,
        StrategyScanReasonCode::StrategyNotSupportedByScanner
    );
}

// unsupported timeframe: requested timeframe does not match the
// strategy's registered timeframe_secs.
#[test]
fn mismatched_timeframe_becomes_unsupported_timeframe() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "5m",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS), // strategy requires 1D (86400s), not 5m
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );
    assert_eq!(
        candidate.truth_state,
        StrategyScanTruthState::UnsupportedTimeframe
    );
    assert_eq!(
        candidate.reason_code,
        StrategyScanReasonCode::TimeframeNotSupportedByScanner
    );
}

// unrecognized timeframe label entirely.
#[test]
fn unrecognized_timeframe_label_is_unsupported_timeframe() {
    assert_eq!(resolve_timeframe_secs("3D"), None);
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "3D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );
    assert_eq!(
        candidate.truth_state,
        StrategyScanTruthState::UnsupportedTimeframe
    );
}

// 4. Successful candidate becomes `candidate_ranked`.
#[test]
fn sufficient_bars_and_supported_strategy_becomes_candidate_ranked() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );
    assert_eq!(
        candidate.truth_state,
        StrategyScanTruthState::CandidateRanked
    );
    assert_eq!(candidate.reason_code, StrategyScanReasonCode::Ranked);
    assert!(candidate.score.is_some());
    assert_eq!(candidate.bars_available, 200);
    assert_eq!(candidate.metrics.bars_used, 200);
    assert!(candidate.blockers.is_empty());
}

// 5 + 6. Ranking is deterministic, and ties sort by symbol/timeframe/strategy_id.
#[test]
fn ranking_is_deterministic_and_ties_sort_by_symbol_timeframe_strategy() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);

    let build = || {
        vec![
            evaluate_scan_candidate(
                "MSFT",
                "1D",
                "swing_momentum",
                Some(SWING_TIMEFRAME_SECS),
                Some(swing_strategy("MSFT")),
                Some(&bars),
                &policy,
            ),
            evaluate_scan_candidate(
                "AAPL",
                "1D",
                "swing_momentum",
                Some(SWING_TIMEFRAME_SECS),
                Some(swing_strategy("AAPL")),
                Some(&bars),
                &policy,
            ),
        ]
    };

    let mut run_a = build();
    let mut run_b = build();
    rank_scan_candidates(&mut run_a);
    rank_scan_candidates(&mut run_b);

    let symbols_a: Vec<&str> = run_a.iter().map(|c| c.symbol.as_str()).collect();
    let symbols_b: Vec<&str> = run_b.iter().map(|c| c.symbol.as_str()).collect();
    assert_eq!(
        symbols_a, symbols_b,
        "identical inputs must rank identically"
    );

    // Both candidates share the exact same bars/strategy shape, so their
    // scores are equal (deterministic replay) -> tie-break falls through
    // to symbol ascending: AAPL before MSFT.
    assert_eq!(symbols_a, vec!["AAPL", "MSFT"]);
    assert_eq!(run_a[0].rank, Some(1));
    assert_eq!(run_a[1].rank, Some(2));
}

// 7. Negative score candidate can still rank but below positive score.
// 8. No-score candidates sort below scored candidates.
#[test]
fn ranked_before_skipped_and_higher_score_first() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);

    let ranked = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );
    let skipped_missing = evaluate_scan_candidate(
        "ZZZZ",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("ZZZZ")),
        None,
        &policy,
    );
    let skipped_unsupported = evaluate_scan_candidate(
        "YYYY",
        "1D",
        "not_a_real_strategy",
        None,
        None,
        Some(&bars),
        &policy,
    );

    let mut candidates = vec![skipped_unsupported, skipped_missing, ranked];
    rank_scan_candidates(&mut candidates);

    assert_eq!(
        candidates[0].truth_state,
        StrategyScanTruthState::CandidateRanked
    );
    assert_eq!(candidates[0].rank, Some(1));
    assert!(candidates[0].score.is_some());

    // Both skipped candidates keep rank=None regardless of order.
    for c in &candidates[1..] {
        assert_ne!(c.truth_state, StrategyScanTruthState::CandidateRanked);
        assert!(c.rank.is_none());
        assert!(c.score.is_none());
    }
}

// Explicit synthetic score ordering (7/8 spirit): manually constructed
// candidates prove score-desc ordering independent of the engine.
#[test]
fn explicit_score_ordering_positive_before_negative_before_none() {
    use mqk_backtest::{StrategyScanCandidate, StrategyScanMetrics};

    fn candidate(symbol: &str, score: Option<f64>) -> StrategyScanCandidate {
        StrategyScanCandidate {
            symbol: symbol.to_string(),
            timeframe: "1D".to_string(),
            strategy_id: "swing_momentum".to_string(),
            bars_available: 200,
            truth_state: StrategyScanTruthState::CandidateRanked,
            reason_code: StrategyScanReasonCode::Ranked,
            score,
            rank: None,
            metrics: StrategyScanMetrics::default(),
            warnings: Vec::new(),
            blockers: Vec::new(),
        }
    }

    let mut candidates = vec![
        candidate("NEG", Some(-5.0)),
        candidate("POS", Some(10.0)),
        candidate("NONE", None),
    ];
    rank_scan_candidates(&mut candidates);

    let order: Vec<&str> = candidates.iter().map(|c| c.symbol.as_str()).collect();
    assert_eq!(order, vec!["POS", "NEG", "NONE"]);
    assert_eq!(candidates[0].rank, Some(1));
    assert_eq!(candidates[1].rank, Some(2));
    // A `candidate_ranked` row with score=None still ranks (rank is
    // assigned by truth_state, not by score presence) -- score is only
    // the sort key, not the ranked/skipped gate.
    assert_eq!(candidates[2].rank, Some(3));
}

// 9. Artifact schema serializes/deserializes.
#[test]
fn candidate_schema_round_trips_through_json() {
    let policy = StrategyScanPolicy::default();
    let bars = synthetic_daily_bars("AAPL", 200);
    let candidate = evaluate_scan_candidate(
        "AAPL",
        "1D",
        "swing_momentum",
        Some(SWING_TIMEFRAME_SECS),
        Some(swing_strategy("AAPL")),
        Some(&bars),
        &policy,
    );

    let json = serde_json::to_string(&candidate).expect("serialize candidate");
    assert!(json.contains("\"truth_state\":\"candidate_ranked\""));
    assert!(json.contains("\"reason_code\":\"ranked\""));

    let round_tripped: mqk_backtest::StrategyScanCandidate =
        serde_json::from_str(&json).expect("deserialize candidate");
    assert_eq!(round_tripped, candidate);
}

// 10. No order/execution/broker modules imported by the scanner core
// module's own source (source-level check -- the scanner reuses the
// existing in-memory BacktestEngine, but must not itself reach for a
// live broker, provider, or OMS DB-write type).
#[test]
fn scanner_source_does_not_import_broker_or_oms_write_types() {
    let source = include_str!("../src/strategy_scanner.rs");
    for forbidden in [
        "mqk_broker",
        "mqk_db::insert_outbox",
        "mqk_db::insert_inbox",
        "oms_outbox",
        "oms_inbox",
        "reqwest",
        "tokio::net",
        "std::net::",
    ] {
        assert!(
            !source.contains(forbidden),
            "strategy_scanner.rs must not reference '{forbidden}'"
        );
    }
}
