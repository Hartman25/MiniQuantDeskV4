use mqk_integrity::*;

#[test]
fn scenario_feed_disagreement_halt() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: true,
        calendar: CalendarSpec::AlwaysOn,
    };
    let mut st = IntegrityState::new();

    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");
    let tf = Timeframe::secs(60);

    let key = BarKey::new("SPY", tf, 1000);

    // Feed A bar
    let a = Bar::new(key.clone(), true, 500_000_000, 1000);
    let d1 = evaluate_bar(&cfg, &mut st, &feed_a, 1, &a);
    assert_eq!(d1.action, IntegrityAction::Allow);

    // Feed B reports same bar key but different close => HALT
    let b = Bar::new(key, true, 501_000_000, 1000);
    let d2 = evaluate_bar(&cfg, &mut st, &feed_b, 1, &b);
    assert_eq!(d2.action, IntegrityAction::Halt);
    assert_eq!(d2.reason, IntegrityReason::FeedDisagreement);
    assert!(st.halted);
}
