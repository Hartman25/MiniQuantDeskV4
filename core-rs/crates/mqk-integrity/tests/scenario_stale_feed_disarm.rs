use mqk_integrity::*;

#[test]
fn scenario_stale_feed_disarm() {
    let cfg = IntegrityConfig { gap_tolerance_bars: 0, stale_threshold_ticks: 5, enforce_feed_disagreement: false };
    let mut st = IntegrityState::new();

    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");

    // Seed both feeds as known
    tick_feed(&cfg, &mut st, &feed_a, 10);
    tick_feed(&cfg, &mut st, &feed_b, 10);

    // Advance only feed_a; feed_b becomes stale by now_tick=16 (>5 ticks since 10)
    let d = tick_feed(&cfg, &mut st, &feed_a, 16);
    assert_eq!(d.action, IntegrityAction::Disarm);
    assert_eq!(d.reason, IntegrityReason::StaleFeed);
    assert!(st.disarmed);
}
