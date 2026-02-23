use mqk_integrity::*;

#[test]
fn scenario_incomplete_bar_rejection() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: true,
        calendar: CalendarSpec::AlwaysOn,
    };
    let mut st = IntegrityState::new();
    let feed = FeedId::new("feedA");
    let tf = Timeframe::secs(60);

    let bar = Bar::new(
        BarKey::new("SPY", tf, 1_700_000_000),
        false,
        500_000_000,
        1000,
    );

    let d = evaluate_bar(&cfg, &mut st, &feed, 10, &bar);
    assert_eq!(d.action, IntegrityAction::Reject);
    assert_eq!(d.reason, IntegrityReason::IncompleteBar);
    assert!(!st.halted);
    assert!(!st.disarmed);
}
