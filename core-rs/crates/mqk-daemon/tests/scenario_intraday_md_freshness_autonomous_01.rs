//! INTRADAY-MD-FRESHNESS-AUTONOMOUS-01: autonomous intraday bar freshness proof.
//!
//! Proves the autonomous dispatch gate uses a tight intraday completed-bar cap
//! before native strategy dispatch can create a target, while preserving the
//! existing daily/1D 4-day tolerance.
//!
//! Tests use in-memory `MdBarRow` fixtures through the same post-fetch dispatch
//! helper that production uses after reading `md_bars`. They do not require a
//! DB URL and do not call providers, brokers, order routes, or market-data
//! provider code.
//!
//! | Test | What it proves |
//! |------|----------------|
//! | I01  | 5m is intraday, default intraday cap constant is 900s, 1D is 345600s |
//! | I02  | Fresh 5m bars within the 900s cap dispatch normally |
//! | I03  | Stale 5m bars beyond the cap return no target and surface `intraday_bar_stale` |
//! | I04  | Missing 5m bars return no target and surface `intraday_bar_not_current` |
//! | I05  | Daily/1D weekend-sized gaps remain within the 4-day tolerance |
//! | I06  | Multi-symbol loop omits stale intraday symbol while preserving fresh symbol |

use std::collections::BTreeMap;
use std::sync::Arc;

use mqk_daemon::{
    market_data_freshness::{
        default_max_allowed_age_secs_for_timeframe, evaluate_md_freshness_snapshot,
        is_intraday_timeframe, timeframe_secs, DEFAULT_INTRADAY_BAR_MAX_AGE_SECS,
        MD_FRESHNESS_STALE_SECS, REASON_CODE_INTRADAY_BAR_NOT_CURRENT,
        REASON_CODE_INTRADAY_BAR_STALE,
    },
    state::{self, classify_bar_staleness, AppState, StrategyBarInput, SymbolStrategyAssignment},
};
use mqk_runtime::native_strategy::NativeStrategyBootstrap;

const NOW_TS: i64 = 2_000_000_000;

fn active_scalper_bootstrap(symbol: &str) -> NativeStrategyBootstrap {
    let mut registry = mqk_strategy::PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol)
        .expect("built-in strategy registration must succeed");
    let ids = vec!["intraday_scalper".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &registry)
}

async fn state_with_scalper(strategy_symbol: &str) -> Arc<AppState> {
    let st = AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    st.set_native_strategy_bootstrap_for_test(Some(active_scalper_bootstrap(strategy_symbol)))
        .await;
    Arc::new(st)
}

fn test_bar_input(now_tick: u64) -> StrategyBarInput {
    StrategyBarInput {
        now_tick,
        end_ts: 1_700_000_000,
        limit_price: Some(200_000_000),
        qty: 1,
    }
}

fn assignment(symbol: &str) -> SymbolStrategyAssignment {
    SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: "intraday_scalper".to_string(),
        timeframe: "5m".to_string(),
    }
}

fn scalper_rows(symbol: &str, timeframe: &str, latest_end_ts: i64) -> Vec<mqk_db::MdBarRow> {
    let base = 200_000_000_i64;
    let closes = [base, base, base, base, base + base / 400];
    closes
        .iter()
        .enumerate()
        .map(|(idx, close)| mqk_db::MdBarRow {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            end_ts: latest_end_ts - ((closes.len() - 1 - idx) as i64 * 300),
            open_micros: base,
            high_micros: base,
            low_micros: base,
            close_micros: *close,
            volume: 1_000,
            is_complete: true,
        })
        .collect()
}

#[test]
fn i01_intraday_timeframe_default_is_tight_and_daily_stays_broad() {
    assert_eq!(timeframe_secs("5m"), Some(300));
    assert_eq!(timeframe_secs("5Min"), Some(300));
    assert_eq!(timeframe_secs("1H"), Some(3_600));
    assert!(is_intraday_timeframe("5m"));
    assert!(is_intraday_timeframe("1H"));
    assert!(!is_intraday_timeframe("1D"));
    assert_eq!(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS, 900);
    assert_eq!(
        default_max_allowed_age_secs_for_timeframe("1D"),
        MD_FRESHNESS_STALE_SECS
    );
}

#[tokio::test]
async fn i02_fresh_5m_bar_within_default_cap_dispatches_normally() {
    let symbol = "IMDF02FRESH";
    let now_ts = NOW_TS;
    let rows = scalper_rows(symbol, "5m", now_ts - 60);

    let st = state_with_scalper(symbol).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS));

    let result = st
        .dispatch_native_strategy_for_symbol_with_loaded_bars_for_test(
            symbol,
            "5m",
            test_bar_input(1),
            rows,
            now_ts,
        )
        .await;
    let result = result.expect("I02: fresh 5m bars must dispatch");
    assert!(
        result
            .intents
            .output
            .targets
            .iter()
            .any(|t| t.symbol == symbol && t.qty > 0),
        "I02: fresh bullish 5m bars should produce a positive target for {symbol}; got {:?}",
        result.intents.output.targets
    );
}

#[tokio::test]
async fn i03_stale_5m_bar_blocks_dispatch_and_surfaces_reason() {
    let symbol = "IMDF03STALE";
    let now_ts = NOW_TS;
    let stale_age_secs = DEFAULT_INTRADAY_BAR_MAX_AGE_SECS
        .max(default_max_allowed_age_secs_for_timeframe("5m"))
        + 60;
    let rows = scalper_rows(symbol, "5m", now_ts - stale_age_secs);

    let status = evaluate_md_freshness_snapshot(
        symbol,
        "5m",
        rows.len() as u64,
        rows.last().map(|r| r.end_ts),
        now_ts,
    );
    assert_eq!(status.freshness_state, "stale");
    assert_eq!(status.reason_code, REASON_CODE_INTRADAY_BAR_STALE);
    assert_eq!(
        status.max_allowed_age_seconds,
        default_max_allowed_age_secs_for_timeframe("5m")
    );
    assert!(
        status.age_seconds.unwrap_or_default() > status.max_allowed_age_seconds,
        "I03: age_seconds must exceed max_allowed_age_seconds: {status:?}"
    );
    assert!(
        status.reason.contains("latest_completed_bar_ts=")
            && status.reason.contains("now_utc=")
            && status.reason.contains("age_seconds=")
            && status.reason.contains("max_allowed_age_seconds="),
        "I03: reason must include exact diagnostics: {}",
        status.reason
    );

    let st = state_with_scalper(symbol).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS));

    let result = st
        .dispatch_native_strategy_for_symbol_with_loaded_bars_for_test(
            symbol,
            "5m",
            test_bar_input(1),
            rows,
            now_ts,
        )
        .await;
    assert!(
        result.is_none(),
        "I03: stale 5m bars must not produce a StrategyBarResult/target"
    );
    assert!(
        st.last_strategy_diagnostics().await.is_none(),
        "I03: stale gate must refuse before strategy diagnostics/decision are produced"
    );
}

#[tokio::test]
async fn i04_missing_5m_bar_blocks_dispatch_and_surfaces_reason() {
    let symbol = "IMDF04MISS";
    let rows = Vec::new();
    let now_ts = NOW_TS;

    let status = evaluate_md_freshness_snapshot(symbol, "5m", 0, None, now_ts);
    assert_eq!(status.freshness_state, "missing");
    assert_eq!(status.reason_code, REASON_CODE_INTRADAY_BAR_NOT_CURRENT);
    assert!(status.latest_completed_bar_ts.is_none());
    assert!(status.age_seconds.is_none());
    assert!(
        status.reason.contains("latest_completed_bar_ts=null")
            && status.reason.contains("age_seconds=null")
            && status.reason.contains("max_allowed_age_seconds="),
        "I04: reason must include exact missing-bar diagnostics: {}",
        status.reason
    );

    let st = state_with_scalper(symbol).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS));

    let result = st
        .dispatch_native_strategy_for_symbol_with_loaded_bars_for_test(
            symbol,
            "5m",
            test_bar_input(1),
            rows,
            now_ts,
        )
        .await;
    assert!(
        result.is_none(),
        "I04: missing 5m bars must not produce a StrategyBarResult/target"
    );
    assert!(
        st.last_strategy_diagnostics().await.is_none(),
        "I04: missing-bar gate must refuse before strategy diagnostics/decision are produced"
    );
}

#[test]
fn i05_daily_weekend_gap_keeps_four_day_tolerance() {
    let now_ts = 2_000_000_000_i64;
    let friday_to_monday_gap_secs = 3 * 24 * 3_600;
    assert_eq!(default_max_allowed_age_secs_for_timeframe("1D"), 345_600);
    assert_eq!(
        classify_bar_staleness(
            Some(now_ts - friday_to_monday_gap_secs),
            now_ts,
            Some(default_max_allowed_age_secs_for_timeframe("1D")),
        ),
        Some(false),
        "I05: a normal 3-day weekend daily gap must remain accepted under the 4-day cap"
    );
}

#[tokio::test]
async fn i06_multi_symbol_stale_5m_does_not_block_fresh_5m_symbol() {
    let fresh_symbol = "IMDF06FRESH";
    let stale_symbol = "IMDF06STALE";
    let now_ts = NOW_TS;
    let stale_age_secs = DEFAULT_INTRADAY_BAR_MAX_AGE_SECS
        .max(default_max_allowed_age_secs_for_timeframe("5m"))
        + 60;
    let fresh_rows = scalper_rows(fresh_symbol, "5m", now_ts - 60);
    let stale_rows = scalper_rows(stale_symbol, "5m", now_ts - stale_age_secs);
    let mut rows_by_symbol = BTreeMap::new();
    rows_by_symbol.insert(fresh_symbol.to_string(), fresh_rows);
    rows_by_symbol.insert(stale_symbol.to_string(), stale_rows);

    let st = state_with_scalper(fresh_symbol).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS));

    let results = st
        .tick_strategy_dispatch_multi_symbol_with_loaded_bars_for_test(
            &[assignment(fresh_symbol), assignment(stale_symbol)],
            test_bar_input(1),
            &rows_by_symbol,
            now_ts,
        )
        .await;

    assert!(
        results.iter().any(|(a, _)| a.symbol == fresh_symbol),
        "I06: fresh symbol must dispatch; got {:?}",
        results.iter().map(|(a, _)| &a.symbol).collect::<Vec<_>>()
    );
    assert!(
        results.iter().all(|(a, _)| a.symbol != stale_symbol),
        "I06: stale symbol must be omitted without blocking the fresh symbol; got {:?}",
        results.iter().map(|(a, _)| &a.symbol).collect::<Vec<_>>()
    );
}
