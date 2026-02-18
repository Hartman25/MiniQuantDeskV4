//! PATCH 22 — Integrity DISARM propagates to risk path.
//!
//! Verifies that when integrity disarms (stale feed), the execution gate
//! (`is_execution_blocked()`) returns true, which is the integration point
//! for risk/execution to reject all new orders.
//!
//! Pattern: integrity state is the single source of truth for "should we
//! allow new order submission?" — checked via `is_execution_blocked()`.

use mqk_integrity::*;

// ========================== TESTS ==========================

/// When integrity disarms due to stale feed, is_execution_blocked() returns true.
#[test]
fn stale_disarm_blocks_execution_gate() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 5,
        enforce_feed_disagreement: false,
    };
    let mut st = IntegrityState::new();
    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");

    // Gate should be open initially.
    assert!(
        !st.is_execution_blocked(),
        "should not be blocked initially"
    );

    // Seed both feeds.
    tick_feed(&cfg, &mut st, &feed_a, 10);
    tick_feed(&cfg, &mut st, &feed_b, 10);
    assert!(
        !st.is_execution_blocked(),
        "should not be blocked after seeding"
    );

    // Advance only feed_a; feed_b becomes stale at tick=16 (>5 ticks since 10).
    let decision = tick_feed(&cfg, &mut st, &feed_a, 16);
    assert_eq!(decision.action, IntegrityAction::Disarm);
    assert_eq!(decision.reason, IntegrityReason::StaleFeed);

    // Gate should now block execution.
    assert!(
        st.is_execution_blocked(),
        "execution must be blocked after stale disarm"
    );
    assert!(st.disarmed, "disarmed flag must be set");
}

/// When integrity halts due to gap, is_execution_blocked() returns true.
#[test]
fn gap_halt_blocks_execution_gate() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0, // disabled
        enforce_feed_disagreement: false,
    };
    let mut st = IntegrityState::new();
    let feed = FeedId::new("main");
    let tf = Timeframe::secs(60);

    // First bar: sets baseline.
    let bar1 = Bar::new(BarKey::new("SPY", tf, 1000), true, 500_000_000, 100);
    let d1 = evaluate_bar(&cfg, &mut st, &feed, 1, &bar1);
    assert_eq!(d1.action, IntegrityAction::Allow);
    assert!(!st.is_execution_blocked());

    // Second bar: 3-minute gap (skipped 2 bars) => HALT.
    let bar2 = Bar::new(BarKey::new("SPY", tf, 1180), true, 500_000_000, 100);
    let d2 = evaluate_bar(&cfg, &mut st, &feed, 2, &bar2);
    assert_eq!(d2.action, IntegrityAction::Halt);
    assert_eq!(d2.reason, IntegrityReason::GapDetected);

    // Gate should block.
    assert!(
        st.is_execution_blocked(),
        "execution must be blocked after gap halt"
    );
    assert!(st.halted, "halted flag must be set");
}

/// Disarm is sticky: once disarmed, all subsequent decisions are AlreadyDisarmed
/// and execution remains blocked.
#[test]
fn disarm_is_sticky_execution_stays_blocked() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 100,
        stale_threshold_ticks: 5,
        enforce_feed_disagreement: false,
    };
    let mut st = IntegrityState::new();
    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");

    // Seed and trigger disarm.
    tick_feed(&cfg, &mut st, &feed_a, 10);
    tick_feed(&cfg, &mut st, &feed_b, 10);
    tick_feed(&cfg, &mut st, &feed_a, 16); // disarms

    assert!(st.is_execution_blocked());

    // Subsequent ticks still blocked (AlreadyDisarmed).
    let d = tick_feed(&cfg, &mut st, &feed_a, 17);
    assert_eq!(d.action, IntegrityAction::Disarm);
    assert_eq!(d.reason, IntegrityReason::AlreadyDisarmed);
    assert!(st.is_execution_blocked(), "must stay blocked");

    // Even evaluating a bar stays blocked.
    let tf = Timeframe::secs(60);
    let bar = Bar::new(BarKey::new("SPY", tf, 1060), true, 500_000_000, 100);
    let d2 = evaluate_bar(&cfg, &mut st, &feed_a, 18, &bar);
    assert_eq!(d2.action, IntegrityAction::Disarm);
    assert_eq!(d2.reason, IntegrityReason::AlreadyDisarmed);
    assert!(st.is_execution_blocked(), "must remain blocked");
}

/// Halt is sticky: once halted, execution stays blocked.
#[test]
fn halt_is_sticky_execution_stays_blocked() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
    };
    let mut st = IntegrityState::new();
    let feed = FeedId::new("main");
    let tf = Timeframe::secs(60);

    // Bar 1 sets baseline.
    let bar1 = Bar::new(BarKey::new("SPY", tf, 1000), true, 500_000_000, 100);
    evaluate_bar(&cfg, &mut st, &feed, 1, &bar1);

    // Bar 2 has gap => HALT.
    let bar2 = Bar::new(BarKey::new("SPY", tf, 1180), true, 500_000_000, 100);
    evaluate_bar(&cfg, &mut st, &feed, 2, &bar2);
    assert!(st.is_execution_blocked());

    // Bar 3 after halt: still blocked (AlreadyHalted).
    let bar3 = Bar::new(BarKey::new("SPY", tf, 1240), true, 500_000_000, 100);
    let d3 = evaluate_bar(&cfg, &mut st, &feed, 3, &bar3);
    assert_eq!(d3.action, IntegrityAction::Halt);
    assert_eq!(d3.reason, IntegrityReason::AlreadyHalted);
    assert!(st.is_execution_blocked(), "must remain blocked after halt");
}

/// Both halted and disarmed: execution is blocked.
#[test]
fn both_halted_and_disarmed_blocks() {
    let mut st = IntegrityState::new();
    assert!(!st.is_execution_blocked());

    st.disarmed = true;
    assert!(st.is_execution_blocked());

    st.halted = true;
    assert!(st.is_execution_blocked());

    // Only disarmed (not halted).
    st.halted = false;
    assert!(st.is_execution_blocked());

    // Only halted (not disarmed).
    st.disarmed = false;
    st.halted = true;
    assert!(st.is_execution_blocked());

    // Neither.
    st.halted = false;
    assert!(!st.is_execution_blocked());
}

/// The gate function is the single integration point: downstream consumers
/// only need to check `is_execution_blocked()` — not individual flags.
///
/// Uses two feeds: "main" advances normally, "heartbeat" stays at tick=1.
/// When main advances to tick=5, heartbeat's delta (5-1=4) exceeds threshold 3 => DISARM.
#[test]
fn gate_function_is_single_integration_point() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 100,
        stale_threshold_ticks: 3,
        enforce_feed_disagreement: false,
    };
    let mut st = IntegrityState::new();
    let main_feed = FeedId::new("main");
    let heartbeat = FeedId::new("heartbeat");

    // Seed both feeds at tick=1. Gate is open.
    tick_feed(&cfg, &mut st, &main_feed, 1);
    tick_feed(&cfg, &mut st, &heartbeat, 1);
    assert!(!st.is_execution_blocked());

    // Advance only main_feed to tick=5.
    // Heartbeat is still at tick=1, delta = 5 - 1 = 4 > threshold 3 => DISARM.
    let d = tick_feed(&cfg, &mut st, &main_feed, 5);
    assert_eq!(d.action, IntegrityAction::Disarm);
    assert_eq!(d.reason, IntegrityReason::StaleFeed);

    // Gate closed — this is the ONLY check a risk/execution module needs.
    assert!(st.is_execution_blocked());
}
