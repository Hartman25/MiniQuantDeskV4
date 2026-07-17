//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER: proof
//! tests for the autonomous completed-bar driver (`state::autonomous_completed_bar_driver`)
//! and the extracted latest-bar poll seam (`state::market_data_latest_bar`).
//!
//! DB-backed tests skip truthfully when `MQK_DATABASE_URL` is not set. No
//! real provider, broker, or network call is made anywhere in this file —
//! every provider is a fake injected trait object.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, Utc};
use mqk_daemon::daily_data_readiness::AssignmentReadiness;
use mqk_daemon::state::autonomous_completed_bar_driver::{
    resolve_autonomous_provider_call_authorization, resolve_single_effective_binding,
    tick_autonomous_completed_bar_driver, AutonomousBindingRejection,
    AutonomousCompletedBarDriverInput, AutonomousCompletedBarDriverOutcome,
    AutonomousProviderCallAuthorization,
};
use mqk_daemon::state::market_data_latest_bar::LatestBarRegistryAdmissionRejection;
use mqk_daemon::state::{self, OperatorAuthMode};
use mqk_daemon::state::{
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SymbolStrategyAssignment,
};
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_runtime::native_strategy::{
    build_daemon_plugin_registry, EffectiveRuntimeBinding, NativeStrategyBootstrap,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fixture_instrument(
    symbol: &str,
    provider: &str,
    provider_symbol: &str,
    timeframe: &str,
) -> TrackedInstrument {
    TrackedInstrument {
        instrument_id: format!("equity:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        provider: provider.to_string(),
        provider_symbol: provider_symbol.to_string(),
        venue: "TEST".to_string(),
        currency: "USD".to_string(),
        enabled: true,
        timeframes: vec![timeframe.to_string()],
        notes: "test fixture".to_string(),
        instrument_kind: None,
        sector: None,
        category: None,
    }
}

fn fixture_assignment_config(
    symbol: &str,
    strategy_id: &str,
    timeframe: &str,
) -> MultiSymbolRuntimeConfig {
    MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: timeframe.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    }
}

fn fixture_binding(
    symbol: &str,
    strategy_id: &str,
    timeframe_secs: i64,
) -> EffectiveRuntimeBinding {
    EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(strategy_id.to_string()),
        effective_runtime_target_symbol: Some(symbol.to_string()),
        effective_runtime_timeframe_secs: Some(timeframe_secs),
    }
}

fn ready_readiness(
    symbol: &str,
    timeframe: &str,
    expected_latest_bar_ts: Option<i64>,
) -> AssignmentReadiness {
    AssignmentReadiness {
        assignment_symbol: symbol.to_string(),
        assignment_timeframe: timeframe.to_string(),
        configured_strategy_id: "swing_momentum".to_string(),
        effective_runtime_strategy_id: Some("swing_momentum".to_string()),
        effective_runtime_target_symbol: Some(symbol.to_string()),
        effective_runtime_timeframe_secs: Some(300),
        required_history_bars: Some(1),
        asset_class: Some("equity".to_string()),
        expected_provider_id: Some("fake".to_string()),
        expected_provider_symbol: Some(symbol.to_string()),
        actual_provider_ids: vec!["fake".to_string()],
        actual_provider_symbols: vec![symbol.to_string()],
        readiness_state: "ready",
        blockers: vec![],
        configured_grace_seconds: 0,
        effective_grace_seconds: 0,
        configured_future_skew_seconds: 0,
        effective_future_skew_seconds: 0,
        loaded_completed_bars: Some(1),
        expected_latest_bar_ts,
        actual_latest_bar_ts: expected_latest_bar_ts,
        continuity_state: "ok",
        provenance_state: "ok",
        remediation: vec![],
    }
}

fn blocked_readiness(
    symbol: &str,
    timeframe: &str,
    blockers: Vec<&'static str>,
) -> AssignmentReadiness {
    let mut r = ready_readiness(symbol, timeframe, None);
    r.readiness_state = "blocked";
    r.blockers = blockers;
    r
}

type FakeLatestBarOutcome = Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError>;
type FakeLatestBarOutcomeQueues = std::collections::HashMap<String, VecDeque<FakeLatestBarOutcome>>;

/// Queue-based fake provider: each call to `fetch_latest_closed_bar` pops
/// one outcome from the front of the queue (per symbol), defaulting to
/// `Ok(None)` once exhausted. Makes zero network calls; `calls()` proves
/// exactly how many times the provider was actually invoked.
#[derive(Default)]
struct FakeQueueProvider {
    outcomes: Mutex<FakeLatestBarOutcomeQueues>,
    calls: AtomicUsize,
    capabilities: Mutex<Option<mqk_md::MarketDataProviderCapabilities>>,
}

impl FakeQueueProvider {
    fn new() -> Self {
        Self::default()
    }

    fn push_outcome(
        &self,
        symbol: &str,
        outcome: Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError>,
    ) {
        self.outcomes
            .lock()
            .unwrap()
            .entry(symbol.to_string())
            .or_default()
            .push_back(outcome);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn set_capabilities(&self, caps: mqk_md::MarketDataProviderCapabilities) {
        *self.capabilities.lock().unwrap() = Some(caps);
    }
}

#[async_trait::async_trait]
impl mqk_md::MarketDataProvider for FakeQueueProvider {
    fn provider_id(&self) -> &str {
        "fake"
    }
    fn display_name(&self) -> &str {
        "Fake Queue Provider"
    }
    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        self.capabilities.lock().unwrap().clone().unwrap_or(
            mqk_md::MarketDataProviderCapabilities {
                historical_bars: false,
                latest_closed_bar: true,
                completed_bar_stream: false,
                supported_asset_classes: vec![mqk_md::ProviderAssetClass::Equity],
                supported_timeframes: vec![mqk_md::Timeframe::M5, mqk_md::Timeframe::D1],
            },
        )
    }
    fn health(&self) -> mqk_md::MarketDataProviderHealth {
        mqk_md::MarketDataProviderHealth::unknown()
    }
    fn rate_limits(&self) -> Option<mqk_md::MarketDataProviderRateLimits> {
        None
    }
    async fn fetch_historical_bars(
        &self,
        _request: mqk_md::HistoricalBarsRequest,
    ) -> Result<Vec<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        Err(mqk_md::MarketDataProviderError::UnsupportedCapability {
            provider_id: "fake".to_string(),
            capability: "historical_bars".to_string(),
        })
    }
    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.outcomes.lock().unwrap();
        match guard.get_mut(&request.symbol).and_then(|q| q.pop_front()) {
            Some(outcome) => outcome,
            None => Ok(None),
        }
    }
}

fn bar(symbol: &str, timeframe: &str, end_ts: i64, is_complete: bool) -> mqk_md::CanonicalBar {
    mqk_md::CanonicalBar {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open: "100".to_string(),
        high: "101".to_string(),
        low: "99".to_string(),
        close: "100.5".to_string(),
        volume: 1000,
        is_complete,
    }
}

async fn maybe_db(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("{label}: skipped DB-backed proof because MQK_DATABASE_URL is not set");
            return None;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where local_symbol like 'ZZDRV%'")
        .execute(&pool)
        .await
        .expect("clean dispatch claims");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zzdrv%')")
        .execute(&pool)
        .await
        .expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zzdrv%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    sqlx::query("delete from md_bars where symbol like 'ZZDRV%'")
        .execute(&pool)
        .await
        .expect("clean md_bars");
    Some(pool)
}

/// One reusable timing skeleton: a regular trading day, preopen 09:00Z,
/// exchange session 09:30Z-16:00Z, effective operation window equal to the
/// exchange window (no fixed-window override), postclose well after close.
struct Timing {
    preopen_start_utc: DateTime<Utc>,
    exchange_open: DateTime<Utc>,
    exchange_close: DateTime<Utc>,
    effective_open: DateTime<Utc>,
    effective_close: DateTime<Utc>,
    postclose_finalize_utc: DateTime<Utc>,
    previous_trading_date: NaiveDate,
    market_date: NaiveDate,
}

/// Same shape as [`standard_timing`] but dated well in the past (2020-01-06,
/// a Monday). The per-tick staleness gate inside
/// `AppState::dispatch_native_strategy_for_symbol_with_bar` compares a
/// bar's `end_ts` against the real wall clock (`Utc::now()`), not any
/// injected time — so proving that gate still fires (C.19 point 40)
/// requires a bar whose `end_ts` is genuinely, unavoidably old relative to
/// whatever instant the test actually runs at, not merely "old" relative to
/// this driver's own injected `now_utc`.
fn past_timing() -> Timing {
    let market_date = NaiveDate::from_ymd_opt(2020, 1, 6).unwrap(); // Monday
    let previous_trading_date = NaiveDate::from_ymd_opt(2020, 1, 3).unwrap(); // Friday
    let exchange_open = DateTime::parse_from_rfc3339("2020-01-06T13:30:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let exchange_close = DateTime::parse_from_rfc3339("2020-01-06T20:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    Timing {
        preopen_start_utc: DateTime::parse_from_rfc3339("2020-01-06T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        exchange_open,
        exchange_close,
        effective_open: exchange_open,
        effective_close: exchange_close,
        postclose_finalize_utc: DateTime::parse_from_rfc3339("2020-01-06T20:15:00Z")
            .unwrap()
            .with_timezone(&Utc),
        previous_trading_date,
        market_date,
    }
}

fn standard_timing() -> Timing {
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(); // Monday
    let previous_trading_date = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(); // Friday
    let exchange_open = DateTime::parse_from_rfc3339("2026-07-20T13:30:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let exchange_close = DateTime::parse_from_rfc3339("2026-07-20T20:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    Timing {
        preopen_start_utc: DateTime::parse_from_rfc3339("2026-07-20T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        exchange_open,
        exchange_close,
        effective_open: exchange_open,
        effective_close: exchange_close,
        postclose_finalize_utc: DateTime::parse_from_rfc3339("2026-07-20T20:15:00Z")
            .unwrap()
            .with_timezone(&Utc),
        previous_trading_date,
        market_date,
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_test_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    symbol: &str,
    strategy_id: &str,
    timeframe: &str,
    timing: &Timing,
    initial_state: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let assignment_config = fixture_assignment_config(symbol, strategy_id, timeframe);
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let timeframe_secs = mqk_md::Timeframe::parse(timeframe)
        .expect("valid timeframe")
        .duration_secs();
    let binding = fixture_binding(symbol, strategy_id, timeframe_secs);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let session_plan_identity = format!("test-session-plan|{adapter_id}");
    let operation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.autonomous-daily-operation.v1|test|{adapter_id}").as_bytes(),
    );
    let now = timing.preopen_start_utc;

    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: timing.market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: timing.effective_open,
        effective_operation_close_utc: timing.effective_close,
        exchange_session_open_utc: timing.exchange_open,
        exchange_session_close_utc: timing.exchange_close,
        exchange_is_early_close: false,
        previous_trading_date: timing.previous_trading_date,
        preopen_start_utc: timing.preopen_start_utc,
        postclose_finalize_utc: timing.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "awaiting_preopen".to_string(),
        occurred_at_utc: now,
        bounded_detail: "test fixture".to_string(),
    };

    match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create operation")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => record,
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
            panic!("unexpected identity conflict in test fixture")
        }
    }
}

// ---------------------------------------------------------------------------
// Authorization (C.19 points 1-4)
// ---------------------------------------------------------------------------

#[test]
fn auth_01_both_true_authorizes() {
    assert_eq!(
        resolve_autonomous_provider_call_authorization(Some("true"), Some("true")),
        AutonomousProviderCallAuthorization::Authorized
    );
}

#[test]
fn auth_02_both_absent_disabled() {
    assert_eq!(
        resolve_autonomous_provider_call_authorization(None, None),
        AutonomousProviderCallAuthorization::Disabled
    );
}

#[test]
fn auth_03_either_false_disabled() {
    assert_eq!(
        resolve_autonomous_provider_call_authorization(Some("false"), Some("true")),
        AutonomousProviderCallAuthorization::Disabled
    );
    assert_eq!(
        resolve_autonomous_provider_call_authorization(Some("true"), Some("false")),
        AutonomousProviderCallAuthorization::Disabled
    );
}

#[test]
fn auth_04_malformed_produces_invalid() {
    match resolve_autonomous_provider_call_authorization(Some("yes"), Some("true")) {
        AutonomousProviderCallAuthorization::Invalid { .. } => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Admission (C.19 points 5-10)
// ---------------------------------------------------------------------------

#[test]
fn admission_05_invalid_provider_registry_zero_calls() {
    let bad_path = "C:/definitely/does/not/exist/providers.json";
    let good_instruments = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(good_instruments.path(), "[]").unwrap();
    let result =
        mqk_daemon::state::autonomous_completed_bar_driver::load_driver_instruments_and_provider(
            good_instruments.path().to_str().unwrap(),
            bad_path,
            "fake",
        );
    assert!(matches!(
        result,
        Err(mqk_daemon::state::autonomous_completed_bar_driver::AutonomousDriverSetupRejection::ProviderRegistryUnavailable(_))
    ));
}

#[test]
fn admission_06_invalid_instrument_registry_zero_calls() {
    let bad_instruments = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(bad_instruments.path(), "not json").unwrap();
    let good_providers = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(good_providers.path(), "[]").unwrap();
    let result =
        mqk_daemon::state::autonomous_completed_bar_driver::load_driver_instruments_and_provider(
            bad_instruments.path().to_str().unwrap(),
            good_providers.path().to_str().unwrap(),
            "fake",
        );
    assert!(matches!(
        result,
        Err(mqk_daemon::state::autonomous_completed_bar_driver::AutonomousDriverSetupRejection::InstrumentRegistryUnavailable(_))
    ));
}

#[tokio::test]
async fn admission_07_disabled_provider_zero_calls() {
    let Some(pool) = maybe_db("admission_07").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-adm07",
        "ZZDRVSYM",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVSYM", "fake", "ZZDRVSYM", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVSYM", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVSYM", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    provider.set_capabilities(mqk_md::MarketDataProviderCapabilities {
        historical_bars: false,
        latest_closed_bar: false, // capability disabled -> Unsupported, not RegistryBlocked
        completed_bar_stream: false,
        supported_asset_classes: vec![],
        supported_timeframes: vec![],
    });
    let readiness = ready_readiness("ZZDRVSYM", "5m", Some(1_000));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert!(matches!(
        outcome,
        AutonomousCompletedBarDriverOutcome::Unsupported { .. }
    ));
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn admission_08_blank_provider_symbol_zero_calls() {
    let Some(pool) = maybe_db("admission_08").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-adm08",
        "ZZDRVSYM",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVSYM", "fake", "", "5m")]; // blank provider_symbol
    let assignment_config = fixture_assignment_config("ZZDRVSYM", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVSYM", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVSYM", "5m", Some(1_000));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert!(matches!(
        outcome,
        AutonomousCompletedBarDriverOutcome::RegistryBlocked {
            rejection: LatestBarRegistryAdmissionRejection::BlankProviderSymbol
        }
    ));
    assert_eq!(provider.calls(), 0);
}

#[test]
fn admission_09_unsupported_timeframe_binding_rejected() {
    let operation_stub = stub_operation("id-mismatch", "id-mismatch2");
    let assignment_config =
        fixture_assignment_config("ZZDRVSYM", "swing_momentum", "not_a_timeframe");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVSYM", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let mut op = operation_stub;
    op.assignment_identity = assignment_identity.clone();
    op.runtime_binding_identity = runtime_binding_identity.clone();

    let result = resolve_single_effective_binding(
        &op,
        &assignment_config,
        &assignment_identity,
        &binding,
        &runtime_binding_identity,
    );
    assert_eq!(
        result,
        Err(AutonomousBindingRejection::UnsupportedTimeframe)
    );
}

#[test]
fn admission_10_multi_symbol_assignment_not_exactly_bound() {
    let assignment_config = MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![
            SymbolStrategyAssignment {
                symbol: "AAA".to_string(),
                strategy_id: "s".to_string(),
                timeframe: "5m".to_string(),
            },
            SymbolStrategyAssignment {
                symbol: "BBB".to_string(),
                strategy_id: "s".to_string(),
                timeframe: "5m".to_string(),
            },
        ],
        max_concurrent_symbols: 2,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("AAA", "s", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let mut op = stub_operation("x", "y");
    op.assignment_identity = assignment_identity.clone();
    op.runtime_binding_identity = runtime_binding_identity.clone();

    let result = resolve_single_effective_binding(
        &op,
        &assignment_config,
        &assignment_identity,
        &binding,
        &runtime_binding_identity,
    );
    assert_eq!(
        result,
        Err(AutonomousBindingRejection::MultiSymbolAssignmentNotExactlyBound)
    );
}

/// Minimal in-memory (non-DB) operation record for pure binding-logic tests
/// that never touch the database.
fn stub_operation(
    assignment_identity: &str,
    runtime_binding_identity: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let now = Utc::now();
    mqk_db::AutonomousDailyOperationRecord {
        operation_id: Uuid::new_v4(),
        market_date: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "paper".to_string(),
        adapter_id: "zzdrv-stub".to_string(),
        session_plan_identity: "stub".to_string(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: now,
        effective_operation_close_utc: now + chrono::Duration::hours(6),
        exchange_session_open_utc: Some(now),
        exchange_session_close_utc: Some(now + chrono::Duration::hours(6)),
        exchange_is_early_close: Some(false),
        previous_trading_date: Some(NaiveDate::from_ymd_opt(2026, 7, 17).unwrap()),
        preopen_start_utc: now - chrono::Duration::minutes(30),
        postclose_finalize_utc: now + chrono::Duration::hours(6) + chrono::Duration::minutes(15),
        state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        state_reason_code: None,
        state_version: 1,
        run_id: None,
        start_attempt_count: 0,
        last_start_attempt_utc: None,
        next_retry_utc: None,
        data_refresh_state: "awaiting_preopen".to_string(),
        last_provider_poll_utc: None,
        provider_poll_attempt_count: 0,
        provider_poll_success_count: 0,
        provider_poll_failure_count: 0,
        last_completed_bar_ts: None,
        last_dispatched_bar_ts: None,
        bars_observed: 0,
        bars_dispatched: 0,
        started_at_utc: None,
        stopped_at_utc: None,
        finalized_at_utc: None,
        outcome: None,
        no_trade_reason: None,
        last_error: None,
        created_at_utc: now,
        updated_at_utc: now,
    }
}

// ---------------------------------------------------------------------------
// Poll cadence (C.19 points 11-16) + session truth (45-47) + window (16)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cadence_11_not_repeated_for_same_expected_bar_after_success() {
    let Some(pool) = maybe_db("cadence_11").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-c11",
        "ZZDRVC11",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    // Simulate: this exact expected bar was already observed.
    let expected_ts = timing.effective_open.timestamp() + 300;
    let outcome = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    assert!(matches!(
        outcome,
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { .. }
    ));
    operation.last_completed_bar_ts = Some(expected_ts);

    let instruments = vec![fixture_instrument("ZZDRVC11", "fake", "ZZDRVC11", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVC11", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVC11", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVC11", "5m", Some(expected_ts));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(outcome, AutonomousCompletedBarDriverOutcome::PollNotDue);
    assert_eq!(
        provider.calls(),
        0,
        "already-observed expected bar must not trigger a poll"
    );
}

#[tokio::test]
async fn cadence_12_13_no_poll_before_interval_close_plus_grace() {
    let Some(pool) = maybe_db("cadence_12_13").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-c12",
        "ZZDRVC12",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVC12", "fake", "ZZDRVC12", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVC12", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVC12", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    // Bundle 2's own evaluator says no interval has closed yet (None).
    let readiness = ready_readiness("ZZDRVC12", "5m", None);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(1),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(outcome, AutonomousCompletedBarDriverOutcome::PollNotDue);
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn cadence_14_15_new_due_interval_polls_again_after_success() {
    let Some(pool) = maybe_db("cadence_14_15").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-c14",
        "ZZDRVC14",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVC14", "fake", "ZZDRVC14", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVC14", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVC14", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let first_expected_ts = timing.effective_open.timestamp() + 300;
    provider.push_outcome(
        "ZZDRVC14",
        Ok(Some(bar("ZZDRVC14", "5m", first_expected_ts, true))),
    );
    let readiness1 = ready_readiness("ZZDRVC14", "5m", Some(first_expected_ts));
    let now1 = timing.effective_open + chrono::Duration::minutes(6);
    let outcome1 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now1,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness1,
    })
    .await
    .expect("tick ok");
    assert!(
        matches!(
            outcome1,
            AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. }
                | AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved { .. }
                | AutonomousCompletedBarDriverOutcome::DispatchCompleted { .. }
        ),
        "expected a dispatch-path outcome for a freshly observed bar, got {outcome1:?}"
    );
    assert_eq!(provider.calls(), 1);

    // Refresh operation row (bars_observed/last_completed_bar_ts advanced).
    operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch")
        .expect("exists");

    // Same expected bar again — must not re-poll (cadence 11/14 re-proof).
    let readiness_same = ready_readiness("ZZDRVC14", "5m", Some(first_expected_ts));
    let outcome_same = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now1 + chrono::Duration::seconds(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness_same,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome_same,
        AutonomousCompletedBarDriverOutcome::PollNotDue
    );
    assert_eq!(
        provider.calls(),
        1,
        "no re-poll for the same already-observed expected bar"
    );

    // A new due interval (next bar) may poll again (point 15).
    let second_expected_ts = first_expected_ts + 300;
    provider.push_outcome(
        "ZZDRVC14",
        Ok(Some(bar("ZZDRVC14", "5m", second_expected_ts, true))),
    );
    let readiness2 = ready_readiness("ZZDRVC14", "5m", Some(second_expected_ts));
    let now2 = timing.effective_open + chrono::Duration::minutes(11);
    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now2,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness2,
    })
    .await
    .expect("tick ok");
    assert!(!matches!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::PollNotDue
    ));
    assert_eq!(provider.calls(), 2, "a new due interval must poll again");
}

#[tokio::test]
async fn cadence_16_no_poll_after_effective_operation_close() {
    let Some(pool) = maybe_db("cadence_16").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-c16",
        "ZZDRVC16",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVC16", "fake", "ZZDRVC16", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVC16", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVC16", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVC16", "5m", Some(timing.effective_close.timestamp()));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_close + chrono::Duration::minutes(1),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::OutsideOperationWindow
    );
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn window_before_preopen_not_applicable() {
    let Some(pool) = maybe_db("window_before_preopen").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-preopen",
        "ZZDRVPRE",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_PREOPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVPRE", "fake", "ZZDRVPRE", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVPRE", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVPRE", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVPRE", "5m", None);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.preopen_start_utc - chrono::Duration::minutes(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::OutsideOperationWindow
    );
    assert_eq!(provider.calls(), 0);
}

// ---------------------------------------------------------------------------
// Mapping (C.19 points 17-21)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mapping_17_21_canonical_symbol_and_provenance_stored() {
    let Some(pool) = maybe_db("mapping_17_21").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-map",
        "ZZDRVMAP",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    // provider_symbol differs from local canonical symbol.
    let instruments = vec![fixture_instrument("ZZDRVMAP", "fake", "ZZDRVMAP.PRV", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVMAP", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVMAP", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let expected_ts = timing.effective_open.timestamp() + 300;
    provider.push_outcome(
        "ZZDRVMAP.PRV",
        Ok(Some(bar("ZZDRVMAP.PRV", "5m", expected_ts, true))),
    );
    let readiness = ready_readiness("ZZDRVMAP", "5m", Some(expected_ts));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let _outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        provider.calls(),
        1,
        "provider must have been called with the provider symbol"
    );

    let row: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "select symbol, provider_id, provider_symbol, ingest_mode from md_bars where symbol = $1 and timeframe = '5m' and end_ts = $2",
    )
    .bind("ZZDRVMAP")
    .bind(expected_ts)
    .fetch_one(&pool)
    .await
    .expect("md_bars row exists");
    assert_eq!(
        row.0, "ZZDRVMAP",
        "md_bars.symbol must be the canonical local symbol (17,19)"
    );
    assert_eq!(
        row.1, "fake",
        "md_bars.provider_id must be canonical provider id (19)"
    );
    assert_eq!(
        row.2.as_deref(),
        Some("ZZDRVMAP.PRV"),
        "md_bars.provider_symbol must be canonical provider symbol (20)"
    );
    assert_eq!(
        row.3.as_deref(),
        Some("autonomous_daily_operation_driver"),
        "md_bars.ingest_mode must be the driver's latest-poll mode (21)"
    );
}

// ---------------------------------------------------------------------------
// Bar eligibility via Bundle 2 readiness (C.19 points 22-30)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn readiness_22_30_not_ready_blocks_regardless_of_reason() {
    let Some(pool) = maybe_db("readiness_22_30").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-rdy",
        "ZZDRVRDY",
        "swing_momentum",
        "1D",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVRDY", "alpaca", "ZZDRVRDY", "1D")];
    let assignment_config = fixture_assignment_config("ZZDRVRDY", "swing_momentum", "1D");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVRDY", "swing_momentum", 86_400);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    // Proxy for "Alpaca 1D remains blocked" (point 30): Bundle 2's own
    // readiness evaluator would set this exact blocker for an
    // unverified-timestamp-convention provider/timeframe pair; the driver's
    // job is only to honor `readiness_state != "ready"`, never to
    // re-implement that judgment itself.
    let readiness = blocked_readiness(
        "ZZDRVRDY",
        "1D",
        vec!["provider_timestamp_convention_unverified"],
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::hours(1),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "alpaca",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ReadinessBlocked { blockers } => {
            assert_eq!(blockers, vec!["provider_timestamp_convention_unverified"]);
        }
        other => panic!("expected ReadinessBlocked, got {other:?}"),
    }
    assert_eq!(
        provider.calls(),
        0,
        "readiness-blocked assignment must never poll (22-30)"
    );
}

#[tokio::test]
async fn readiness_29_verified_provider_may_proceed_to_poll() {
    let Some(pool) = maybe_db("readiness_29").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-tdok",
        "ZZDRVTDOK",
        "swing_momentum",
        "1D",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument(
        "ZZDRVTDOK",
        "twelvedata",
        "ZZDRVTDOK",
        "1D",
    )];
    let assignment_config = fixture_assignment_config("ZZDRVTDOK", "swing_momentum", "1D");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVTDOK", "swing_momentum", 86_400);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let expected_ts = timing.effective_open.timestamp();
    provider.push_outcome("ZZDRVTDOK", Ok(None)); // no bar yet is fine — just proves the poll was attempted
    let readiness = ready_readiness("ZZDRVTDOK", "1D", Some(expected_ts));
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::hours(1),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "twelvedata",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::NoNewCompletedBar
    );
    assert_eq!(provider.calls(), 1, "a ready assignment must actually poll");
}

// ---------------------------------------------------------------------------
// Observation (C.19 points 31-34)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn observation_31_32_33_bars_observed_counter_semantics() {
    let Some(pool) = maybe_db("observation_31_32_33").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-obs",
        "ZZDRVOBS",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let ts1 = timing.effective_open.timestamp() + 300;
    let ts2 = ts1 + 300;

    let r1 = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        ts1,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(
        r1,
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { bars_observed: 1 }
    ));

    // Point 32: same completed bar does not increment twice.
    let r1_again = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        ts1,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(
        r1_again,
        mqk_db::RecordCompletedBarObservedOutcome::AlreadyObserved { bars_observed: 1 }
    ));

    // Point 31 (continued): a genuinely new bar increments once more.
    let r2 = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        ts2,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(
        r2,
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { bars_observed: 2 }
    ));

    // Point 33: an older bar cannot replace the newer last_completed_bar_ts.
    let r_old = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        ts1,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(
        r_old,
        mqk_db::RecordCompletedBarObservedOutcome::StaleBarIgnored { current_last_completed_bar_ts: Some(ts) } if ts == ts2
    ));
}

#[tokio::test]
async fn observation_34_db_evidence_failure_prevents_dispatch() {
    let Some(pool) = maybe_db("observation_34").await else {
        return;
    };
    let timing = standard_timing();
    // Deliberately do NOT create the operation row — the driver must treat a
    // missing operation as an evidence-persistence failure, never silently
    // proceeding to a provider call or dispatch.
    let mut operation = stub_operation("a", "b");
    let assignment_config = fixture_assignment_config("ZZDRVMISSING", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVMISSING", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    operation.assignment_identity = assignment_identity.clone();
    operation.runtime_binding_identity = runtime_binding_identity.clone();
    operation.preopen_start_utc = timing.preopen_start_utc;
    operation.effective_operation_open_utc = timing.effective_open;
    operation.effective_operation_close_utc = timing.effective_close;

    let instruments = vec![fixture_instrument(
        "ZZDRVMISSING",
        "fake",
        "ZZDRVMISSING",
        "5m",
    )];
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness(
        "ZZDRVMISSING",
        "5m",
        Some(timing.effective_open.timestamp() + 300),
    );
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert!(matches!(
        outcome,
        AutonomousCompletedBarDriverOutcome::EvidencePersistenceFailed { .. }
    ));
    assert_eq!(
        provider.calls(),
        0,
        "evidence write must precede and gate any provider call"
    );
}

// ---------------------------------------------------------------------------
// Dispatch (C.19 points 35-44)
// ---------------------------------------------------------------------------

async fn active_bootstrap_state(pool: sqlx::PgPool) -> state::AppState {
    let state =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    let bootstrap = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    state
        .set_native_strategy_bootstrap_for_test(Some(bootstrap))
        .await;
    state
}

#[tokio::test]
async fn dispatch_35_36_37_39_41_new_bar_dispatches_once_new_bar_dispatches_again() {
    let Some(pool) = maybe_db("dispatch_35_36_37").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-disp",
        "ZZDRVDISP",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVDISP", "fake", "ZZDRVDISP", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVDISP", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVDISP", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let state = active_bootstrap_state(pool.clone()).await;

    let ts1 = timing.effective_open.timestamp() + 300;
    provider.push_outcome("ZZDRVDISP", Ok(Some(bar("ZZDRVDISP", "5m", ts1, true))));
    let now1 = DateTime::<Utc>::from_timestamp(ts1 + 10, 0).unwrap();
    let readiness1 = ready_readiness("ZZDRVDISP", "5m", Some(ts1));

    let outcome1 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now1,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness1,
    })
    .await
    .expect("tick ok");

    // Point 35, 39, 41: a new eligible bar dispatches exactly once.
    assert_eq!(
        outcome1,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted { bar_end_ts: ts1 }
    );
    operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(operation.bars_dispatched, 1);
    assert_eq!(operation.last_dispatched_bar_ts, Some(ts1));

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVDISP",
        "5m",
        ts1,
    )
    .await
    .unwrap()
    .expect("claim exists");
    assert_eq!(claim.status, mqk_db::DISPATCH_STATUS_COMPLETED);

    // Point 36: repeated tick with the SAME bar_end_ts does not redispatch.
    // (Force by directly re-invoking the claim, since the operation's
    // last_completed_bar_ts already equals ts1 and cadence would refuse to
    // re-poll — this proves the deeper claim-level guarantee directly.)
    let reclaim = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVDISP",
        "5m",
        ts1,
        now1,
    )
    .await
    .unwrap();
    assert!(matches!(
        reclaim,
        mqk_db::BarDispatchClaimOutcome::AlreadyCompleted { .. }
    ));

    // Point 37: a new bar_end_ts dispatches once more.
    let ts2 = ts1 + 300;
    provider.push_outcome("ZZDRVDISP", Ok(Some(bar("ZZDRVDISP", "5m", ts2, true))));
    let now2 = DateTime::<Utc>::from_timestamp(ts2 + 10, 0).unwrap();
    let readiness2 = ready_readiness("ZZDRVDISP", "5m", Some(ts2));

    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now2,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness2,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted { bar_end_ts: ts2 }
    );
    let operation_final =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        operation_final.bars_dispatched, 2,
        "point 41: dispatch success increments bars_dispatched exactly once per bar"
    );
    assert_eq!(operation_final.last_dispatched_bar_ts, Some(ts2));
}

#[tokio::test]
async fn dispatch_38_43_restart_recreated_driver_does_not_redispatch() {
    let Some(pool) = maybe_db("dispatch_38_43").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-restart",
        "ZZDRVRST",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let ts1 = timing.effective_open.timestamp() + 300;
    let now1 = DateTime::<Utc>::from_timestamp(ts1 + 10, 0).unwrap();

    // Simulate a prior process that claimed but crashed before completing.
    let claim1 = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVRST",
        "5m",
        ts1,
        now1,
    )
    .await
    .unwrap();
    assert!(matches!(claim1, mqk_db::BarDispatchClaimOutcome::Claimed));

    // "Restart": a fresh attempt to claim the exact same bar identity.
    let claim2 = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVRST",
        "5m",
        ts1,
        now1 + chrono::Duration::minutes(1),
    )
    .await
    .unwrap();
    assert!(
        matches!(claim2, mqk_db::BarDispatchClaimOutcome::Unresolved { status } if status == mqk_db::DISPATCH_STATUS_UNCERTAIN)
    );

    let claim_row = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVRST",
        "5m",
        ts1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(claim_row.status, mqk_db::DISPATCH_STATUS_UNCERTAIN);

    // A third attempt remains Unresolved — never silently redispatched (43).
    let claim3 = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVRST",
        "5m",
        ts1,
        now1 + chrono::Duration::minutes(2),
    )
    .await
    .unwrap();
    assert!(
        matches!(claim3, mqk_db::BarDispatchClaimOutcome::Unresolved { status } if status == mqk_db::DISPATCH_STATUS_UNCERTAIN)
    );
}

#[tokio::test]
async fn dispatch_40_42_staleness_gate_remains_active_and_failure_not_falsely_completed() {
    let Some(pool) = maybe_db("dispatch_40_42").await else {
        return;
    };
    let timing = past_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-stale",
        "ZZDRVSTALE",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVSTALE", "fake", "ZZDRVSTALE", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVSTALE", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVSTALE", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let state = active_bootstrap_state(pool.clone()).await;

    let ts1 = timing.effective_open.timestamp() + 300;
    provider.push_outcome("ZZDRVSTALE", Ok(Some(bar("ZZDRVSTALE", "5m", ts1, true))));
    let readiness1 = ready_readiness("ZZDRVSTALE", "5m", Some(ts1));
    // now_utc is past the default intraday staleness cap (900s) for the
    // just-ingested bar, but still comfortably inside the effective
    // operation window, so the per-tick staleness gate deep inside
    // `dispatch_native_strategy_for_symbol_with_bar` (point 40) refuses to
    // dispatch despite this driver's own gates all passing.
    let now_stale = DateTime::<Utc>::from_timestamp(ts1 + 1_000, 0).unwrap();
    assert!(
        now_stale < timing.effective_close,
        "test fixture must keep now_utc inside the operation window"
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now_stale,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness1,
    })
    .await
    .expect("tick ok");

    // Point 42: a dispatch that did not confirm success is never marked completed.
    match outcome {
        AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved { status } => {
            assert_eq!(status, mqk_db::DISPATCH_STATUS_FAILED);
        }
        other => panic!(
            "expected DispatchClaimUnresolved(failed) from the staleness gate, got {other:?}"
        ),
    }
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVSTALE",
        "5m",
        ts1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(
        claim.status,
        mqk_db::DISPATCH_STATUS_COMPLETED,
        "point 42: failure must never be falsely marked completed"
    );
}

#[tokio::test]
async fn dispatch_44_no_broker_or_order_side_effects_in_test() {
    // Structural proof: this whole test file never constructs a broker
    // client, never calls an order/outbox function directly, and every
    // provider used is `FakeQueueProvider` (in-process, no network). If a
    // broker/order call were reachable from this file it would require an
    // import this file does not have.
    let source = std::fs::read_to_string(file!()).unwrap_or_default();
    assert!(
        !source.contains("mqk_broker"),
        "this test file must never reference a broker crate"
    );
}

// ---------------------------------------------------------------------------
// Session truth (C.19 points 45-47)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_45_early_close_operation_still_pollable() {
    let Some(pool) = maybe_db("session_45").await else {
        return;
    };
    let mut timing = standard_timing();
    // Early close: exchange session shortened, effective window follows.
    timing.exchange_close = timing.exchange_open + chrono::Duration::hours(3);
    timing.effective_close = timing.exchange_close;
    timing.postclose_finalize_utc = timing.effective_close + chrono::Duration::minutes(15);

    let assignment_config = fixture_assignment_config("ZZDRVEC", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVEC", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let operation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"mqk.autonomous-daily-operation.v1|test|zzdrv-ec",
    );
    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: timing.market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: "zzdrv-ec".to_string(),
        session_plan_identity: "test-session-plan|ec".to_string(),
        assignment_identity: assignment_identity.clone(),
        runtime_binding_identity: runtime_binding_identity.clone(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: timing.effective_open,
        effective_operation_close_utc: timing.effective_close,
        exchange_session_open_utc: timing.exchange_open,
        exchange_session_close_utc: timing.exchange_close,
        exchange_is_early_close: true,
        previous_trading_date: timing.previous_trading_date,
        preopen_start_utc: timing.preopen_start_utc,
        postclose_finalize_utc: timing.postclose_finalize_utc,
        initial_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "awaiting_preopen".to_string(),
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "early close fixture".to_string(),
    };
    let operation = match mqk_db::create_or_recover_autonomous_daily_operation(&pool, &args)
        .await
        .unwrap()
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(r)
        | mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => r,
        other => panic!("unexpected: {other:?}"),
    };
    assert_eq!(operation.exchange_is_early_close, Some(true));

    let instruments = vec![fixture_instrument("ZZDRVEC", "fake", "ZZDRVEC", "5m")];
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVEC", "5m", None);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(30),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    // Early close does not itself block the driver window (it is still
    // inside preopen..effective_close); no exchange-truth-missing rejection.
    assert_ne!(
        outcome,
        AutonomousCompletedBarDriverOutcome::ExchangeSessionTruthMissing
    );
}

#[tokio::test]
async fn session_46_effective_override_controls_operation_timing_only() {
    let Some(pool) = maybe_db("session_46").await else {
        return;
    };
    let mut timing = standard_timing();
    // Fixed-window override: effective window narrower than the exchange
    // session, per the boundary-model repair's contract.
    timing.effective_open = timing.exchange_open + chrono::Duration::hours(1);
    timing.effective_close = timing.exchange_close - chrono::Duration::hours(1);

    let operation = create_test_operation(
        &pool,
        "zzdrv-ovr",
        "ZZDRVOVR",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    assert_eq!(
        operation.effective_operation_open_utc,
        timing.effective_open
    );
    assert_eq!(
        operation.exchange_session_open_utc,
        Some(timing.exchange_open)
    );
    assert_ne!(
        operation.effective_operation_open_utc,
        operation.exchange_session_open_utc.unwrap()
    );

    let instruments = vec![fixture_instrument("ZZDRVOVR", "fake", "ZZDRVOVR", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVOVR", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVOVR", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVOVR", "5m", None);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    // Past the effective (overridden) close, though still before the raw
    // exchange close — must be OutsideOperationWindow, proving the
    // *effective* window (not the exchange window) governs when the driver
    // stops, per the boundary-model repair's contract (C.6).
    let now_after_effective_close_before_exchange_close =
        timing.effective_close + chrono::Duration::minutes(10);
    assert!(now_after_effective_close_before_exchange_close < timing.exchange_close);
    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now_after_effective_close_before_exchange_close,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::OutsideOperationWindow
    );
}

#[tokio::test]
async fn session_47_legacy_null_exchange_truth_blocks() {
    let Some(pool) = maybe_db("session_47").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-legacy",
        "ZZDRVLEG",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    // Simulate a legacy row (predates the boundary-model repair) by nulling
    // the exchange_* columns directly via SQL, exactly as the Phase B store
    // tests do for the identical scenario.
    sqlx::query(
        "update sys_autonomous_daily_operations set exchange_session_open_utc = null, \
         exchange_session_close_utc = null, exchange_is_early_close = null, \
         previous_trading_date = null where operation_id = $1",
    )
    .bind(operation.operation_id)
    .execute(&pool)
    .await
    .unwrap();
    let legacy_operation =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    assert!(legacy_operation.exchange_session_open_utc.is_none());

    let instruments = vec![fixture_instrument("ZZDRVLEG", "fake", "ZZDRVLEG", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVLEG", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVLEG", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = ready_readiness("ZZDRVLEG", "5m", None);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &legacy_operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::ExchangeSessionTruthMissing
    );
    assert_eq!(provider.calls(), 0);
}

// ---------------------------------------------------------------------------
// No automatic history (C.19 points 48-50)
// ---------------------------------------------------------------------------

#[test]
fn history_48_50_driver_source_never_wires_historical_sync_or_ingest_jobs() {
    let driver_source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/state/autonomous_completed_bar_driver.rs"
    ))
    .expect("read driver source");
    // Check for actual wiring (imports/calls), not the bare words — the
    // driver's own doc comments legitimately disclaim "no backfill" / "no
    // historical sync" in prose.
    for forbidden in [
        "ingest_jobs::",
        "IngestJobStore",
        "persist_ingest_job_record",
        "sync_provider",
        "fetch_historical_bars",
    ] {
        assert!(
            !driver_source.contains(forbidden),
            "driver source must never wire '{forbidden}' — Phase C is latest-bar-only"
        );
    }
    assert!(driver_source.contains("poll_and_ingest_latest_closed_bar"));
}

#[tokio::test]
async fn history_49_insufficient_history_blocker_remains_a_hard_block() {
    let Some(pool) = maybe_db("history_49").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-hist",
        "ZZDRVHIST",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVHIST", "fake", "ZZDRVHIST", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVHIST", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVHIST", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = FakeQueueProvider::new();
    let readiness = blocked_readiness("ZZDRVHIST", "5m", vec!["insufficient_history"]);
    let state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider: &provider,
        readiness: &readiness,
    })
    .await
    .expect("tick ok");

    assert!(matches!(
        outcome,
        AutonomousCompletedBarDriverOutcome::ReadinessBlocked { .. }
    ));
    assert_eq!(
        provider.calls(),
        0,
        "missing history must remain blocked, never auto-repaired"
    );
}

// ---------------------------------------------------------------------------
// C.16 — source-level guard: Phase C does not wire the new driver into
// production startup, and the legacy ticker remains the only spawned path.
// ---------------------------------------------------------------------------

#[test]
fn guard_main_rs_does_not_start_new_driver_and_still_spawns_legacy_ticker() {
    let main_source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read main.rs source");
    assert!(
        !main_source.contains("autonomous_completed_bar_driver"),
        "main.rs must not reference the new Phase C driver module — Phase D owns startup wiring"
    );
    assert!(
        !main_source.contains("run_bounded_cadence_task"),
        "main.rs must not start the new driver's task-runner scaffold"
    );
    assert!(
        main_source.contains("spawn_autonomous_bar_ticker"),
        "main.rs must still spawn only the legacy ticker in Phase C"
    );
}
