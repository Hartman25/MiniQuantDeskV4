//! INTRADAY-MD-REFRESHER-01: completed-bar refresh safety proof.
//!
//! These tests exercise the completed-bar filtering and freshness-gate contract
//! with fake provider rows only. They do not call providers, brokers, order
//! routes, or the database.

use mqk_daemon::market_data_freshness::{
    evaluate_md_freshness_snapshot, DEFAULT_INTRADAY_BAR_MAX_AGE_SECS,
    REASON_CODE_INTRADAY_BAR_STALE,
};
use mqk_md::{filter_completed_provider_bars, refresh_attempt_decision, ProviderBar, Timeframe};

const NOW_TS: i64 = 2_000_000_000;

fn provider_bar(end_ts: i64, is_complete: bool) -> ProviderBar {
    ProviderBar {
        symbol: "AAPL".to_string(),
        timeframe: "5m".to_string(),
        end_ts,
        open: "200".to_string(),
        high: "201".to_string(),
        low: "199".to_string(),
        close: "200.5".to_string(),
        volume: 1_000,
        is_complete,
    }
}

#[test]
fn rf_01_fake_completed_aapl_5m_refresh_is_fresh_for_gate() {
    let fake_provider_rows = vec![
        provider_bar(NOW_TS - 1_500, true),
        provider_bar(NOW_TS - 1_200, true),
        provider_bar(NOW_TS - 900, true),
        provider_bar(NOW_TS - 600, true),
        provider_bar(NOW_TS - 300, true),
    ];

    let (completed_rows, report) =
        filter_completed_provider_bars(fake_provider_rows, Timeframe::M5, NOW_TS);

    assert_eq!(report.rows_kept, 5);
    assert_eq!(report.rows_dropped_current, 0);
    assert_eq!(report.latest_completed_end_ts, Some(NOW_TS - 300));

    let freshness = evaluate_md_freshness_snapshot(
        "AAPL",
        "5m",
        completed_rows.len() as u64,
        report.latest_completed_end_ts,
        NOW_TS,
    );
    assert_eq!(freshness.freshness_state, "ok");
    assert_eq!(freshness.reason_code, "ok");
    assert_eq!(freshness.age_seconds, Some(300));
}

#[test]
fn rf_02_current_or_incomplete_5m_bar_is_not_usable_completed_input() {
    let fake_provider_rows = vec![
        provider_bar(NOW_TS - 1_500, true),
        provider_bar(NOW_TS - 1_200, true),
        provider_bar(NOW_TS - 900, true),
        provider_bar(NOW_TS - 600, true),
        provider_bar(NOW_TS - 300, true),
        provider_bar(NOW_TS - 60, true),
        provider_bar(NOW_TS - 1_800, false),
    ];

    let (completed_rows, report) =
        filter_completed_provider_bars(fake_provider_rows, Timeframe::M5, NOW_TS);

    assert_eq!(completed_rows.len(), 5);
    assert_eq!(report.rows_dropped_current, 1);
    assert_eq!(report.rows_dropped_incomplete_flag, 1);
    assert_eq!(
        report.latest_completed_end_ts,
        Some(NOW_TS - 300),
        "the in-progress current bar must not advance latest completed bar time"
    );
}

#[test]
fn rf_03_rate_limit_suppresses_too_frequent_refresh_attempts() {
    let decision = refresh_attempt_decision(Some(NOW_TS - 120), NOW_TS, 300);

    assert!(!decision.allowed);
    assert_eq!(decision.retry_after_secs, Some(180));
    assert!(
        decision
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("min_refresh_interval_secs=300"),
        "operator reason must name the configured minimum interval"
    );
}

#[test]
fn rf_04_failed_or_disabled_refresh_leaves_stale_5m_blocked() {
    let stale_latest = NOW_TS - DEFAULT_INTRADAY_BAR_MAX_AGE_SECS - 60;
    let freshness = evaluate_md_freshness_snapshot("AAPL", "5m", 5, Some(stale_latest), NOW_TS);

    assert_eq!(freshness.freshness_state, "stale");
    assert_eq!(freshness.reason_code, REASON_CODE_INTRADAY_BAR_STALE);
    assert!(
        freshness.age_seconds.unwrap_or_default() > freshness.max_allowed_age_seconds,
        "stale rows must remain blocked if refresh is disabled or failed"
    );
}
