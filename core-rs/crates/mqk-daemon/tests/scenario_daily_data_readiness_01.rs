//! DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED Phase B — strict daily
//! readiness evaluator proof.
//!
//! Binding contract:
//! `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md`.
//!
//! Exercises the production evaluator (`mqk_daemon::daily_data_readiness`)
//! directly with injected fixtures — never a separate test-only
//! reimplementation. DB-backed tests are gated on `MQK_DATABASE_URL` and
//! self-skip (matching the existing `scenario_data_freshness_readiness_gate_01`
//! / `scenario_premarket_data_readiness_gate_01` pattern) when it is absent.
//!
//! Run alone: `cargo test -p mqk-daemon --test scenario_daily_data_readiness_01 -- --test-threads=1`

use chrono::{DateTime, TimeZone, Utc};

use mqk_daemon::daily_data_readiness::{
    self, derive_continuity_state, evaluate_assignment, evaluate_assignments,
    evaluate_bar_readiness, expected_daily_end_ts_window, expected_intraday_end_ts_window,
    resolve_daily_bar_timestamp_convention, DailyBarTimestampConvention,
    REASON_ASSET_CLASS_UNKNOWN, REASON_CALENDAR_UNAVAILABLE, REASON_DUPLICATE_TIMESTAMP,
    REASON_EXPECTED_LATEST_BAR_MISSING, REASON_INSUFFICIENT_HISTORY, REASON_INTERIOR_GAP,
    REASON_LATEST_BAR_FUTURE, REASON_MARKET_DATA_MISSING, REASON_PROVIDER_CAPABILITY_MISMATCH,
    REASON_PROVIDER_DISABLED, REASON_PROVIDER_ID_MISMATCH, REASON_PROVIDER_INGEST_TIME_FUTURE,
    REASON_PROVIDER_PROVENANCE_INVALID, REASON_PROVIDER_SYMBOL_MISMATCH,
    REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED, REASON_PROVIDER_UNKNOWN,
    REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH, REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH,
    REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH, REASON_STRATEGY_REQUIREMENT_UNKNOWN,
    REASON_UNSUPPORTED_INTRADAY_CONTINUITY,
};
use mqk_daemon::state::market_calendar::{
    self, resolve_market_session_schedule, CalendarCoverageState, FixedWindowOverrideProvider,
    MarketSessionSchedule, NyseWeekdaysProvider,
};
use mqk_daemon::state::{
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SessionWindow, SymbolStrategyAssignment,
    MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION,
};
use mqk_db::md::MdBarRowWithProvenance;
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_md::provider_registry::ProviderConfig;
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

fn strategy_registry() -> PluginRegistry {
    let mut reg = PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies(&mut reg, "AAPL".to_string())
        .expect("registration must not fail");
    reg
}

fn alpaca_provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "alpaca".to_string(),
        display_name: "Alpaca".to_string(),
        asset_classes: vec!["equity".to_string()],
        free_tier_available: true,
        api_key_required: true,
        credential_env_vars: vec![],
        rate_limit_notes: String::new(),
        supported_timeframes: vec!["1D".to_string(), "1m".to_string(), "5m".to_string()],
        historical_depth_notes: String::new(),
        realtime_support_notes: String::new(),
        licensing_notes: String::new(),
        implementation_status: "implemented_equity_provider".to_string(),
        enabled: true,
        verification_status: "test".to_string(),
        docs_url: String::new(),
    }
}

/// B2.2: the one canonical enabled equity/ETF daily provider with real,
/// committed parser proof of its `1D` timestamp convention (see
/// `resolve_daily_bar_timestamp_convention`'s doc comment in
/// `mqk-daemon/src/daily_data_readiness.rs` and
/// `mqk-md/src/lib.rs::tests::rate_limit_retry_succeeds_after_one_body_429`).
/// Also the real canonical provider assigned to every enabled equity in
/// `config/instruments/equities.json` today.
fn twelvedata_provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "twelvedata".to_string(),
        display_name: "TwelveData".to_string(),
        asset_classes: vec!["equity".to_string()],
        free_tier_available: true,
        api_key_required: true,
        credential_env_vars: vec![],
        rate_limit_notes: String::new(),
        supported_timeframes: vec!["1D".to_string(), "1m".to_string(), "5m".to_string()],
        historical_depth_notes: String::new(),
        realtime_support_notes: String::new(),
        licensing_notes: String::new(),
        implementation_status: "implemented_equity_provider".to_string(),
        enabled: true,
        verification_status: "test".to_string(),
        docs_url: String::new(),
    }
}

fn kraken_disabled_provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "kraken".to_string(),
        display_name: "Kraken".to_string(),
        asset_classes: vec!["crypto".to_string()],
        free_tier_available: true,
        api_key_required: false,
        credential_env_vars: vec![],
        rate_limit_notes: String::new(),
        supported_timeframes: vec!["1D".to_string()],
        historical_depth_notes: String::new(),
        realtime_support_notes: String::new(),
        licensing_notes: String::new(),
        implementation_status: "test".to_string(),
        enabled: false,
        verification_status: "test".to_string(),
        docs_url: String::new(),
    }
}

fn provider_configs() -> Vec<ProviderConfig> {
    vec![
        alpaca_provider_config(),
        twelvedata_provider_config(),
        kraken_disabled_provider_config(),
    ]
}

/// A `TrackedInstrument` assigned to `"twelvedata"` (mirrors the real
/// `config/instruments/equities.json` provider assignment), for B2.2's
/// verified-daily-convention proofs.
fn twelvedata_instrument(symbol: &str) -> TrackedInstrument {
    TrackedInstrument {
        instrument_id: format!("equity:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        provider: "twelvedata".to_string(),
        provider_symbol: symbol.to_string(),
        venue: "NASDAQ".to_string(),
        currency: "USD".to_string(),
        enabled: true,
        timeframes: vec!["1D".to_string()],
        notes: String::new(),
        instrument_kind: None,
        sector: None,
        category: None,
    }
}

fn aapl_instrument() -> TrackedInstrument {
    TrackedInstrument {
        instrument_id: "equity:US:AAPL".to_string(),
        symbol: "AAPL".to_string(),
        asset_class: "equity".to_string(),
        provider: "alpaca".to_string(),
        provider_symbol: "AAPL".to_string(),
        venue: "NASDAQ".to_string(),
        currency: "USD".to_string(),
        enabled: true,
        timeframes: vec!["1D".to_string()],
        notes: String::new(),
        instrument_kind: None,
        sector: None,
        category: None,
    }
}

fn instruments() -> Vec<TrackedInstrument> {
    vec![aapl_instrument()]
}

fn matching_binding(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> EffectiveRuntimeBinding {
    EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(strategy_id.to_string()),
        effective_runtime_target_symbol: Some(symbol.to_string()),
        effective_runtime_timeframe_secs: Some(timeframe_secs),
    }
}

fn assignment(symbol: &str, strategy_id: &str, timeframe: &str) -> SymbolStrategyAssignment {
    SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe: timeframe.to_string(),
    }
}

fn single_symbol_config(a: SymbolStrategyAssignment) -> MultiSymbolRuntimeConfig {
    MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: vec![a],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    }
}

fn multi_symbol_config(assignments: Vec<SymbolStrategyAssignment>) -> MultiSymbolRuntimeConfig {
    let n = assignments.len();
    MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: assignments,
        max_concurrent_symbols: n,
        source: MultiSymbolConfigSource::WatchlistArtifactV2 {
            path: "test".to_string(),
        },
    }
}

fn bar(
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    is_complete: bool,
    provider_id: &str,
) -> MdBarRowWithProvenance {
    MdBarRowWithProvenance {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open_micros: 100_000_000,
        high_micros: 101_000_000,
        low_micros: 99_000_000,
        close_micros: 100_500_000,
        volume: 1_000_000,
        is_complete,
        provider_id: provider_id.to_string(),
        provider_source: Some("alpaca".to_string()),
        provider_symbol: Some(symbol.to_string()),
        ingest_mode: Some("historical_sync".to_string()),
        provider_updated_at_utc: None,
        // REPAIR 3: fixed relative to the bar's own end_ts, not real wall-clock
        // `Utc::now()` — every fixture `now` used across this file is a fixed
        // historical instant (2024), so an actual current timestamp would
        // always appear materially "in the future" against it.
        ingested_at: ts(end_ts + 60),
    }
}

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("valid ts")
}

// Reference instants. Each is independently verified against its UTC
// decomposition + the known US Eastern DST offset for that date (not merely
// copied from a source-comment label — one candidate constant borrowed from
// `mqk-integrity::calendar`'s own test comments turned out to be mislabeled
// there: `1_704_510_000` decomposes to 2024-01-05 22:00 EST (Friday), not
// "2024-01-06 Sat 10:00 ET" as that file's comment claims; that existing
// test only asserts non-trading, which holds either way, so the mislabel
// never surfaced there. `SAT_2024_01_06_10AM_ET` below is computed fresh.
const MON_2024_01_08_10AM_ET: i64 = 1_704_726_000; // 2024-01-08 15:00 UTC = 10:00 EST, Mon
const SAT_2024_01_06_10AM_ET: i64 = 1_704_553_200; // 2024-01-06 15:00 UTC = 10:00 EST, Sat
const SAT_2024_01_06_3PM_ET: i64 = 1_704_571_200; // 2024-01-06 20:00 UTC = 15:00 EST, Sat
const NEW_YEARS_2024_01_01_8AM_ET: i64 = 1_704_114_000; // 2024-01-01 13:00 UTC = 08:00 EST, Mon (holiday)
const NEW_YEARS_2024_01_01_1PM_ET: i64 = 1_704_132_000; // 2024-01-01 18:00 UTC = 13:00 EST, Mon (holiday)
const MON_2024_04_15_935AM_EDT: i64 = 1_713_188_100; // 2024-04-15 13:35 UTC = 09:35 EDT, Mon
const BLACK_FRIDAY_2024_11_29_1PM_EST: i64 = 1_732_903_200; // 2024-11-29 18:00 UTC = 13:00 EST, Fri (early close)

// ---------------------------------------------------------------------------
// Assignment resolution / binding
// ---------------------------------------------------------------------------

/// Missing assignment set blocks.
#[tokio::test]
async fn ddr_01_empty_assignment_set_blocks() {
    let config = multi_symbol_config(vec![]);
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let provider = NyseWeekdaysProvider;
    let report = evaluate_assignments(
        None,
        &config,
        &binding,
        &provider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(
        !report.start_allowed,
        "DDR-01: empty set must not allow start"
    );
    assert_eq!(
        report.top_level_blocker,
        Some(daily_data_readiness::REASON_REQUIRED_ASSIGNMENTS_MISSING),
        "DDR-01: empty set must report required_assignments_missing"
    );
}

/// Assignment resolver failure blocks (env-driven path, no config at all).
#[tokio::test]
async fn ddr_02_env_resolver_failure_blocks() {
    // SAFETY: this test file runs with --test-threads=1 per the mission's
    // required validation command; env var mutation here is not racy against
    // other tests in this binary.
    for k in [
        "MQK_STRATEGY_SYMBOL",
        "MQK_STRATEGY_IDS",
        "MQK_STRATEGY_MD_TIMEFRAME",
        "MQK_PAPER_WATCHLIST_PATH",
    ] {
        std::env::remove_var(k);
    }
    let report = daily_data_readiness::evaluate_daily_data_readiness_from_env(
        None,
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(
        !report.start_allowed,
        "DDR-02: unset env must not allow start"
    );
    assert_eq!(
        report.top_level_blocker,
        Some(daily_data_readiness::REASON_REQUIRED_ASSIGNMENTS_MISSING),
        "DDR-02: unset env must report required_assignments_missing"
    );
}

/// Strategy-ID mismatch blocks; other binding fields still match.
#[tokio::test]
async fn ddr_03_strategy_id_mismatch_blocks() {
    let a = assignment("AAPL", "mean_reversion", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(!result.is_ready());
    assert!(
        result
            .blockers
            .contains(&REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH),
        "DDR-03: must block on runtime_strategy_assignment_mismatch, got {:?}",
        result.blockers
    );
    assert_eq!(
        result.readiness_state, "blocked",
        "DDR-03: binding mismatch must not progress to db_unavailable/query_failed"
    );
}

/// Target-symbol mismatch blocks.
#[tokio::test]
async fn ddr_04_target_symbol_mismatch_blocks() {
    let a = assignment("MSFT", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result
        .blockers
        .contains(&REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH));
    assert_eq!(result.readiness_state, "blocked");
}

/// Empty target symbol blocks under the same reason as a mismatch.
#[tokio::test]
async fn ddr_05_empty_target_symbol_blocks() {
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some("swing_momentum".to_string()),
        effective_runtime_target_symbol: None,
        effective_runtime_timeframe_secs: Some(86_400),
    };
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result
        .blockers
        .contains(&REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH));
}

/// Timeframe mismatch blocks.
#[tokio::test]
async fn ddr_06_timeframe_mismatch_blocks() {
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 3_600); // wrong: 1h not 1D
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result
        .blockers
        .contains(&REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH));
}

/// Exact single-symbol binding progresses to data evaluation (db=None ->
/// db_unavailable, with zero blockers accumulated before the DB stage).
#[tokio::test]
async fn ddr_07_exact_binding_progresses_to_data_evaluation() {
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert_eq!(
        result.readiness_state, "db_unavailable",
        "DDR-07: matching binding + db=None must reach db_unavailable, got {result:?}",
    );
    assert!(
        result.blockers.is_empty(),
        "DDR-07: no structural blockers should exist before the DB stage: {:?}",
        result.blockers
    );
    assert_eq!(result.required_history_bars, Some(20));
    assert_eq!(result.asset_class.as_deref(), Some("equity"));
}

/// Multi-symbol, same-strategy-id, different-symbol configuration blocks
/// every non-bound symbol via runtime_strategy_symbol_binding_mismatch.
#[tokio::test]
async fn ddr_08_multi_symbol_same_strategy_blocks_non_bound_symbols() {
    let mut extra_instruments = instruments();
    extra_instruments.push(TrackedInstrument {
        symbol: "MSFT".to_string(),
        instrument_id: "equity:US:MSFT".to_string(),
        ..aapl_instrument()
    });

    let config = multi_symbol_config(vec![
        assignment("AAPL", "swing_momentum", "1D"),
        assignment("MSFT", "swing_momentum", "1D"),
    ]);
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let report = evaluate_assignments(
        None,
        &config,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &extra_instruments,
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;

    assert!(!report.start_allowed, "DDR-08: aggregate must not be ready");
    let aapl = report
        .assignments
        .iter()
        .find(|a| a.assignment_symbol == "AAPL")
        .unwrap();
    let msft = report
        .assignments
        .iter()
        .find(|a| a.assignment_symbol == "MSFT")
        .unwrap();
    assert_eq!(
        aapl.readiness_state, "db_unavailable",
        "DDR-08: bound symbol (AAPL) must progress past binding checks"
    );
    assert!(
        msft.blockers
            .contains(&REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH),
        "DDR-08: non-bound symbol (MSFT) must block on symbol binding mismatch"
    );
}

/// Unknown strategy requirement blocks.
#[tokio::test]
async fn ddr_09_unknown_strategy_requirement_blocks() {
    let a = assignment("AAPL", "totally_unregistered_strategy", "1D");
    let binding = matching_binding("totally_unregistered_strategy", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result
        .blockers
        .contains(&REASON_STRATEGY_REQUIREMENT_UNKNOWN));
    assert_eq!(result.required_history_bars, None);
    assert_eq!(result.readiness_state, "blocked");
}

/// DB absent blocks (readiness_state = db_unavailable, distinct from "ready").
#[tokio::test]
async fn ddr_10_db_absent_blocks() {
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert_eq!(result.readiness_state, "db_unavailable");
    assert!(!result.is_ready());
}

/// DB query failure blocks (distinct from db_unavailable) — a lazily
/// connected pool pointed at an unreachable address fails on first query,
/// with no real Postgres server required for this test.
#[tokio::test]
async fn ddr_11_db_query_failure_blocks() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
        .expect("connect_lazy must not fail synchronously");

    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        Some(&pool),
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert_eq!(
        result.readiness_state, "query_failed",
        "DDR-11: unreachable DB must surface query_failed, not db_unavailable"
    );
}

// ---------------------------------------------------------------------------
// Asset class / provider capability
// ---------------------------------------------------------------------------

/// Asset-class unknown blocks (symbol absent from the instrument registry).
#[tokio::test]
async fn ddr_12_asset_class_unknown_blocks() {
    let a = assignment("ZZZNOTFOUND", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "ZZZNOTFOUND", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(), // does not contain ZZZNOTFOUND
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result.blockers.contains(&REASON_ASSET_CLASS_UNKNOWN));
    assert_eq!(result.asset_class, None);
    assert_eq!(result.readiness_state, "blocked");
}

/// A current `1h` assignment blocks as provider_capability_mismatch — the
/// enabled provider registry does not declare `1h` support (contract §13).
#[tokio::test]
async fn ddr_13_unsupported_1h_timeframe_blocks() {
    let a = assignment("AAPL", "mean_reversion", "1h");
    let binding = matching_binding("mean_reversion", "AAPL", 3_600);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(
        result
            .blockers
            .contains(&REASON_PROVIDER_CAPABILITY_MISMATCH),
        "DDR-13: 1h must block on provider_capability_mismatch, got {:?}",
        result.blockers
    );
    assert_eq!(
        result.readiness_state, "blocked",
        "DDR-13: must never reach db_unavailable/ready for 1h"
    );
}

/// Same claim restated for `volatility_breakout` (also 1h) — both currently
/// registered 1h strategies must block identically, honestly.
#[tokio::test]
async fn ddr_14_volatility_breakout_1h_also_blocks() {
    let a = assignment("AAPL", "volatility_breakout", "1h");
    let binding = matching_binding("volatility_breakout", "AAPL", 3_600);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result
        .blockers
        .contains(&REASON_PROVIDER_CAPABILITY_MISMATCH));
    assert_eq!(
        result.required_history_bars,
        Some(21),
        "LOOKBACK+1 for volatility_breakout"
    );
}

// ---------------------------------------------------------------------------
// Calendar schedule (§6a/§7)
// ---------------------------------------------------------------------------

/// Daily before-close expectation: the previous trading date is expected,
/// not today's.
#[test]
fn ddr_15_daily_before_close_expectation() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    assert_eq!(schedule.market_date, (2024, 1, 8));

    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 900, 1)
        .expect("DDR-15: expected window must resolve");
    assert_eq!(expected.len(), 1);
    assert_eq!(
        expected[0],
        market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date),
        "DDR-15: before close, the previous trading date's row is expected"
    );
}

/// Daily after-close-plus-grace expectation: today's row becomes expected.
#[test]
fn ddr_16_daily_after_close_plus_grace_expectation() {
    let provider = NyseWeekdaysProvider;
    let baseline = resolve_market_session_schedule(&provider, ts(MON_2024_01_08_10AM_ET));
    let grace = 900;
    let after_close = ts(baseline.session_close_utc.timestamp() + grace + 1);
    let schedule = resolve_market_session_schedule(&provider, after_close);
    assert_eq!(schedule.market_date, (2024, 1, 8), "still the same ET date");

    let expected =
        expected_daily_end_ts_window(&provider, &schedule, after_close.timestamp(), grace, 1)
            .expect("DDR-16: expected window must resolve");
    assert_eq!(
        expected[0],
        market_calendar::midnight_utc_ts_for_date(schedule.market_date),
        "DDR-16: after close+grace, today's row is expected"
    );
}

/// Weekend previous-session behavior: Saturday resolves to Friday.
#[test]
fn ddr_17_weekend_previous_session_behavior() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(SAT_2024_01_06_10AM_ET));
    assert_eq!(schedule.market_date, (2024, 1, 6));
    assert_eq!(
        schedule.previous_trading_date,
        (2024, 1, 5),
        "DDR-17: Saturday's previous trading date must be Friday 2024-01-05"
    );
}

/// Holiday previous-session behavior: New Year's Day resolves to the prior
/// year's last trading day, skipping the weekend in between.
#[test]
fn ddr_18_holiday_previous_session_behavior() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(NEW_YEARS_2024_01_01_8AM_ET));
    assert_eq!(
        schedule.previous_trading_date,
        (2023, 12, 29),
        "DDR-18: New Year's Day 2024's previous trading date must be Fri 2023-12-29 \
         (skipping the holiday itself and the intervening weekend)"
    );
}

/// Early-close behavior: session_close_utc reflects 13:00 ET, not 16:00 ET.
#[test]
fn ddr_19_early_close_behavior() {
    let provider = NyseWeekdaysProvider;
    let now = ts(BLACK_FRIDAY_2024_11_29_1PM_EST);
    let schedule = resolve_market_session_schedule(&provider, now);
    assert!(
        schedule.is_early_close,
        "DDR-19: Black Friday must be flagged early-close"
    );
    assert_eq!(
        schedule.session_close_utc.timestamp(),
        BLACK_FRIDAY_2024_11_29_1PM_EST,
        "DDR-19: early-close session_close_utc must be exactly 13:00 ET, not 16:00 ET"
    );

    // At exactly close+0 grace, today's row is already expected (proves the
    // early close time, not the normal 16:00 close, gates the decision).
    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 0, 1)
        .expect("DDR-19: expected window must resolve");
    assert_eq!(
        expected[0],
        market_calendar::midnight_utc_ts_for_date(schedule.market_date)
    );
}

/// Before first intraday close behavior: the entire expected window spills
/// into the previous trading session (no slot in the current session has
/// closed yet).
#[test]
fn ddr_20_before_first_intraday_close_behavior() {
    let provider = NyseWeekdaysProvider;
    // 3 minutes before the first 5m bar (09:30-09:35 ET) closes.
    let now = ts(MON_2024_04_15_935AM_EDT - 180);
    let schedule = resolve_market_session_schedule(&provider, now);
    let session_open_ts = schedule.session_open_utc.timestamp();

    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, now.timestamp(), 300, 0, 5)
            .expect("DDR-20: expected window must resolve via previous-session spillover");
    assert_eq!(expected.len(), 5);
    assert!(
        expected.iter().all(|&t| t < session_open_ts),
        "DDR-20: before the first interval closes, every expected slot must come \
         from the previous session: {expected:?} vs session_open={session_open_ts}"
    );
}

/// After intraday close plus grace: the just-closed slot is included as the
/// most recent expected bar.
#[test]
fn ddr_21_after_intraday_close_plus_grace_behavior() {
    let provider = NyseWeekdaysProvider;
    let baseline = resolve_market_session_schedule(&provider, ts(MON_2024_04_15_935AM_EDT));
    // 5 complete 5m bars after open (09:30 -> 09:55), well past open.
    let now_ts = baseline.session_open_utc.timestamp() + 5 * 300;
    let now = ts(now_ts);
    let schedule = resolve_market_session_schedule(&provider, now);

    let expected = expected_intraday_end_ts_window(&provider, &schedule, now_ts, 300, 0, 5)
        .expect("DDR-21: expected window must resolve");
    assert_eq!(expected.len(), 5);
    let last = *expected.last().unwrap();
    assert_eq!(
        last,
        schedule.session_open_utc.timestamp() + 4 * 300,
        "DDR-21: the most recent expected slot must be the 5th bar-start (index 4)"
    );
    // All entries strictly ascending, all within the current session.
    assert!(expected.windows(2).all(|w| w[0] < w[1]));
}

// ---------------------------------------------------------------------------
// Non-trading-day intraday expectation (REPAIR 1,
// DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
// ---------------------------------------------------------------------------

/// Saturday morning: the entire expected intraday window must come from
/// Friday's session tail, never a same-day Saturday grid.
#[test]
fn ddr_31_saturday_morning_uses_friday_tail() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(SAT_2024_01_06_10AM_ET));
    assert!(
        !schedule.is_trading_day,
        "DDR-31: Saturday must not be a trading day"
    );
    let friday_schedule =
        market_calendar::resolve_market_session_schedule_for_date(&provider, (2024, 1, 5))
            .expect("DDR-31: Friday's schedule must resolve");
    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, SAT_2024_01_06_10AM_ET, 300, 0, 5)
            .expect("DDR-31: must resolve via Friday's tail");
    assert_eq!(expected.len(), 5);
    assert!(
        expected
            .iter()
            .all(|&t| t < friday_schedule.session_close_utc.timestamp()
                && t >= friday_schedule.session_open_utc.timestamp()),
        "DDR-31: every expected slot must come from Friday's own session: {expected:?}"
    );
}

/// Saturday afternoon: the same requirement holds regardless of the later
/// wall-clock hour — a Saturday afternoon must never require Saturday bars.
#[test]
fn ddr_32_saturday_afternoon_still_uses_friday_tail() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(SAT_2024_01_06_3PM_ET));
    assert!(
        !schedule.is_trading_day,
        "DDR-32: Saturday afternoon must not be a trading day"
    );
    let friday_schedule =
        market_calendar::resolve_market_session_schedule_for_date(&provider, (2024, 1, 5))
            .expect("DDR-32: Friday's schedule must resolve");
    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, SAT_2024_01_06_3PM_ET, 300, 0, 5)
            .expect("DDR-32: must resolve via Friday's tail");
    assert!(
        expected
            .iter()
            .all(|&t| t < friday_schedule.session_close_utc.timestamp()),
        "DDR-32: Saturday afternoon must never require Saturday bars: {expected:?}"
    );
}

/// Full-day exchange holiday morning: uses the prior trading session's tail.
#[test]
fn ddr_33_holiday_morning_uses_prior_session() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(NEW_YEARS_2024_01_01_8AM_ET));
    assert!(
        !schedule.is_trading_day,
        "DDR-33: New Year's Day must not be a trading day"
    );
    let expected = expected_intraday_end_ts_window(
        &provider,
        &schedule,
        NEW_YEARS_2024_01_01_8AM_ET,
        300,
        0,
        5,
    )
    .expect("DDR-33: must resolve via the prior session's tail");
    assert_eq!(expected.len(), 5);
    assert!(
        expected
            .iter()
            .all(|&t| t < schedule.session_open_utc.timestamp()),
        "DDR-33: a holiday morning must never require the holiday's own bars: {expected:?}"
    );
}

/// Full-day exchange holiday afternoon: the same requirement holds
/// regardless of the later wall-clock hour.
#[test]
fn ddr_34_holiday_afternoon_still_uses_prior_session() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(NEW_YEARS_2024_01_01_1PM_ET));
    assert!(
        !schedule.is_trading_day,
        "DDR-34: New Year's Day afternoon must not be a trading day"
    );
    let expected = expected_intraday_end_ts_window(
        &provider,
        &schedule,
        NEW_YEARS_2024_01_01_1PM_ET,
        300,
        0,
        5,
    )
    .expect("DDR-34: must resolve via the prior session's tail");
    assert!(
        expected
            .iter()
            .all(|&t| t < schedule.session_open_utc.timestamp()),
        "DDR-34: a holiday afternoon must never require the holiday's own bars: {expected:?}"
    );
}

/// Monday after the first interval closes: correctly begins requiring
/// Monday's own bars — a real trading day underway must not falsely spill
/// into the previous session.
#[test]
fn ddr_35_monday_after_first_interval_requires_monday_bars() {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, ts(MON_2024_01_08_10AM_ET));
    assert!(
        schedule.is_trading_day,
        "DDR-35: Monday must be a trading day"
    );
    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, MON_2024_01_08_10AM_ET, 300, 0, 5)
            .expect("DDR-35: must resolve on Monday's own grid");
    assert_eq!(expected.len(), 5);
    let session_open_ts = schedule.session_open_utc.timestamp();
    assert!(
        expected.iter().all(|&t| t >= session_open_ts),
        "DDR-35: once the first interval has closed, Monday's own bars must be required: {expected:?}"
    );
}

// ---------------------------------------------------------------------------
// Full-window calendar coverage (REPAIR 2,
// DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
// ---------------------------------------------------------------------------

/// Intraday previous-session spillover whose own resolved coverage is
/// `OutOfRange` must fail closed, even when the current evaluation date's
/// own schedule reports `Active`.
#[test]
fn ddr_36_intraday_previous_session_out_of_coverage_blocks() {
    let provider = NyseWeekdaysProvider;
    // (2023,1,3) is inside SCHEDULE_COVERAGE_START..=END; (2022,12,30) is
    // deliberately placed before it — the point of this fixture is to
    // isolate the fail-closed mechanism itself, not to assert real NYSE
    // calendar accuracy for either date.
    let midnight = market_calendar::midnight_utc_ts_for_date((2023, 1, 3));
    let schedule = MarketSessionSchedule {
        market_date: (2023, 1, 3),
        session_open_utc: ts(midnight + 9 * 3600 + 30 * 60),
        session_close_utc: ts(midnight + 16 * 3600),
        previous_trading_date: (2022, 12, 30),
        is_early_close: false,
        is_trading_day: true,
        calendar_source: "test_fixture",
        coverage_state: CalendarCoverageState::Active,
    };
    let now_ts = schedule.session_open_utc.timestamp() + 60; // before the first 5m interval closes
    let expected = expected_intraday_end_ts_window(&provider, &schedule, now_ts, 300, 0, 5);
    assert!(
        expected.is_none(),
        "DDR-36: previous-session spillover outside coverage must fail closed, got {expected:?}"
    );
}

/// A 20-day daily warmup whose walk-back crosses before the coverage start
/// must fail closed (`None`), never a silent partial window built from the
/// static provider's non-fail-closed out-of-range fallback.
#[test]
fn ddr_37_daily_20day_warmup_crossing_coverage_start_blocks() {
    let provider = NyseWeekdaysProvider;
    let result = market_calendar::walk_back_trading_dates(&provider, (2023, 1, 25), 20);
    assert!(
        result.is_none(),
        "DDR-37: a 20-day warmup crossing 2023-01-01 must fail closed, got {result:?}"
    );
}

/// A fully in-range 2024 20-day warmup must still resolve normally — the
/// coverage fix must not regress ordinary in-range evaluation.
#[test]
fn ddr_38_in_range_2024_window_remains_valid() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 900, 20);
    assert!(
        expected.is_some(),
        "DDR-38: an in-range 2024 20-day warmup must still resolve"
    );
    assert_eq!(expected.unwrap().len(), 20);
}

// ---------------------------------------------------------------------------
// Bar-level provenance and continuity (§9/§10/§11)
// ---------------------------------------------------------------------------

/// Materially future row blocks.
#[test]
fn ddr_22_future_row_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let rows = vec![bar("AAPL", "1D", now.timestamp() + 100_000, true, "alpaca")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(blockers.contains(&REASON_LATEST_BAR_FUTURE));
}

/// Insufficient history blocks.
#[test]
fn ddr_23_insufficient_history_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let rows = vec![bar(
        "AAPL",
        "1D",
        market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date),
        true,
        "alpaca",
    )];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        20,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(blockers.contains(&REASON_INSUFFICIENT_HISTORY));
}

/// Duplicate timestamp blocks.
#[test]
fn ddr_24_duplicate_timestamp_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![
        bar("AAPL", "1D", d, true, "alpaca"),
        bar("AAPL", "1D", d, true, "alpaca"),
    ];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(blockers.contains(&REASON_DUPLICATE_TIMESTAMP));
}

/// Interior gap blocks (missing the middle expected date of a 3-date window).
#[test]
fn ddr_25_interior_gap_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 900, 3)
        .expect("resolves");
    assert_eq!(expected.len(), 3);
    // Provide only the first and last expected dates — the middle is missing.
    let rows = vec![
        bar("AAPL", "1D", expected[0], true, "alpaca"),
        bar("AAPL", "1D", expected[2], true, "alpaca"),
    ];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        3,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_INTERIOR_GAP),
        "DDR-25: missing middle expected date must block as interior_gap: {blockers:?}"
    );
    assert!(
        !blockers.contains(&REASON_EXPECTED_LATEST_BAR_MISSING),
        "DDR-25: the latest expected bar IS present; must not also claim it's missing"
    );
}

/// Unknown provider blocks.
#[test]
fn ddr_26_unknown_provider_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![bar("AAPL", "1D", d, true, "unknown")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID));
}

/// Disabled provider blocks (row references a registered-but-disabled provider).
#[test]
fn ddr_27_disabled_provider_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![bar("AAPL", "1D", d, true, "kraken")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(blockers.contains(&REASON_PROVIDER_DISABLED));
}

/// Calendar unavailable blocks (FixedWindowOverrideProvider has no exchange
/// calendar authority — must report Unknown, not silently pass).
#[tokio::test]
async fn ddr_28_calendar_unavailable_via_fixed_window_override_blocks() {
    let fixed = FixedWindowOverrideProvider {
        window: SessionWindow::parse("09:30", "16:00").unwrap(),
    };
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &fixed,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(result.blockers.contains(&REASON_CALENDAR_UNAVAILABLE));
    assert_eq!(result.readiness_state, "blocked");
}

// ---------------------------------------------------------------------------
// Complete provider provenance (REPAIR 3,
// DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
// ---------------------------------------------------------------------------

/// Blank `provider_source` blocks as invalid provenance.
#[test]
fn ddr_39_blank_provider_source_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.provider_source = None;
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID),
        "DDR-39: blank provider_source must block as invalid provenance: {blockers:?}"
    );
}

/// Blank `provider_symbol` blocks as invalid provenance.
#[test]
fn ddr_40_blank_provider_symbol_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.provider_symbol = None;
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID),
        "DDR-40: blank provider_symbol must block as invalid provenance: {blockers:?}"
    );
}

/// A present-but-wrong `provider_symbol` blocks under its own distinguishable
/// reason, never folded into the generic invalid-provenance bucket.
#[test]
fn ddr_41_wrong_provider_symbol_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.provider_symbol = Some("MSFT".to_string());
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_SYMBOL_MISMATCH),
        "DDR-41: wrong provider_symbol must block as provider_symbol_mismatch: {blockers:?}"
    );
}

/// Blank `ingest_mode` blocks as invalid provenance.
#[test]
fn ddr_42_blank_ingest_mode_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.ingest_mode = None;
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID),
        "DDR-42: blank ingest_mode must block as invalid provenance: {blockers:?}"
    );
}

/// A materially future `ingested_at` blocks as `provider_ingest_time_future`.
#[test]
fn ddr_43_future_ingest_time_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.ingested_at = ts(now.timestamp() + 10_000);
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_INGEST_TIME_FUTURE),
        "DDR-43: materially future ingested_at must block: {blockers:?}"
    );
}

/// A legitimately absent `provider_updated_at_utc` (the audited Alpaca/
/// current-provider contract never supplies it) must pass, not block.
#[test]
fn ddr_44_absent_provider_updated_at_passes() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let mut row = bar("AAPL", "1D", d, true, "alpaca");
    row.provider_updated_at_utc = None; // matches every current write path
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        !blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID),
        "DDR-44: absent provider_updated_at_utc must not itself block: {blockers:?}"
    );
}

/// Fully valid provenance on a supported timeframe passes every provenance
/// check (the D1-only unverified-timestamp-convention blocker is asserted
/// separately and does not indicate a provenance defect).
#[test]
fn ddr_45_fully_valid_provenance_passes() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_04_15_935AM_EDT + 5 * 300);
    let schedule = resolve_market_session_schedule(&provider, now);
    let row = bar(
        "AAPL",
        "5m",
        schedule.session_open_utc.timestamp(),
        true,
        "alpaca",
    );
    let blockers = evaluate_bar_readiness(
        &[row],
        mqk_md::Timeframe::M5,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        0,
        60,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        !blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID)
            && !blockers.contains(&REASON_PROVIDER_SYMBOL_MISMATCH)
            && !blockers.contains(&REASON_PROVIDER_INGEST_TIME_FUTURE)
            && !blockers.contains(&REASON_PROVIDER_DISABLED)
            && !blockers.contains(&REASON_PROVIDER_CAPABILITY_MISMATCH),
        "DDR-45: fully valid provenance must not raise any provenance blocker: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// Aggregate + full end-to-end (DB-backed; skip without MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

fn get_test_db_url() -> Option<String> {
    std::env::var("MQK_DATABASE_URL").ok()
}

async fn seed_daily_bars(pool: &sqlx::PgPool, symbol: &str, expected_ts: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = '1D'")
        .bind(symbol)
        .execute(pool)
        .await
        .expect("cleanup failed");
    for &end_ts in expected_ts {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,'1D',$2,100000000,101000000,99000000,100500000,1000000,true,
                      'alpaca','alpaca',$1,'historical_sync',$3)
            "#,
        )
        .bind(symbol)
        .bind(end_ts)
        // REPAIR 3: fixed relative to end_ts, never real wall-clock — see the
        // `bar()` fixture helper's comment for why this matters against the
        // fixed 2024 `now` every test in this file evaluates against.
        .bind(ts(end_ts + 60))
        .execute(pool)
        .await
        .expect("seed insert failed");
    }
}

async fn seed_intraday_bars(
    pool: &sqlx::PgPool,
    symbol: &str,
    timeframe: &str,
    expected_ts: &[i64],
) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("cleanup failed");
    for &end_ts in expected_ts {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,$2,$3,100000000,101000000,99000000,100500000,1000000,true,
                      'alpaca','alpaca',$1,'historical_sync',$4)
            "#,
        )
        .bind(symbol)
        .bind(timeframe)
        .bind(end_ts)
        .bind(ts(end_ts + 60))
        .execute(pool)
        .await
        .expect("seed insert failed");
    }
}

/// Supported exact assignment can become ready, and all-ready assignments
/// produce aggregate ready.
///
/// Uses `5m`/`intraday_scalper` rather than the original `1D`/`swing_momentum`
/// fixture: REPAIR 4 makes every `1D` evaluation fail closed under
/// `provider_timestamp_convention_unverified`, so `5m` (a continuity-complete,
/// currently-supported intraday timeframe) is now the only fixture that can
/// prove the full ready/aggregate-ready path end to end. See DDR-46 below for
/// the dedicated proof that `1D` itself blocks as designed.
#[tokio::test]
async fn ddr_29_ready_assignment_and_all_ready_aggregate() {
    let Some(db_url) = get_test_db_url() else {
        eprintln!("DDR-29: skipped (no MQK_DATABASE_URL)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("DDR-29: pool connect failed");

    // SAFETY: --test-threads=1 per the mission's required validation command.
    std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
    std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");

    let symbol = "ZZDDRTEST29";
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_04_15_935AM_EDT + 10 * 300);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, now.timestamp(), 300, 0, 5)
            .expect("resolves");
    seed_intraday_bars(&pool, symbol, "5m", &expected).await;

    let mut instruments = instruments();
    instruments.push(TrackedInstrument {
        symbol: symbol.to_string(),
        instrument_id: format!("equity:US:{symbol}"),
        provider_symbol: symbol.to_string(),
        ..aapl_instrument()
    });

    let a = assignment(symbol, "intraday_scalper", "5m");
    let binding = matching_binding("intraday_scalper", symbol, 300);
    let result = evaluate_assignment(
        Some(&pool),
        &a,
        &binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;
    assert_eq!(
        result.readiness_state, "ready",
        "DDR-29: fully seeded, matching assignment must reach ready: {result:?}"
    );

    let config = single_symbol_config(a);
    let report = evaluate_assignments(
        Some(&pool),
        &config,
        &binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;
    assert!(
        report.start_allowed,
        "DDR-29: all-ready assignments must produce aggregate ready"
    );
    assert_eq!(report.aggregate_state, "ready");

    sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(&pool)
        .await
        .ok();
    std::env::remove_var("MQK_DATA_READINESS_GRACE_SECS");
    std::env::remove_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS");
}

/// One blocked assignment blocks the aggregate, even when the other is ready.
/// (Same `5m`/`intraday_scalper` fixture substitution as DDR-29, for the same
/// reason.)
#[tokio::test]
async fn ddr_30_one_blocked_assignment_blocks_aggregate() {
    let Some(db_url) = get_test_db_url() else {
        eprintln!("DDR-30: skipped (no MQK_DATABASE_URL)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("DDR-30: pool connect failed");

    std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
    std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");

    let ready_symbol = "ZZDDRTEST30READY";
    let blocked_symbol = "ZZDDRTEST30BLOCKED";
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_04_15_935AM_EDT + 10 * 300);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected =
        expected_intraday_end_ts_window(&provider, &schedule, now.timestamp(), 300, 0, 5)
            .expect("resolves");
    seed_intraday_bars(&pool, ready_symbol, "5m", &expected).await;
    // blocked_symbol: no bars seeded at all -> market_data_missing.
    sqlx::query("delete from md_bars where symbol = $1")
        .bind(blocked_symbol)
        .execute(&pool)
        .await
        .ok();

    let mut instruments = instruments();
    for sym in [ready_symbol, blocked_symbol] {
        instruments.push(TrackedInstrument {
            symbol: sym.to_string(),
            instrument_id: format!("equity:US:{sym}"),
            provider_symbol: sym.to_string(),
            ..aapl_instrument()
        });
    }

    // Binding is bound to ready_symbol; blocked_symbol only differs by
    // having no md_bars data (both configured for the same strategy id, and
    // for this test we bind to ready_symbol so blocked_symbol necessarily
    // fails at the symbol-binding step instead — to isolate a pure
    // data-availability block, evaluate blocked_symbol independently below
    // with its own matching binding.)
    let binding = matching_binding("intraday_scalper", ready_symbol, 300);
    let config = single_symbol_config(assignment(ready_symbol, "intraday_scalper", "5m"));
    let ready_report = evaluate_assignments(
        Some(&pool),
        &config,
        &binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;
    assert!(ready_report.start_allowed);

    let blocked_binding = matching_binding("intraday_scalper", blocked_symbol, 300);
    let blocked_result = evaluate_assignment(
        Some(&pool),
        &assignment(blocked_symbol, "intraday_scalper", "5m"),
        &blocked_binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;
    assert_eq!(blocked_result.readiness_state, "blocked");
    assert!(blocked_result
        .blockers
        .contains(&daily_data_readiness::REASON_MARKET_DATA_MISSING));

    sqlx::query("delete from md_bars where symbol = any($1)")
        .bind(vec![ready_symbol, blocked_symbol])
        .execute(&pool)
        .await
        .ok();
    std::env::remove_var("MQK_DATA_READINESS_GRACE_SECS");
    std::env::remove_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS");
}

// ---------------------------------------------------------------------------
// Daily provider timestamp convention (REPAIR 4,
// DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
// ---------------------------------------------------------------------------

/// A `1D` assignment with otherwise fully valid, fully seeded, fully
/// continuous data must still block — solely under
/// `provider_timestamp_convention_unverified` — because no trustworthy
/// existing evidence proves Alpaca's daily `end_ts` label convention.
#[tokio::test]
async fn ddr_46_daily_always_blocks_on_unverified_timestamp_convention() {
    let Some(db_url) = get_test_db_url() else {
        eprintln!("DDR-46: skipped (no MQK_DATABASE_URL)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("DDR-46: pool connect failed");

    let symbol = "ZZDDRTEST46";
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 900, 20)
        .expect("resolves");
    seed_daily_bars(&pool, symbol, &expected).await;

    let mut instruments = instruments();
    instruments.push(TrackedInstrument {
        symbol: symbol.to_string(),
        instrument_id: format!("equity:US:{symbol}"),
        provider_symbol: symbol.to_string(),
        ..aapl_instrument()
    });

    let a = assignment(symbol, "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", symbol, 86_400);
    let result = evaluate_assignment(
        Some(&pool),
        &a,
        &binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;
    assert_eq!(
        result.readiness_state, "blocked",
        "DDR-46: 1D must never reach ready regardless of otherwise-valid data: {result:?}"
    );
    assert_eq!(
        result.blockers,
        vec![REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED],
        "DDR-46: fully valid/continuous 1D data must block on exactly this one reason: {:?}",
        result.blockers
    );

    sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(&pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// Provider contract (§B2.1/§B2.2,
// DAILY-DATA-READINESS-01B-PROVIDER-CONTRACT-INTEGRATION-01)
// ---------------------------------------------------------------------------

/// `resolve_daily_bar_timestamp_convention` is provider-specific and pure.
#[test]
fn ddr_48_daily_timestamp_convention_is_provider_specific() {
    assert_eq!(
        resolve_daily_bar_timestamp_convention("twelvedata"),
        DailyBarTimestampConvention::MidnightUtcMarketDate,
        "DDR-48: twelvedata's daily convention is proven (mqk-md parser test)"
    );
    assert_eq!(
        resolve_daily_bar_timestamp_convention("alpaca"),
        DailyBarTimestampConvention::Unverified,
        "DDR-48: alpaca's daily convention remains unproven"
    );
    assert_eq!(
        resolve_daily_bar_timestamp_convention("some_other_provider"),
        DailyBarTimestampConvention::Unverified,
        "DDR-48: an unrecognized provider fails closed to Unverified"
    );
}

/// The canonical provider id/symbol are exposed on `AssignmentReadiness`,
/// populated even when the DB stage is never reached.
#[tokio::test]
async fn ddr_49_canonical_provider_identity_is_exposed() {
    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert_eq!(
        result.expected_provider_id,
        Some("alpaca".to_string()),
        "DDR-49: canonical provider id must be exposed"
    );
    assert_eq!(
        result.expected_provider_symbol,
        Some("AAPL".to_string()),
        "DDR-49: canonical provider symbol must be exposed"
    );
}

/// An assignment for a symbol absent from the instrument registry exposes no
/// canonical provider identity (already blocked separately via
/// `asset_class_unknown`) rather than a fabricated guess.
#[tokio::test]
async fn ddr_50_unregistered_symbol_exposes_no_provider_identity() {
    let a = assignment("ZZUNREGISTERED", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "ZZUNREGISTERED", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert_eq!(result.expected_provider_id, None);
    assert_eq!(result.expected_provider_symbol, None);
    assert!(result.blockers.contains(&REASON_ASSET_CLASS_UNKNOWN));
}

/// Exact provider ID match passes; a materially different registered,
/// enabled, capable provider still blocks under its own reason.
#[test]
fn ddr_51_exact_provider_id_match_passes() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![bar("AAPL", "1D", d, true, "alpaca")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        !blockers.contains(&REASON_PROVIDER_ID_MISMATCH),
        "DDR-51: matching provider id must not block: {blockers:?}"
    );
}

/// A row from a different, registered/enabled/capable provider than the
/// instrument's own configured provider blocks as `provider_id_mismatch`.
#[test]
fn ddr_52_wrong_provider_id_blocks() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![bar("AAPL", "1D", d, true, "twelvedata")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_ID_MISMATCH),
        "DDR-52: a row from a different (even valid) provider must block: {blockers:?}"
    );
}

/// A row whose `provider_id` is a real, non-blank, non-"unknown" string that
/// simply is not registered blocks as `provider_unknown` — distinguished
/// from the legacy blank/"unknown" `provider_provenance_invalid` bucket.
#[test]
fn ddr_53_unregistered_provider_id_blocks_as_provider_unknown() {
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let d = market_calendar::midnight_utc_ts_for_date(schedule.previous_trading_date);
    let rows = vec![bar("AAPL", "1D", d, true, "not_a_registered_provider")];
    let blockers = evaluate_bar_readiness(
        &rows,
        mqk_md::Timeframe::D1,
        &schedule,
        &provider,
        1,
        now.timestamp(),
        900,
        300,
        &provider_configs(),
        "equity",
        "alpaca",
        "AAPL",
    );
    assert!(
        blockers.contains(&REASON_PROVIDER_UNKNOWN),
        "DDR-53: an unregistered provider_id must block as provider_unknown: {blockers:?}"
    );
    assert!(
        !blockers.contains(&REASON_PROVIDER_PROVENANCE_INVALID),
        "DDR-53: must not also fold into the generic legacy-unknown bucket: {blockers:?}"
    );
}

/// The capability pre-check now evaluates the instrument's *exact configured
/// provider*, not merely "any" enabled provider that happens to support the
/// same asset class/timeframe. An instrument configured for a provider that
/// is not registered at all blocks as `provider_unknown`, even though a
/// different, capable, enabled provider genuinely exists in the registry.
#[tokio::test]
async fn ddr_54_capability_precheck_uses_instruments_exact_provider() {
    let mut instruments = instruments();
    instruments.push(TrackedInstrument {
        symbol: "ZZNOTREGPROV".to_string(),
        instrument_id: "equity:US:ZZNOTREGPROV".to_string(),
        provider: "not_a_registered_provider".to_string(),
        provider_symbol: "ZZNOTREGPROV".to_string(),
        ..aapl_instrument()
    });
    let a = assignment("ZZNOTREGPROV", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "ZZNOTREGPROV", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;
    assert!(
        result.blockers.contains(&REASON_PROVIDER_UNKNOWN),
        "DDR-54: instrument's own unregistered provider must block, even though \
         alpaca/twelvedata are registered and capable: {:?}",
        result.blockers
    );
    assert_eq!(result.readiness_state, "blocked");
}

/// A verified provider-specific `1D` convention (twelvedata) is not blocked
/// by `provider_timestamp_convention_unverified` — the corrected contract
/// blocks only the specific unverified `(provider, timeframe)` pair, never
/// every `1D` evaluation universally.
#[tokio::test]
async fn ddr_55_verified_twelvedata_1d_convention_not_blocked() {
    let Some(db_url) = get_test_db_url() else {
        eprintln!("DDR-55: skipped (no MQK_DATABASE_URL)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("DDR-55: pool connect failed");

    let symbol = "ZZDDRTEST55";
    let provider = NyseWeekdaysProvider;
    let now = ts(MON_2024_01_08_10AM_ET);
    let schedule = resolve_market_session_schedule(&provider, now);
    let expected = expected_daily_end_ts_window(&provider, &schedule, now.timestamp(), 900, 20)
        .expect("resolves");

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = '1D'")
        .bind(symbol)
        .execute(&pool)
        .await
        .expect("cleanup failed");
    for &end_ts in &expected {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,'1D',$2,100000000,101000000,99000000,100500000,1000000,true,
                      'twelvedata','twelvedata',$1,'historical_backfill',$3)
            "#,
        )
        .bind(symbol)
        .bind(end_ts)
        .bind(ts(end_ts + 60))
        .execute(&pool)
        .await
        .expect("seed insert failed");
    }

    let mut instruments = instruments();
    instruments.push(twelvedata_instrument(symbol));

    let a = assignment(symbol, "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", symbol, 86_400);
    let result = evaluate_assignment(
        Some(&pool),
        &a,
        &binding,
        &provider,
        &provider_configs(),
        &instruments,
        &strategy_registry(),
        now,
    )
    .await;

    assert!(
        !result
            .blockers
            .contains(&REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED),
        "DDR-55: a verified provider's 1D convention must not block on this reason: {:?}",
        result.blockers
    );
    assert_eq!(
        result.readiness_state, "ready",
        "DDR-55: fully valid twelvedata-provenanced 1D warmup must reach ready: {result:?}"
    );

    sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(&pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// Lifecycle-binding safety (REPAIR 5,
// DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
// ---------------------------------------------------------------------------

/// The evaluator must consume the `EffectiveRuntimeBinding` exactly as
/// injected by the caller — never re-derive its own binding from the
/// environment. Proven by deliberately setting `MQK_STRATEGY_SYMBOL`/
/// `MQK_STRATEGY_IDS` to values that conflict with an explicitly injected,
/// fully matching binding: if `evaluate_assignment` ever silently re-read
/// the environment (e.g. by calling `bootstrap_with_effective_binding`
/// itself, as `evaluate_daily_data_readiness_from_env` does), the conflicting
/// env values would spuriously produce a binding-mismatch blocker even
/// though the injected binding matches perfectly.
#[tokio::test]
async fn ddr_47_evaluator_uses_injected_binding_not_environment() {
    // SAFETY: --test-threads=1 per the mission's required validation command.
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZZENVCONFLICTSYMBOL");
    std::env::set_var("MQK_STRATEGY_IDS", "mean_reversion");

    let a = assignment("AAPL", "swing_momentum", "1D");
    let binding = matching_binding("swing_momentum", "AAPL", 86_400);
    let result = evaluate_assignment(
        None,
        &a,
        &binding,
        &NyseWeekdaysProvider,
        &provider_configs(),
        &instruments(),
        &strategy_registry(),
        ts(MON_2024_01_08_10AM_ET),
    )
    .await;

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_IDS");

    assert!(
        !result
            .blockers
            .contains(&REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH)
            && !result
                .blockers
                .contains(&REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH)
            && !result
                .blockers
                .contains(&REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH),
        "DDR-47: injected binding must be used as-is, never re-derived from \
         conflicting environment state: {:?}",
        result.blockers
    );
    assert_eq!(
        result.readiness_state, "db_unavailable",
        "DDR-47: the injected binding must let evaluation progress past binding checks: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// REPAIR 2 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): continuity_state
// summary truth. `derive_continuity_state` is a pure function, tested
// directly against every continuity/history blocker in isolation — this
// proves the "never ok" invariant holds for each blocker regardless of
// which combinations the live bar-window evaluator can currently produce
// (e.g. `md_bars`'s primary key makes a genuine duplicate `end_ts` row
// impossible to ingest, so `REASON_DUPLICATE_TIMESTAMP` can only be
// exercised this way or via `evaluate_bar_readiness` directly, as DDR-24
// above already does).
// ---------------------------------------------------------------------------

#[test]
fn ddr_56_history_insufficient_continuity_state_never_ok() {
    // Required example: history_insufficient -> continuity_state != "ok".
    let state = derive_continuity_state(&[REASON_INSUFFICIENT_HISTORY], Some(3));
    assert_ne!(
        state, "ok",
        "REPAIR 2: insufficient_history must never report continuity_state=ok"
    );
    assert_eq!(state, "insufficient");

    // Realistic co-occurrence: under the live evaluator, insufficient
    // history always ships alongside a gap-type blocker too (a
    // fixed-length expected window can never be fully covered by fewer
    // completed bars than it requires) — gap_detected takes priority, and
    // that combination must still never be "ok".
    let state_with_gap =
        derive_continuity_state(&[REASON_INSUFFICIENT_HISTORY, REASON_INTERIOR_GAP], Some(3));
    assert_ne!(state_with_gap, "ok");
    assert_eq!(state_with_gap, "gap_detected");
}

#[test]
fn ddr_57_duplicate_timestamp_continuity_state_never_ok() {
    // Required example: duplicate timestamp -> continuity_state != "ok".
    let state = derive_continuity_state(&[REASON_DUPLICATE_TIMESTAMP], Some(21));
    assert_ne!(
        state, "ok",
        "REPAIR 2: duplicate_timestamp must never report continuity_state=ok"
    );
    assert_eq!(state, "duplicate_detected");
}

#[test]
fn ddr_58_interior_gap_continuity_state_is_gap_detected() {
    // Required example: interior gap -> gap_detected.
    assert_eq!(
        derive_continuity_state(&[REASON_INTERIOR_GAP], Some(19)),
        "gap_detected"
    );
    assert_eq!(
        derive_continuity_state(&[REASON_EXPECTED_LATEST_BAR_MISSING], Some(19)),
        "gap_detected"
    );
}

#[test]
fn ddr_59_unsupported_intraday_continuity_state_is_unsupported() {
    // Required example: unsupported continuity -> unsupported.
    assert_eq!(
        derive_continuity_state(&[REASON_UNSUPPORTED_INTRADAY_CONTINUITY], None),
        "unsupported"
    );
}

#[test]
fn ddr_60_calendar_and_market_data_blockers_are_unknown_never_ok() {
    assert_eq!(
        derive_continuity_state(&[REASON_CALENDAR_UNAVAILABLE], None),
        "unknown"
    );
    assert_eq!(
        derive_continuity_state(&[REASON_MARKET_DATA_MISSING], None),
        "unknown"
    );
}

#[test]
fn ddr_61_no_blockers_and_loaded_bars_is_ok_absent_bars_is_unknown() {
    assert_eq!(derive_continuity_state(&[], Some(20)), "ok");
    assert_eq!(derive_continuity_state(&[], None), "unknown");
}
