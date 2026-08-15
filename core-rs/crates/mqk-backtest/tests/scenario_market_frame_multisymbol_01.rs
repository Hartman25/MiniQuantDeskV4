//! BKT-MULTISYMBOL-MARKET-FRAME-01: multi-symbol market-frame proof harness.
//!
//! Proves the deterministic, symbol-isolated, anti-lookahead multi-symbol
//! market-frame abstraction in `mqk_backtest::market_frame`, including the
//! real streaming evaluation seam (`evaluate_market_frames`), the fail-closed
//! duplicate-bar contract, and the bounded (not frame-count-scaling) history
//! footprint of that seam. This module is standalone and additive — none of
//! these tests touch `BacktestEngine`'s existing single-symbol run loop,
//! which is exercised by its own separate scenario tests (e.g.
//! `scenario_lookahead_bias_proof.rs`, `scenario_backtest_ohlcv_context_01.rs`)
//! and is unchanged by this patch.

use mqk_backtest::{
    build_market_frames, evaluate_market_frames, strategy_context_for_symbol, BacktestBar,
    BacktestConfig, BacktestEngine, MarketFrameError, MarketFrameEvaluator, MarketFrameView,
};
use mqk_execution::StrategyOutput as ExecOutput;
use mqk_strategy::{RecentBarsWindow, Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a bar with clearly distinct O/H/L/C/V so fidelity checks are
/// unambiguous. `close` anchors the price band; symbols use disjoint bands
/// (AAPL ~100s, AMD ~200s, NVDA ~300s, SPY ~400s, SMH ~500s) so any
/// cross-symbol contamination would land in the wrong band and be caught.
fn bar(symbol: &str, end_ts: i64, close: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        close - 3,       // open
        close + 5,       // high
        close - 5,       // low
        close,           // close
        1_000 + close,   // volume (also distinct, for fidelity check)
    )
}

// ---------------------------------------------------------------------------
// Test 1 — Same-timestamp frame
// ---------------------------------------------------------------------------

/// AAPL, AMD, SPY at the same end_ts must form exactly one coherent frame
/// exposing all three symbols' current bars.
#[test]
fn test1_same_timestamp_bars_form_one_coherent_frame() {
    let bars = vec![
        bar("AAPL", 600, 100),
        bar("AMD", 600, 200),
        bar("SPY", 600, 400),
    ];
    let frames = build_market_frames(&bars, 20).unwrap();

    assert_eq!(frames.len(), 1, "one distinct end_ts must produce exactly one frame");
    let f = &frames[0];
    assert_eq!(f.end_ts(), 600);
    assert!(f.current("AAPL").is_some());
    assert!(f.current("AMD").is_some());
    assert!(f.current("SPY").is_some());
    let symbols: Vec<&str> = f.symbols().collect();
    assert_eq!(symbols, vec!["AAPL", "AMD", "SPY"], "symbols() must be deterministic (alphabetical)");
}

// ---------------------------------------------------------------------------
// Test 2 — Symbol history isolation
// ---------------------------------------------------------------------------

/// After several timestamps, AAPL's history must contain only AAPL bars —
/// proven by checking every close price in AAPL's history falls in AAPL's
/// disjoint price band, never AMD's or SPY's band.
#[test]
fn test2_symbol_history_is_isolated() {
    let mut bars = Vec::new();
    for i in 0..5 {
        let ts = 600 + i * 60;
        bars.push(bar("AAPL", ts, 100 + i));
        bars.push(bar("AMD", ts, 200 + i));
        bars.push(bar("SPY", ts, 400 + i));
    }
    let frames = build_market_frames(&bars, 20).unwrap();
    let last = frames.last().unwrap();

    let aapl_hist = last.history("AAPL").expect("AAPL history present");
    assert_eq!(aapl_hist.len(), 5);
    for b in &aapl_hist.bars {
        assert!(
            (100..200).contains(&b.close_micros),
            "AAPL history contains an out-of-band close ({}) — cross-symbol contamination",
            b.close_micros
        );
    }

    let amd_hist = last.history("AMD").expect("AMD history present");
    for b in &amd_hist.bars {
        assert!(
            (200..300).contains(&b.close_micros),
            "AMD history contains an out-of-band close ({}) — cross-symbol contamination",
            b.close_micros
        );
    }

    let spy_hist = last.history("SPY").expect("SPY history present");
    for b in &spy_hist.bars {
        assert!(
            (400..500).contains(&b.close_micros),
            "SPY history contains an out-of-band close ({}) — cross-symbol contamination",
            b.close_micros
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3 — Input-order invariance (build_market_frames)
// ---------------------------------------------------------------------------

/// Feeding the same bars in two different row orders (including reordering
/// within a shared timestamp AND across timestamps) must produce identical
/// frame sequences.
#[test]
fn test3_input_row_order_does_not_affect_frames() {
    let ordered = vec![
        bar("AAPL", 600, 100),
        bar("AMD", 600, 200),
        bar("NVDA", 600, 300),
        bar("SPY", 600, 400),
        bar("AAPL", 660, 101),
        bar("AMD", 660, 201),
        bar("NVDA", 660, 301),
        bar("SPY", 660, 401),
    ];

    // Fully reversed, including across timestamps.
    let mut shuffled = ordered.clone();
    shuffled.reverse();

    // A second, differently-scrambled permutation for extra confidence.
    let scrambled = vec![
        ordered[5].clone(), // AMD @ 660
        ordered[0].clone(), // AAPL @ 600
        ordered[7].clone(), // SPY @ 660
        ordered[2].clone(), // NVDA @ 600
        ordered[4].clone(), // AAPL @ 660
        ordered[1].clone(), // AMD @ 600
        ordered[6].clone(), // NVDA @ 660
        ordered[3].clone(), // SPY @ 600
    ];

    let frames_ordered = build_market_frames(&ordered, 20).unwrap();
    let frames_shuffled = build_market_frames(&shuffled, 20).unwrap();
    let frames_scrambled = build_market_frames(&scrambled, 20).unwrap();

    assert_eq!(
        frames_ordered, frames_shuffled,
        "reversed row order must produce an identical frame sequence"
    );
    assert_eq!(
        frames_ordered, frames_scrambled,
        "scrambled row order must produce an identical frame sequence"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — No future-symbol leakage
// ---------------------------------------------------------------------------

/// While evaluating frame T, no bar from T + timeframe may be observable —
/// for any symbol, via either `current()` or `history()`.
#[test]
fn test4_no_future_bar_leakage_across_frames() {
    let bars = vec![
        bar("AAPL", 600, 100),
        bar("SPY", 600, 400),
        bar("AAPL", 660, 999), // distinct future value, must never appear at frame 600
        bar("SPY", 660, 999),
    ];
    let frames = build_market_frames(&bars, 20).unwrap();
    assert_eq!(frames.len(), 2);

    let f_now = &frames[0];
    assert_eq!(f_now.end_ts(), 600);

    // current() at T must never see the T+60 bar.
    assert_eq!(f_now.current("AAPL").unwrap().close_micros, 100);
    assert_eq!(f_now.current("SPY").unwrap().close_micros, 400);

    // history() at T must never contain a bar with end_ts > T.
    for symbol in ["AAPL", "SPY"] {
        let hist = f_now.history(symbol).unwrap();
        for b in &hist.bars {
            assert!(
                b.end_ts <= f_now.end_ts(),
                "frame at {} exposed a future bar at {} for {}",
                f_now.end_ts(),
                b.end_ts,
                symbol
            );
        }
    }

    // General property: check across the whole sequence, not just frame 0.
    for f in &frames {
        for symbol in f.symbols() {
            if let Some(hist) = f.history(symbol) {
                for b in &hist.bars {
                    assert!(b.end_ts <= f.end_ts(), "future bar leaked into history");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 5 — Full OHLCV fidelity
// ---------------------------------------------------------------------------

/// Every symbol must retain its own exact open/high/low/close/volume inside
/// the frame — no truncation, no cross-symbol swap.
#[test]
fn test5_full_ohlcv_fidelity_per_symbol() {
    let aapl = BacktestBar::new("AAPL", 600, 100, 108, 95, 103, 5_000);
    let spy = BacktestBar::new("SPY", 600, 400, 420, 390, 410, 9_000);
    let bars = vec![aapl.clone(), spy.clone()];

    let frames = build_market_frames(&bars, 20).unwrap();
    let f = &frames[0];

    let got_aapl = f.current("AAPL").unwrap();
    assert_eq!(got_aapl.open_micros, aapl.open_micros);
    assert_eq!(got_aapl.high_micros, aapl.high_micros);
    assert_eq!(got_aapl.low_micros, aapl.low_micros);
    assert_eq!(got_aapl.close_micros, aapl.close_micros);
    assert_eq!(got_aapl.volume, aapl.volume);

    let got_spy = f.current("SPY").unwrap();
    assert_eq!(got_spy.open_micros, spy.open_micros);
    assert_eq!(got_spy.high_micros, spy.high_micros);
    assert_eq!(got_spy.low_micros, spy.low_micros);
    assert_eq!(got_spy.close_micros, spy.close_micros);
    assert_eq!(got_spy.volume, spy.volume);
}

// ---------------------------------------------------------------------------
// Test 6 — Missing symbol explicit
// ---------------------------------------------------------------------------

/// A universe of AAPL/AMD/NVDA/SPY/SMH where NVDA is absent at one timestamp
/// must expose that absence as an explicit `None`, never a synthetic /
/// forward-filled bar.
#[test]
fn test6_missing_symbol_is_explicit_none() {
    let bars = vec![
        bar("AAPL", 600, 100),
        bar("AMD", 600, 200),
        bar("NVDA", 600, 300),
        bar("SPY", 600, 400),
        bar("SMH", 600, 500),
        // NVDA missing at 660 — universe only has 4 of 5 expected symbols.
        bar("AAPL", 660, 101),
        bar("AMD", 660, 201),
        bar("SPY", 660, 401),
        bar("SMH", 660, 501),
    ];
    let frames = build_market_frames(&bars, 20).unwrap();
    assert_eq!(frames.len(), 2);

    let f0 = &frames[0];
    assert!(f0.current("NVDA").is_some(), "NVDA present at 600");

    let f1 = &frames[1];
    assert!(f1.current("NVDA").is_none(), "NVDA absent at 660 must be explicit None");
    assert!(!f1.is_present("NVDA"));
    // The other four symbols are unaffected by NVDA's absence.
    for s in ["AAPL", "AMD", "SPY", "SMH"] {
        assert!(f1.current(s).is_some(), "{s} must still be present at 660");
    }

    // NVDA's history is not extended with a synthetic bar at 660 — it still
    // has exactly one observed bar (from 600).
    let nvda_hist = f1.history("NVDA").expect("NVDA history persists from 600");
    assert_eq!(nvda_hist.len(), 1);
    assert_eq!(nvda_hist.last().unwrap().end_ts, 600);
    assert_eq!(nvda_hist.last().unwrap().close_micros, 300);
}

// ---------------------------------------------------------------------------
// Test 7 — Bounded history
// ---------------------------------------------------------------------------

/// Per-symbol history must honor the configured `bar_history_len`, keeping
/// only the most recent N bars (tail truncation), same as `RecentBarsWindow`.
#[test]
fn test7_history_is_bounded_per_symbol() {
    let mut bars = Vec::new();
    for i in 0..10 {
        bars.push(bar("AAPL", 600 + i * 60, 100 + i));
    }
    let frames = build_market_frames(&bars, 3).unwrap();
    let last = frames.last().unwrap();

    let hist = last.history("AAPL").unwrap();
    assert_eq!(hist.len(), 3, "history must be truncated to bar_history_len=3");
    // Must contain exactly the 3 most recent bars (end_ts 1020, 1080, 1140,
    // i.e. i=7,8,9 of the 10 bars at 600 + i*60).
    let end_tss: Vec<i64> = hist.bars.iter().map(|b| b.end_ts).collect();
    assert_eq!(end_tss, vec![1020, 1080, 1140]);
}

// ---------------------------------------------------------------------------
// Test 8 — Existing single-symbol compatibility
// ---------------------------------------------------------------------------

/// The compatibility adapter (`strategy_context_for_symbol`) must reproduce,
/// bar-for-bar, the exact `StrategyContext.recent` window that the existing
/// `BacktestEngine::run` single-symbol path builds — proving a single-symbol
/// strategy keeps working unmodified whether driven by the classic engine
/// loop or by the new market-frame layer.
#[test]
fn test8_compat_adapter_matches_existing_engine_context() {
    let bars = vec![
        bar("SPY", 600, 400),
        bar("SPY", 660, 401),
        bar("SPY", 720, 402),
    ];

    // Path A: existing, unmodified BacktestEngine single-symbol run.
    // Strategy is owned by the engine, so it records into a shared log
    // rather than exposing its own state after the run.
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());

    use std::sync::{Arc, Mutex};
    struct SharedRecorder {
        log: Arc<Mutex<Vec<RecentBarsWindow>>>,
    }
    impl Strategy for SharedRecorder {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new("shared_recorder", 60)
        }
        fn on_bar(&mut self, ctx: &StrategyContext) -> ExecOutput {
            self.log.lock().unwrap().push(ctx.recent.clone());
            ExecOutput::new(vec![])
        }
    }

    let engine_log = Arc::new(Mutex::new(Vec::new()));
    engine
        .add_strategy(Box::new(SharedRecorder {
            log: Arc::clone(&engine_log),
        }))
        .unwrap();
    engine.run(&bars).unwrap();
    let engine_windows = engine_log.lock().unwrap().clone();

    // Path B: market_frame layer + compatibility adapter, same bars/config.
    let frames = build_market_frames(&bars, BacktestConfig::test_defaults().bar_history_len).unwrap();
    let mut adapter_windows = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let ctx = strategy_context_for_symbol(f, "SPY", 60, (i + 1) as u64)
            .expect("SPY history present");
        adapter_windows.push(ctx.recent.clone());
    }

    assert_eq!(
        engine_windows, adapter_windows,
        "compat adapter must reproduce the engine's native single-symbol context bar-for-bar"
    );
}

// ---------------------------------------------------------------------------
// Test 9 — Timestamp determinism
// ---------------------------------------------------------------------------

/// Frames must be evaluated in strictly ascending end_ts order, regardless of
/// input row order.
#[test]
fn test9_frames_are_strictly_ascending_by_end_ts_regardless_of_input_order() {
    let mut bars = vec![
        bar("AAPL", 780, 103),
        bar("AAPL", 600, 100),
        bar("SPY", 660, 401),
        bar("AAPL", 660, 101),
        bar("SPY", 600, 400),
        bar("SPY", 780, 403),
    ];
    // Scramble further to avoid any accidental partial ordering.
    bars.reverse();

    let frames = build_market_frames(&bars, 20).unwrap();
    assert_eq!(frames.len(), 3);

    let end_tss: Vec<i64> = frames.iter().map(|f| f.end_ts()).collect();
    let mut sorted = end_tss.clone();
    sorted.sort();
    assert_eq!(end_tss, sorted, "frames must be in ascending end_ts order");
    assert_eq!(end_tss, vec![600, 660, 780]);
}

// ---------------------------------------------------------------------------
// Optional high-value test — cross-sectional probe (no trading, no alpha)
// ---------------------------------------------------------------------------

/// Demonstrates that a same-timestamp frame supports a minimal, deterministic
/// cross-sectional observation (AAPL close / SPY close) without placing any
/// order and without implementing an actual alpha strategy.
#[test]
fn optional_cross_sectional_probe_computes_relative_close() {
    let bars = vec![bar("AAPL", 600, 100), bar("SPY", 600, 400)];
    let frames = build_market_frames(&bars, 20).unwrap();
    let f = &frames[0];

    let aapl_close = f.current("AAPL").unwrap().close_micros;
    let spy_close = f.current("SPY").unwrap().close_micros;
    let ratio = aapl_close as f64 / spy_close as f64;

    assert!((ratio - 0.25).abs() < 1e-9, "expected AAPL/SPY == 100/400 == 0.25, got {ratio}");
}

// ---------------------------------------------------------------------------
// BLOCKER 1 — real evaluation seam (evaluate_market_frames)
// ---------------------------------------------------------------------------

/// Records everything a `MarketFrameEvaluator` observed at each `on_frame`
/// call, for assertion after the run completes.
#[derive(Default)]
struct RecordingEvaluator {
    /// One entry per `on_frame` call, in call order.
    calls: Vec<FrameObservation>,
}

struct FrameObservation {
    end_ts: i64,
    /// (symbol, close_micros) for every symbol present this frame, in the
    /// frame's own deterministic (alphabetical) order.
    present: Vec<(String, i64)>,
    /// symbol -> history length, for every symbol with any history at all.
    history_lens: Vec<(String, usize)>,
}

impl MarketFrameEvaluator for RecordingEvaluator {
    fn on_frame(&mut self, frame: &MarketFrameView<'_>) {
        let present: Vec<(String, i64)> = frame
            .symbols()
            .map(|s| (s.to_string(), frame.current(s).unwrap().close_micros))
            .collect();
        let history_lens: Vec<(String, usize)> = present
            .iter()
            .filter_map(|(s, _)| frame.history(s).map(|h| (s.clone(), h.len())))
            .collect();
        self.calls.push(FrameObservation {
            end_ts: frame.end_ts(),
            present,
            history_lens,
        });
    }
}

/// End-to-end proof: the real evaluation seam runs a chronological loop over
/// multiple timestamps and an evaluator observes AAPL + AMD + SPY
/// contemporaneously at each one — proving cross-sectional comparison is
/// actually possible through this seam, not just through a probe test on
/// `MarketFrame` directly.
#[test]
fn test10_evaluation_seam_sees_aapl_amd_spy_contemporaneously_across_timestamps() {
    let mut bars = Vec::new();
    for i in 0..4 {
        let ts = 600 + i * 60;
        bars.push(bar("AAPL", ts, 100 + i));
        bars.push(bar("AMD", ts, 200 + i));
        bars.push(bar("SPY", ts, 400 + i));
    }

    let mut evaluator = RecordingEvaluator::default();
    evaluate_market_frames(&bars, 20, &mut evaluator).expect("valid input must not fail closed");

    assert_eq!(evaluator.calls.len(), 4, "one on_frame call per distinct timestamp");
    let end_tss: Vec<i64> = evaluator.calls.iter().map(|c| c.end_ts).collect();
    assert_eq!(end_tss, vec![600, 660, 720, 780], "must evaluate in ascending timestamp order");

    for call in &evaluator.calls {
        let symbols: Vec<&str> = call.present.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            symbols,
            vec!["AAPL", "AMD", "SPY"],
            "evaluator must see all three symbols contemporaneously at end_ts={}",
            call.end_ts
        );
    }

    // Cross-sectional comparison actually possible: last frame's AAPL close
    // vs SPY close, read from the same on_frame call.
    let last = evaluator.calls.last().unwrap();
    let aapl_close = last.present.iter().find(|(s, _)| s == "AAPL").unwrap().1;
    let spy_close = last.present.iter().find(|(s, _)| s == "SPY").unwrap().1;
    assert_eq!(aapl_close, 103);
    assert_eq!(spy_close, 403);
}

/// Evaluation must happen exactly once per distinct timestamp, not once per
/// (symbol, timestamp) pair.
#[test]
fn test11_evaluation_happens_once_per_timestamp_not_per_symbol() {
    let mut bars = Vec::new();
    for i in 0..5 {
        let ts = 600 + i * 60;
        for symbol in ["AAPL", "AMD", "NVDA", "SPY", "SMH"] {
            bars.push(bar(symbol, ts, 100 + i));
        }
    }
    let mut evaluator = RecordingEvaluator::default();
    evaluate_market_frames(&bars, 20, &mut evaluator).unwrap();

    // 5 timestamps x 5 symbols = 25 bars, but only 5 on_frame calls.
    assert_eq!(bars.len(), 25);
    assert_eq!(evaluator.calls.len(), 5, "on_frame must fire once per timestamp, not once per bar");
}

/// The seam's missing-symbol contract must be explicit (never forward-filled)
/// via `MarketFrameView`, same as `MarketFrame`.
#[test]
fn test12_view_missing_symbol_is_explicit() {
    struct MissingChecker {
        observed_missing_at_second_frame: Option<Vec<String>>,
    }
    impl MarketFrameEvaluator for MissingChecker {
        fn on_frame(&mut self, frame: &MarketFrameView<'_>) {
            if frame.end_ts() == 660 {
                let universe = ["AAPL", "NVDA"];
                let missing: Vec<String> = frame
                    .missing(&universe)
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                self.observed_missing_at_second_frame = Some(missing);
            }
        }
    }

    let bars = vec![
        bar("AAPL", 600, 100),
        bar("NVDA", 600, 300),
        // NVDA absent at 660.
        bar("AAPL", 660, 101),
    ];
    let mut checker = MissingChecker {
        observed_missing_at_second_frame: None,
    };
    evaluate_market_frames(&bars, 20, &mut checker).unwrap();

    assert_eq!(
        checker.observed_missing_at_second_frame,
        Some(vec!["NVDA".to_string()]),
        "evaluator must be able to detect NVDA missing at end_ts=660 via the view"
    );
}

/// No future bar may be visible through the streaming seam either — same
/// anti-lookahead contract as `build_market_frames`, proven through
/// `evaluate_market_frames` directly.
#[test]
fn test13_evaluation_seam_has_no_future_bar_leakage() {
    struct LeakChecker {
        violations: Vec<String>,
    }
    impl MarketFrameEvaluator for LeakChecker {
        fn on_frame(&mut self, frame: &MarketFrameView<'_>) {
            for symbol in frame.symbols() {
                if let Some(hist) = frame.history(symbol) {
                    for b in &hist.bars {
                        if b.end_ts > frame.end_ts() {
                            self.violations.push(format!(
                                "{symbol} history at frame {} contained future bar {}",
                                frame.end_ts(),
                                b.end_ts
                            ));
                        }
                    }
                }
                if let Some(cur) = frame.current(symbol) {
                    if cur.end_ts != frame.end_ts() {
                        self.violations.push(format!(
                            "{symbol} current() at frame {} returned bar from {}",
                            frame.end_ts(),
                            cur.end_ts
                        ));
                    }
                }
            }
        }
    }

    let bars = vec![
        bar("AAPL", 600, 100),
        bar("SPY", 600, 400),
        bar("AAPL", 660, 999),
        bar("SPY", 660, 999),
    ];
    let mut checker = LeakChecker { violations: Vec::new() };
    evaluate_market_frames(&bars, 20, &mut checker).unwrap();

    assert!(
        checker.violations.is_empty(),
        "future-bar leakage detected: {:?}",
        checker.violations
    );
}

// ---------------------------------------------------------------------------
// BLOCKER 2 — duplicate (symbol, end_ts) fails closed
// ---------------------------------------------------------------------------

/// Dataset A: AAPL@T close=100 then close=101. Must fail closed with a typed
/// error identifying the offending (symbol, end_ts), not silently pick a
/// winner.
#[test]
fn test14_duplicate_bar_dataset_a_fails_closed() {
    let bars = vec![bar("AAPL", 600, 100), bar("AAPL", 600, 101)];

    let build_err = build_market_frames(&bars, 20).unwrap_err();
    assert_eq!(
        build_err,
        MarketFrameError::DuplicateBar {
            symbol: "AAPL".to_string(),
            end_ts: 600
        }
    );

    let mut evaluator = RecordingEvaluator::default();
    let eval_err = evaluate_market_frames(&bars, 20, &mut evaluator).unwrap_err();
    assert_eq!(eval_err, build_err, "both entry points must report the same typed error");
    assert!(evaluator.calls.is_empty(), "zero evaluations on a refused input");
}

/// Dataset B: same pair, rows reversed (close=101 then close=100). Must fail
/// with the *same* typed error/reason as dataset A — order must not change
/// which failure is reported.
#[test]
fn test15_duplicate_bar_dataset_b_reversed_rows_fails_closed_identically() {
    let dataset_a = vec![bar("AAPL", 600, 100), bar("AAPL", 600, 101)];
    let dataset_b = vec![bar("AAPL", 600, 101), bar("AAPL", 600, 100)];

    let err_a = build_market_frames(&dataset_a, 20).unwrap_err();
    let err_b = build_market_frames(&dataset_b, 20).unwrap_err();
    assert_eq!(err_a, err_b, "reversing the duplicate rows must yield the identical failure");

    let mut eval_a = RecordingEvaluator::default();
    let mut eval_b = RecordingEvaluator::default();
    let eval_err_a = evaluate_market_frames(&dataset_a, 20, &mut eval_a).unwrap_err();
    let eval_err_b = evaluate_market_frames(&dataset_b, 20, &mut eval_b).unwrap_err();
    assert_eq!(eval_err_a, eval_err_b);
    assert!(eval_a.calls.is_empty());
    assert!(eval_b.calls.is_empty());
}

/// An exact byte/value-identical duplicate row must still fail closed —
/// duplicate source rows are ambiguous provenance regardless of whether the
/// values happen to agree.
#[test]
fn test16_exact_duplicate_bar_still_fails_closed() {
    let bars = vec![bar("AAPL", 600, 100), bar("AAPL", 600, 100)];
    let err = build_market_frames(&bars, 20).unwrap_err();
    assert_eq!(
        err,
        MarketFrameError::DuplicateBar {
            symbol: "AAPL".to_string(),
            end_ts: 600
        },
        "byte-identical duplicate rows must still be refused, not silently deduplicated"
    );
}

/// A duplicate on one symbol must not be masked by, or leak past, valid
/// distinct-symbol bars at the same timestamp — the whole input is refused.
#[test]
fn test17_duplicate_among_otherwise_valid_multisymbol_input_fails_closed() {
    let bars = vec![
        bar("AAPL", 600, 100),
        bar("AMD", 600, 200),
        bar("SPY", 600, 400),
        bar("AMD", 600, 201), // duplicate AMD @ 600
    ];
    let err = build_market_frames(&bars, 20).unwrap_err();
    assert_eq!(
        err,
        MarketFrameError::DuplicateBar {
            symbol: "AMD".to_string(),
            end_ts: 600
        }
    );
}

// ---------------------------------------------------------------------------
// Order invariance of evaluator observations (evaluate_market_frames)
// ---------------------------------------------------------------------------

/// A valid (non-duplicate) dataset fed through the streaming seam in
/// arbitrary row orders must produce identical evaluator observations —
/// same per-timestamp symbol sets, closes, and history lengths.
#[test]
fn test18_valid_dataset_arbitrary_row_orders_yield_identical_evaluator_observations() {
    let ordered = vec![
        bar("AAPL", 600, 100),
        bar("AMD", 600, 200),
        bar("NVDA", 600, 300),
        bar("SPY", 600, 400),
        bar("AAPL", 660, 101),
        bar("AMD", 660, 201),
        bar("NVDA", 660, 301),
        bar("SPY", 660, 401),
    ];
    let mut reversed = ordered.clone();
    reversed.reverse();
    let scrambled = vec![
        ordered[5].clone(),
        ordered[0].clone(),
        ordered[7].clone(),
        ordered[2].clone(),
        ordered[4].clone(),
        ordered[1].clone(),
        ordered[6].clone(),
        ordered[3].clone(),
    ];

    type FrameObservationTuple = (i64, Vec<(String, i64)>, Vec<(String, usize)>);

    fn observe(bars: &[BacktestBar]) -> Vec<FrameObservationTuple> {
        let mut evaluator = RecordingEvaluator::default();
        evaluate_market_frames(bars, 20, &mut evaluator).unwrap();
        evaluator
            .calls
            .into_iter()
            .map(|c| (c.end_ts, c.present, c.history_lens))
            .collect()
    }

    let obs_ordered = observe(&ordered);
    let obs_reversed = observe(&reversed);
    let obs_scrambled = observe(&scrambled);

    assert_eq!(obs_ordered, obs_reversed);
    assert_eq!(obs_ordered, obs_scrambled);
}

// ---------------------------------------------------------------------------
// BLOCKER 3 — bounded history, not frame-count-scaling
// ---------------------------------------------------------------------------

/// Runs a large number of timestamps with a small `bar_history_len` and
/// proves the evaluator's observed history length for each symbol never
/// exceeds the configured bound at any point in the run — i.e. the seam's
/// retained per-symbol state does not grow with the number of frames
/// processed. This is the runtime-observable proxy for the structural
/// guarantee documented on `evaluate_market_frames`: it carries exactly one
/// `RecentBarsWindow` per symbol across the whole run (O(symbols x
/// bar_history_len)), never a per-frame snapshot Vec.
#[test]
fn test19_history_stays_bounded_across_a_large_number_of_frames() {
    const NUM_TIMESTAMPS: i64 = 5_000;
    const HISTORY_LEN: usize = 3;
    let symbols = ["AAPL", "AMD", "NVDA", "SPY", "SMH"];

    let mut bars = Vec::new();
    for i in 0..NUM_TIMESTAMPS {
        let ts = 600 + i * 60;
        for (j, symbol) in symbols.iter().enumerate() {
            bars.push(bar(symbol, ts, 100 + (j as i64) * 100 + i));
        }
    }

    struct BoundChecker {
        max_len: usize,
        frame_count: u64,
        max_observed_history_len: usize,
    }
    impl MarketFrameEvaluator for BoundChecker {
        fn on_frame(&mut self, frame: &MarketFrameView<'_>) {
            self.frame_count += 1;
            for symbol in frame.symbols() {
                if let Some(hist) = frame.history(symbol) {
                    assert!(
                        hist.len() <= self.max_len,
                        "history for {symbol} exceeded bound {} at frame {} (len={})",
                        self.max_len,
                        frame.end_ts(),
                        hist.len()
                    );
                    self.max_observed_history_len = self.max_observed_history_len.max(hist.len());
                }
            }
        }
    }

    let mut checker = BoundChecker {
        max_len: HISTORY_LEN,
        frame_count: 0,
        max_observed_history_len: 0,
    };
    evaluate_market_frames(&bars, HISTORY_LEN, &mut checker).unwrap();

    assert_eq!(checker.frame_count, NUM_TIMESTAMPS as u64);
    assert_eq!(
        checker.max_observed_history_len, HISTORY_LEN,
        "history should reach but never exceed the configured bound"
    );
}
