//! PATCH 22 — Stale Data -> Execution Path Kill (End-to-End)
//!
//! Verifies that when the integrity engine detects stale feed data,
//! execution is blocked: no new fills occur after the disarm point.
//!
//! The backtest uses a multi-feed scenario: a "heartbeat" feed is seeded
//! at the first bar's timestamp, then never updated. As the main "backtest"
//! feed advances with each bar, the heartbeat goes stale, triggering DISARM,
//! which gates all subsequent order submissions.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, StressProfile};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Strategy: Buys 10 shares on bar 1, sells on bar 5.
// If execution is blocked by stale disarm, the bar-5 sell never fills.
// ---------------------------------------------------------------------------
struct BuySellStrategy {
    bar_idx: u64,
}

impl BuySellStrategy {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for BuySellStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuySellStrategy", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            // Hold position (re-emit target) until sell at bar 7
            7 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            // Maintain current position by re-emitting the target
            _ => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
        }
    }
}

// ---------------------------------------------------------------------------
// Strategy: Attempts to buy on every bar — any fill after disarm is a failure.
// ---------------------------------------------------------------------------
struct BuyEveryBarStrategy {
    bar_idx: u64,
}

impl BuyEveryBarStrategy {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for BuyEveryBarStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyEveryBar", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        // Every bar: target 10 * bar_idx shares (increasing position)
        let target = (self.bar_idx * 10) as i64;
        StrategyOutput::new(vec![TargetPosition::new("SPY", target)])
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Config with integrity enabled and stale threshold active.
///
/// `stale_threshold_ticks` is compared against `(now_tick - last_feed_tick)`,
/// where the backtest uses `bar.end_ts` as the tick value (in seconds).
/// So `stale_threshold_ticks = 120` means a feed is stale if its last update
/// was more than 120 seconds ago.
fn config_with_integrity(stale_threshold_ticks: u64) -> BacktestConfig {
    BacktestConfig {
        timeframe_secs: 60,
        bar_history_len: 50,
        initial_cash_micros: 100_000_000_000, // 100k USD
        shadow_mode: false,
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects: 100,
        pdt_enabled: false,
        kill_switch_flattens: true,
        max_gross_exposure_mult_micros: 10_000_000, // 10x (generous)
        stress: StressProfile { slippage_bps: 0 },
        // PATCH 22: integrity ON
        integrity_enabled: true,
        integrity_stale_threshold_ticks: stale_threshold_ticks,
        integrity_gap_tolerance_bars: 100, // large so gap detection doesn't trigger halt
        integrity_enforce_feed_disagreement: false,
        integrity_calendar: mqk_integrity::CalendarSpec::AlwaysOn, // Patch B3
    }
}

/// Create normal consecutive 1-minute bars.
fn make_normal_bars(count: usize) -> Vec<BacktestBar> {
    let base_ts = 1_700_000_000i64;
    (0..count)
        .map(|i| {
            BacktestBar::new(
                "SPY",
                base_ts + ((i as i64) + 1) * 60,
                500_000_000, // open
                510_000_000, // high
                490_000_000, // low
                505_000_000, // close
                1000,        // volume
            )
        })
        .collect()
}

/// Create bars where bars 1-2 are normal, then there's a large time gap
/// (simulating stale data / market close), then bars continue.
fn make_bars_with_stale_gap(pre_gap: usize, gap_secs: i64, post_gap: usize) -> Vec<BacktestBar> {
    let base_ts = 1_700_000_000i64;
    let mut bars = Vec::new();

    // Pre-gap bars (normal 1-minute intervals)
    for i in 0..pre_gap {
        bars.push(BacktestBar::new(
            "SPY",
            base_ts + ((i as i64) + 1) * 60,
            500_000_000,
            510_000_000,
            490_000_000,
            505_000_000,
            1000,
        ));
    }

    // Post-gap bars (resume after gap)
    let gap_resume_ts = base_ts + (pre_gap as i64) * 60 + gap_secs;
    for i in 0..post_gap {
        bars.push(BacktestBar::new(
            "SPY",
            gap_resume_ts + ((i as i64) + 1) * 60,
            500_000_000,
            510_000_000,
            490_000_000,
            505_000_000,
            1000,
        ));
    }

    bars
}

// ========================== TESTS ==========================

/// Baseline: with integrity enabled but no stale condition, all fills execute normally.
/// The heartbeat feed is seeded at bar 1's timestamp, and the stale threshold is
/// large enough that it never fires within the test's bar sequence.
#[test]
fn integrity_enabled_no_stale_all_fills_execute() {
    // threshold=999999: heartbeat never goes stale in an 8-bar sequence
    let cfg = config_with_integrity(999_999);
    let bars = make_normal_bars(8);

    let mut engine = BacktestEngine::new(cfg);
    // Seed heartbeat at first bar's end_ts
    engine.seed_integrity_feed("heartbeat", bars[0].end_ts as u64);
    engine
        .add_strategy(Box::new(BuySellStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Should have fills: buy at bar 1, sell at bar 7
    assert!(
        report.fills.len() >= 2,
        "expected at least 2 fills, got {}",
        report.fills.len()
    );
    assert!(!report.execution_blocked, "should NOT be blocked");
    assert!(!engine.is_execution_blocked());
}

/// Core test: stale feed disarm blocks all subsequent execution.
///
/// A "heartbeat" feed is seeded at the first bar's timestamp. The stale threshold
/// is 120 seconds (2 bars at 60s intervals). After bar 3, the heartbeat feed is
/// 3*60=180 seconds stale, which exceeds the 120s threshold => DISARM.
///
/// Bars 1-2 should produce fills; bar 3+ should be blocked.
#[test]
fn stale_disarm_blocks_execution_after_trigger() {
    // Threshold = 120 seconds. Bar 1 is at base+60, heartbeat seeded at base+60.
    // Bar 2: now_tick = base+120, heartbeat at base+60, delta=60 (<=120) => OK.
    // Bar 3: now_tick = base+180, heartbeat at base+60, delta=120 (NOT > 120) => OK.
    // Bar 4: now_tick = base+240, heartbeat at base+60, delta=180 (>120) => DISARM.
    let cfg = config_with_integrity(120);
    let bars = make_normal_bars(8);

    let mut engine = BacktestEngine::new(cfg);
    // Seed heartbeat at first bar's timestamp
    engine.seed_integrity_feed("heartbeat", bars[0].end_ts as u64);
    engine
        .add_strategy(Box::new(BuyEveryBarStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Bars 1-3 should produce fills (delta <= threshold).
    // Bar 4: delta = 180 > 120 => DISARM => no more fills.
    let fill_count = report.fills.len();
    assert!(
        fill_count >= 1,
        "should have at least 1 fill from early bars"
    );
    assert!(
        fill_count <= 3,
        "expected at most 3 fills (bars 1-3 only), got {}; stale disarm should block bar 4+",
        fill_count
    );

    assert!(
        report.execution_blocked,
        "report should indicate execution was blocked"
    );
    assert!(
        engine.is_execution_blocked(),
        "engine should indicate execution blocked"
    );
    assert!(
        engine.integrity_state().disarmed,
        "integrity state should be disarmed"
    );
}

/// After stale disarm, equity curve still records values (strategy runs),
/// but no new fills are created.
#[test]
fn stale_disarm_equity_curve_continues_without_fills() {
    // threshold=120 => disarm fires at bar 4 (delta=180 > 120)
    let cfg = config_with_integrity(120);
    let bars = make_normal_bars(6);

    let mut engine = BacktestEngine::new(cfg);
    engine.seed_integrity_feed("heartbeat", bars[0].end_ts as u64);
    engine
        .add_strategy(Box::new(BuyEveryBarStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Equity curve should have entries for all 6 bars
    // (strategy still processes, just no execution after disarm).
    assert_eq!(
        report.equity_curve.len(),
        6,
        "equity curve should have entries for all bars even after disarm"
    );

    // Fills only from bars 1-3 (pre-disarm)
    let fill_count = report.fills.len();
    assert!(
        fill_count <= 3,
        "expected at most 3 fills (pre-disarm), got {}",
        fill_count
    );
    assert!(report.execution_blocked);
}

/// With a large time gap in the bar sequence, stale detection fires
/// on the first bar after the gap, blocking all subsequent execution.
#[test]
fn large_time_gap_triggers_stale_disarm() {
    // 3 normal bars, then a 600-second gap (10 minutes), then 3 more bars.
    // Threshold = 300 seconds (5 minutes).
    // Heartbeat seeded at bar 1's timestamp.
    //
    // Pre-gap bars pass (delta grows but stays within threshold + gap).
    // First post-gap bar: delta from heartbeat > 300 => DISARM.
    let cfg = config_with_integrity(300);
    let bars = make_bars_with_stale_gap(3, 600, 3);

    let mut engine = BacktestEngine::new(cfg);
    engine.seed_integrity_feed("heartbeat", bars[0].end_ts as u64);
    engine
        .add_strategy(Box::new(BuyEveryBarStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Pre-gap bars: at most 3 fills.
    // Post-gap bars: 0 fills (stale disarm blocks them).
    assert!(
        report.fills.len() <= 3,
        "expected fills only from pre-gap bars, got {}",
        report.fills.len()
    );
    assert!(
        report.execution_blocked,
        "should be blocked after stale gap"
    );
    assert!(engine.integrity_state().disarmed);
}

/// Zero fills after disarm point — not even risk-reducing orders go through.
/// The strategy buys 10 shares on bar 1, holds through bars 2-6 (maintaining
/// target=10), then attempts to sell (target=0) on bar 7. Since disarm fires
/// at bar 4, the sell on bar 7 is blocked. Position remains open.
#[test]
fn stale_disarm_blocks_all_order_types() {
    // threshold=120 => disarm fires at bar 4 (delta=180 > 120).
    // Strategy: bar 1 = buy 10, bars 2-6 = hold 10, bar 7 = sell to 0.
    // The bar 7 sell is after disarm and should NOT execute.
    let cfg = config_with_integrity(120);
    let bars = make_normal_bars(8);

    let mut engine = BacktestEngine::new(cfg);
    engine.seed_integrity_feed("heartbeat", bars[0].end_ts as u64);
    engine
        .add_strategy(Box::new(BuySellStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Bar 1 fill (buy) should succeed (before disarm).
    // Bar 7 sell is blocked (after disarm at bar 4).
    let buy_fills: Vec<_> = report
        .fills
        .iter()
        .filter(|f| f.side == mqk_portfolio::Side::Buy)
        .collect();
    let sell_fills: Vec<_> = report
        .fills
        .iter()
        .filter(|f| f.side == mqk_portfolio::Side::Sell)
        .collect();

    assert_eq!(buy_fills.len(), 1, "should have exactly 1 buy fill (bar 1)");
    assert_eq!(
        sell_fills.len(),
        0,
        "should have 0 sell fills (bar 7 sell blocked by disarm)"
    );
    assert!(report.execution_blocked);
}

/// Integrity disabled (default): no disarm, all fills execute.
#[test]
fn integrity_disabled_no_blocking() {
    let cfg = BacktestConfig::test_defaults(); // integrity_enabled = false
    let bars = make_normal_bars(5);

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(BuyEveryBarStrategy::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // All bars should produce fills (increasing position)
    assert!(
        report.fills.len() >= 4,
        "expected multiple fills with integrity disabled, got {}",
        report.fills.len()
    );
    assert!(!report.execution_blocked);
}
