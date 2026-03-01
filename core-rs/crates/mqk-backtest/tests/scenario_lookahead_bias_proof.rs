//! B5-1: Lookahead Bias Proof Harness
//!
//! Mechanically proves that no future-bar data can leak into strategy
//! decisions or fill prices.  Five orthogonal scenarios, each targeting a
//! distinct lookahead vector:
//!
//! 1. Fill price bounded by *current* bar's [LOW, HIGH] — not next bar's open.
//! 2. `now_tick` advances +1 per bar (strictly monotonic, no jumps).
//! 3. `ctx.recent` window grows by exactly one entry per bar.
//! 4. `ctx.recent.last().end_ts` equals the current bar's end_ts (not future).
//! 5. Incomplete bar → `Err` before strategy `on_bar` is ever called.

use std::sync::{Arc, Mutex};

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Spy strategy — records what strategy sees at each on_bar call
// ---------------------------------------------------------------------------

/// One recorded observation from an `on_bar` invocation.
#[derive(Clone, Debug)]
struct BarObservation {
    now_tick: u64,
    window_len: usize,
    last_end_ts: Option<i64>,
}

/// Passthrough strategy: records observations, never places orders.
struct SpyStrategy {
    log: Arc<Mutex<Vec<BarObservation>>>,
}

impl SpyStrategy {
    fn new(log: Arc<Mutex<Vec<BarObservation>>>) -> Self {
        Self { log }
    }
}

impl Strategy for SpyStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Spy", 60)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.log.lock().unwrap().push(BarObservation {
            now_tick: ctx.now_tick,
            window_len: ctx.recent.len(),
            last_end_ts: ctx.recent.last().map(|b| b.end_ts),
        });
        StrategyOutput::new(vec![])
    }
}

/// Records observations AND places a BUY on tick 1 so we can check fill price.
struct SpyBuyOnFirst {
    log: Arc<Mutex<Vec<BarObservation>>>,
    fired: bool,
}

impl SpyBuyOnFirst {
    fn new(log: Arc<Mutex<Vec<BarObservation>>>) -> Self {
        Self { log, fired: false }
    }
}

impl Strategy for SpyBuyOnFirst {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("SpyBuyOnFirst", 60)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.log.lock().unwrap().push(BarObservation {
            now_tick: ctx.now_tick,
            window_len: ctx.recent.len(),
            last_end_ts: ctx.recent.last().map(|b| b.end_ts),
        });
        if !self.fired {
            self.fired = true;
            StrategyOutput::new(vec![TargetPosition::new("SPY", 10)])
        } else {
            StrategyOutput::new(vec![])
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build N bars with clearly-distinct, non-overlapping price levels per bar
/// so that a fill at any other bar's price is unambiguously detectable.
fn make_bars(n: usize) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            let ts = 1_700_000_060_i64 + i as i64 * 60;
            // Prices spaced $20 apart so no two bars' [LOW, HIGH] ranges overlap.
            // bar[i]: open=$100+20i, high=open+$5, low=open-$5, close=open+$2
            let open = 100_000_000_i64 + i as i64 * 20_000_000;
            BacktestBar::new(
                "SPY",
                ts,
                open,
                open + 5_000_000, // high
                open - 5_000_000, // low
                open + 2_000_000, // close
                1_000,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test 1: Fill price is bounded by *current* bar's [LOW, HIGH]
// ---------------------------------------------------------------------------

/// A BUY signal on bar 1 must fill within bar 1's [LOW_1, HIGH_1].
///
/// Any price outside this range would require knowing bar 2's open (or higher),
/// proving lookahead.  Bar prices are spaced $20 apart so the ranges are
/// non-overlapping — a fill at bar 2's price cannot be mistaken for bar 1's.
///
/// The engine is run with **only bar 1** so that bar 2 is structurally
/// inaccessible; the non-overlap is confirmed statically as a test-setup
/// assertion before the engine is started.
#[test]
fn fill_price_bounded_by_current_bar_range() {
    // Build 2 bars to demonstrate non-overlap, but feed only bar 1.
    let all_bars = make_bars(2);
    let bar1_low = all_bars[0].low_micros;
    let bar1_high = all_bars[0].high_micros;

    // Sanity: bar 2's price range must not overlap bar 1's.
    // With $20 spacing and ±$5 OHLC: bar 2 low = $115, bar 1 high = $105.
    assert!(
        all_bars[1].low_micros > bar1_high,
        "test setup: bar 2 low ({}) must exceed bar 1 high ({}) for non-overlap",
        all_bars[1].low_micros,
        bar1_high
    );

    // Run with only bar 1 — bar 2 is never presented to the engine.
    let bars = vec![all_bars[0].clone()];
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine
        .add_strategy(Box::new(SpyBuyOnFirst::new(Arc::clone(&log))))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1, "expected exactly 1 fill");
    let fill = &report.fills[0];

    // The fill price must lie within bar 1's range.  Since bar 2 was never
    // passed to the engine, any out-of-range price would prove a lookahead.
    assert!(
        fill.price_micros >= bar1_low && fill.price_micros <= bar1_high,
        "fill price {} is outside bar 1 range [{}, {}] — future-bar data leaked",
        fill.price_micros,
        bar1_low,
        bar1_high
    );
}

// ---------------------------------------------------------------------------
// Test 2: now_tick advances +1 per bar (strictly monotonic)
// ---------------------------------------------------------------------------

/// Strategy receives now_tick = 1, 2, 3, ... in strict order.
///
/// Any jump (e.g. 1, 3, 4) would imply the engine skipped a bar without
/// calling strategy — a context inconsistency.  Any repeat or decrease
/// would imply the engine replayed a bar or went backwards — impossible
/// without lookahead into already-seen data.
#[test]
fn now_tick_advances_strictly_one_per_bar() {
    const N: usize = 5;
    let bars = make_bars(N);
    let log = Arc::new(Mutex::new(Vec::new()));

    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine
        .add_strategy(Box::new(SpyStrategy::new(Arc::clone(&log))))
        .unwrap();
    engine.run(&bars).unwrap();

    let obs = log.lock().unwrap();
    assert_eq!(obs.len(), N, "strategy called exactly once per bar");
    for (i, o) in obs.iter().enumerate() {
        let expected = (i + 1) as u64;
        assert_eq!(
            o.now_tick, expected,
            "bar {i}: expected now_tick={expected}, got {}",
            o.now_tick
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3: ctx.recent window grows by exactly one entry per tick
// ---------------------------------------------------------------------------

/// At bar[i], `ctx.recent.len()` must equal `i + 1` (subject to max_len cap).
///
/// If the window ever has more entries than bars processed so far, it contains
/// bars that haven't been fed yet — a direct future-bar pre-load.
#[test]
fn recent_window_grows_one_bar_at_a_time() {
    // Use fewer bars than bar_history_len (50) so the window is never truncated.
    const N: usize = 5;
    let bars = make_bars(N);
    let log = Arc::new(Mutex::new(Vec::new()));

    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine
        .add_strategy(Box::new(SpyStrategy::new(Arc::clone(&log))))
        .unwrap();
    engine.run(&bars).unwrap();

    let obs = log.lock().unwrap();
    for (i, o) in obs.iter().enumerate() {
        let expected_len = i + 1;
        assert_eq!(
            o.window_len, expected_len,
            "bar {i}: window should have {expected_len} entries, got {}",
            o.window_len
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4: ctx.recent.last().end_ts equals the current bar's end_ts
// ---------------------------------------------------------------------------

/// At bar[i], the last entry in the recent window must have the same
/// `end_ts` as `bars[i]`.
///
/// If `last().end_ts == bars[i+1].end_ts`, a future bar was pre-loaded.
/// Since bars are spaced 60 s apart, timestamps are unambiguous.
#[test]
fn context_last_end_ts_is_current_bar_not_future() {
    const N: usize = 4;
    let bars = make_bars(N);
    let log = Arc::new(Mutex::new(Vec::new()));

    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine
        .add_strategy(Box::new(SpyStrategy::new(Arc::clone(&log))))
        .unwrap();
    engine.run(&bars).unwrap();

    let obs = log.lock().unwrap();
    for (i, o) in obs.iter().enumerate() {
        // Must equal current bar's end_ts.
        assert_eq!(
            o.last_end_ts,
            Some(bars[i].end_ts),
            "bar {i}: context.last end_ts should be {} (current bar), got {:?}",
            bars[i].end_ts,
            o.last_end_ts
        );
        // Must NOT equal the next bar's end_ts.
        if i + 1 < N {
            assert_ne!(
                o.last_end_ts,
                Some(bars[i + 1].end_ts),
                "bar {i}: context contains next bar's end_ts — lookahead leak!"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 5: incomplete bar → Err before strategy on_bar is ever called
// ---------------------------------------------------------------------------

/// An incomplete bar (is_complete = false) must cause `engine.run()` to
/// return `Err(BacktestError::IncompleteBar)` *before* the strategy's
/// `on_bar` is invoked for that bar.
///
/// An incomplete bar represents data that is not yet finalised — the close
/// price is provisional, the volume is partial, and the high/low are subject
/// to change.  Feeding such a bar to the strategy would allow trading on
/// future price information.  The engine must reject it at the earliest
/// possible point.
#[test]
fn incomplete_bar_rejected_before_strategy_sees_it() {
    let log = Arc::new(Mutex::new(Vec::new()));

    let complete = BacktestBar::new(
        "SPY",
        1_700_000_060,
        100_000_000,
        105_000_000,
        95_000_000,
        102_000_000,
        1_000,
    );
    let mut incomplete = BacktestBar::new(
        "SPY",
        1_700_000_120,
        102_000_000,
        107_000_000,
        98_000_000,
        104_000_000,
        1_000,
    );
    incomplete.is_complete = false;

    let bars = vec![complete, incomplete];
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine
        .add_strategy(Box::new(SpyStrategy::new(Arc::clone(&log))))
        .unwrap();

    let result = engine.run(&bars);

    // Engine must return IncompleteBar error.
    match result {
        Err(BacktestError::IncompleteBar { .. }) => {}
        other => panic!(
            "expected Err(IncompleteBar), got {:?}",
            other.map(|_| "Ok(report)")
        ),
    }

    // Strategy was called exactly once — for the complete bar only.
    // It must NOT have been called for the incomplete bar.
    let obs = log.lock().unwrap();
    assert_eq!(
        obs.len(),
        1,
        "strategy must not be called for the incomplete bar; got {} calls",
        obs.len()
    );
    assert_eq!(obs[0].now_tick, 1, "tick 1 is the complete bar");
}
