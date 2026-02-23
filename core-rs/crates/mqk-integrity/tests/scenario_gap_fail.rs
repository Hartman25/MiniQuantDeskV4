use mqk_integrity::*;

#[test]
fn scenario_gap_fail_when_tolerance_zero() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::AlwaysOn,
    };
    let mut st = IntegrityState::new();
    let feed = FeedId::new("feedA");
    let tf = Timeframe::secs(60);

    // First complete bar at t=1000
    let b1 = Bar::new(BarKey::new("SPY", tf, 1000), true, 500_000_000, 1000);
    let d1 = evaluate_bar(&cfg, &mut st, &feed, 1, &b1);
    assert_eq!(d1.action, IntegrityAction::Allow);

    // Next bar jumps to t=1120 (missing 1 bar: expected 1060, got 1120)
    let b2 = Bar::new(BarKey::new("SPY", tf, 1120), true, 501_000_000, 1100);
    let d2 = evaluate_bar(&cfg, &mut st, &feed, 2, &b2);
    assert_eq!(d2.action, IntegrityAction::Halt);
    assert_eq!(d2.reason, IntegrityReason::GapDetected);
    assert!(st.halted);
}
