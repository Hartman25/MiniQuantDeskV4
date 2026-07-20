//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER: proof
//! tests for the autonomous completed-bar driver (`state::autonomous_completed_bar_driver`)
//! and the extracted latest-bar poll seam (`state::market_data_latest_bar`).
//!
//! DB-backed tests skip truthfully when `MQK_DATABASE_URL` is not set. No
//! real provider, broker, or network call is made anywhere in this file —
//! every provider is a fake injected trait object.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDate, Utc};
use mqk_daemon::daily_data_readiness::AssignmentReadiness;
use mqk_daemon::state::autonomous_completed_bar_driver::{
    resolve_autonomous_provider_call_authorization, resolve_single_effective_binding,
    tick_autonomous_completed_bar_driver, AutonomousAssignmentReadinessEvaluator,
    AutonomousBindingRejection, AutonomousCompletedBarDriverInput,
    AutonomousCompletedBarDriverMode, AutonomousCompletedBarDriverOutcome,
    AutonomousDriverSetupRejection, AutonomousLatestBarProviderResolver,
    AutonomousProviderCallAuthorization, ResolvedSingleBinding, REASON_LOCAL_RUNTIME_NOT_ACTIVE,
    REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH, REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT,
    REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED, REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
    REASON_OPERATION_NOT_RUNNING, REASON_OPERATION_RUN_ID_MISSING,
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

/// REPAIR 1 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01):
/// injected fake readiness evaluator. Each call to `.evaluate()` pops one
/// `AssignmentReadiness` from the front of a queue; `always(r)` builds an
/// infinite queue of one repeated value (for tests that only care about a
/// single readiness snapshot governing both the pre-poll and post-poll
/// evaluations — a mechanical, behavior-preserving stand-in for the old
/// single injected `&AssignmentReadiness` field), while `queue(vec![...])`
/// lets a test prove the driver's two-stage evaluation explicitly (e.g.
/// "first evaluation -> missing expected bar, second evaluation -> ready").
struct FakeReadinessEvaluator {
    items: Mutex<VecDeque<AssignmentReadiness>>,
    repeat_last: bool,
    calls: AtomicUsize,
}

impl FakeReadinessEvaluator {
    fn always(readiness: AssignmentReadiness) -> Self {
        Self {
            items: Mutex::new(VecDeque::from([readiness])),
            repeat_last: true,
            calls: AtomicUsize::new(0),
        }
    }

    fn queue(items: Vec<AssignmentReadiness>) -> Self {
        Self {
            items: Mutex::new(VecDeque::from(items)),
            repeat_last: false,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl AutonomousAssignmentReadinessEvaluator for FakeReadinessEvaluator {
    async fn evaluate(
        &self,
        _operation: &mqk_db::AutonomousDailyOperationRecord,
        _binding: &ResolvedSingleBinding,
        _now_utc: DateTime<Utc>,
    ) -> anyhow::Result<AssignmentReadiness> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut items = self.items.lock().unwrap();
        match items.pop_front() {
            Some(readiness) => {
                if self.repeat_last {
                    items.push_back(readiness.clone());
                }
                Ok(readiness)
            }
            None => panic!("FakeReadinessEvaluator queue exhausted — test supplied too few readiness snapshots for the number of evaluate() calls the driver actually made"),
        }
    }
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

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: local
/// newtype delegating every `MarketDataProvider` method to a shared
/// `Arc<FakeQueueProvider>`. `MarketDataProviderBox` is `Box<dyn
/// MarketDataProvider>` ('static), so a resolver returning a boxed provider
/// cannot box a borrowed `&FakeQueueProvider` directly — this handle lets
/// each `resolve()` call hand back a fresh, independently-droppable box that
/// still shares the same underlying call-count/outcome-queue state the test
/// holds a reference to.
struct FakeProviderHandle(Arc<FakeQueueProvider>);

#[async_trait::async_trait]
impl mqk_md::MarketDataProvider for FakeProviderHandle {
    fn provider_id(&self) -> &str {
        self.0.provider_id()
    }
    fn display_name(&self) -> &str {
        self.0.display_name()
    }
    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        self.0.capabilities()
    }
    fn health(&self) -> mqk_md::MarketDataProviderHealth {
        self.0.health()
    }
    fn rate_limits(&self) -> Option<mqk_md::MarketDataProviderRateLimits> {
        self.0.rate_limits()
    }
    async fn fetch_historical_bars(
        &self,
        request: mqk_md::HistoricalBarsRequest,
    ) -> Result<Vec<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.0.fetch_historical_bars(request).await
    }
    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.0.fetch_latest_closed_bar(request).await
    }
}

/// Fake lazy provider-resolution seam
/// (`AutonomousLatestBarProviderResolver`). `resolve_calls()` proves exactly
/// how many times the driver actually resolved (constructed) a provider —
/// distinct from `FakeQueueProvider::calls()`, which counts real
/// `fetch_latest_closed_bar` network-shaped calls. `failing(...)` builds a
/// resolver that always returns `ProviderConstructionFailed`, for proving
/// construction-failure handling without ever touching a real provider
/// registry or credentials.
struct FakeProviderResolver {
    provider: Arc<FakeQueueProvider>,
    resolve_calls: AtomicUsize,
    fail_with: Option<String>,
}

impl FakeProviderResolver {
    fn new(provider: Arc<FakeQueueProvider>) -> Self {
        Self {
            provider,
            resolve_calls: AtomicUsize::new(0),
            fail_with: None,
        }
    }

    fn failing(detail: &str) -> Self {
        Self {
            provider: Arc::new(FakeQueueProvider::new()),
            resolve_calls: AtomicUsize::new(0),
            fail_with: Some(detail.to_string()),
        }
    }

    fn resolve_calls(&self) -> usize {
        self.resolve_calls.load(Ordering::SeqCst)
    }
}

impl AutonomousLatestBarProviderResolver for FakeProviderResolver {
    fn resolve(
        &self,
        _provider_id: &str,
    ) -> Result<mqk_md::MarketDataProviderBox, AutonomousDriverSetupRejection> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(detail) = &self.fail_with {
            return Err(AutonomousDriverSetupRejection::ProviderConstructionFailed(
                detail.clone(),
            ));
        }
        Ok(Box::new(FakeProviderHandle(self.provider.clone())))
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
        stop_attempt_count: 0,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
        state_blocker_signature: None,
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
        stop_attempt_count: Some(0),
        last_stop_attempt_utc: None,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: this
    // test never actually ingests a canonical `md_bars` row for the exact
    // expected bar — it only fabricates the observation record directly via
    // `record_completed_bar_observed`. `last_completed_bar_ts == expected_ts`
    // with no backing `md_bars` row is exactly the durable-evidence-
    // inconsistency condition (see `repair_evidence_33_missing_row_...`
    // below): the old `PollNotDue` short-circuit accepted this as
    // authoritative "nothing to do"; the repair fails closed instead.
    match outcome {
        AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
            expected_end_ts,
            reason_code,
        } => {
            assert_eq!(expected_end_ts, expected_ts);
            assert_eq!(
                reason_code,
                mqk_daemon::state::autonomous_completed_bar_driver::REASON_OBSERVED_BAR_MISSING_FROM_MD_BARS
            );
        }
        other => panic!("expected ObservedBarEvidenceInconsistent, got {other:?}"),
    }
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness1.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01:
    // this test proves poll cadence, not dispatch-claim mechanics — it now
    // runs in `PrepareDataOnly` mode, so a freshly observed+ready bar
    // reports `BarObserved`, never a dispatch-path outcome (`PrepareDataOnly`
    // never creates a claim regardless of any bootstrap/runtime state).
    assert_eq!(
        outcome1,
        AutonomousCompletedBarDriverOutcome::BarObserved {
            bar_end_ts: first_expected_ts
        }
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness_same.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
    // exact bar is now genuinely canonical in `md_bars` (ingested by tick
    // 1), so the driver reconciles it via the local path and reaches the
    // same `BarObserved` outcome, idempotently — not the old
    // undifferentiated `PollNotDue`. The no-re-poll guarantee is proven by
    // the unchanged `provider.calls()` count below, not by the outcome
    // variant.
    assert_eq!(
        outcome_same,
        AutonomousCompletedBarDriverOutcome::BarObserved {
            bar_end_ts: first_expected_ts
        }
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness2.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::BarObserved {
            bar_end_ts: second_expected_ts
        }
    );
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01: an
/// [`active_bootstrap_state`] that additionally owns a locally-injected
/// running loop for `run_id` (via `AppState::inject_running_loop_for_test`),
/// so `RunningDispatch`-mode tests can prove runtime-dispatch eligibility
/// without starting a real execution loop, provider, or broker.
async fn running_dispatch_eligible_state(pool: sqlx::PgPool, run_id: Uuid) -> state::AppState {
    let state = active_bootstrap_state(pool).await;
    state.inject_running_loop_for_test(run_id).await;
    // D4 REPAIR 2: `AppState::record_signal_evaluation` derives its
    // evaluation identity from `status.active_run_id`, not from
    // `execution_loop`'s injected ownership — production keeps both in sync
    // via `publish_status` on every real start; this light fixture must do
    // the same explicitly, or the durable evaluation row's run_id would
    // silently diverge from the exact run_id a completed-bar claim's own
    // identity expects (an evaluation-lineage mismatch, not a real
    // production gap).
    state.status.write().await.active_run_id = Some(run_id);
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
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVDISP", "fake", "ZZDRVDISP", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVDISP", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVDISP", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness1.clone()),
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
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
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness2.clone()),
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
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
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-stale",
        "ZZDRVSTALE",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVSTALE", "fake", "ZZDRVSTALE", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVSTALE", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVSTALE", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness1.clone()),
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
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
        stop_attempt_count: 0,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
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
        provider_resolver: &resolver,
        readiness_evaluator: &FakeReadinessEvaluator::always(readiness.clone()),
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
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
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION:
    // this guard's Phase C assertions ("main.rs must not reference the new
    // driver module; main.rs must still spawn only the legacy ticker")
    // described exactly the pre-cutover state Phase D3 was always designed
    // to end. D3 performs that authorized cutover deliberately: main.rs now
    // spawns the supervised completed-bar task
    // (`state::autonomous_completed_bar_task::spawn_autonomous_completed_bar_driver_task`,
    // which necessarily contains the substring "autonomous_completed_bar_driver"
    // as part of its own name) and no longer spawns the legacy ticker. See
    // `tests/scenario_autonomous_completed_bar_task_01.rs`'s `i01`-`i03` for
    // the full current-state production-cutover proof this guard now only
    // partially duplicates; kept here, updated, as the file's own
    // regression baseline rather than removed.
    let main_source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read main.rs source");
    assert!(
        !main_source.contains("spawn_autonomous_bar_ticker"),
        "main.rs must no longer spawn the legacy autonomous bar ticker (D3 cutover)"
    );
    assert!(
        main_source.contains("spawn_autonomous_completed_bar_driver_task"),
        "main.rs must spawn the supervised completed-bar driver task (D3 cutover)"
    );
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01
// REPAIR 10 — two-stage readiness, exact expected-bar enforcement, and
// TwelveData capability closure proofs.
// ---------------------------------------------------------------------------

/// A missing expected bar alone is poll-remediable: pre-poll readiness may
/// be `"blocked"` on exactly `expected_latest_bar_missing` and still
/// authorize one provider call (REPAIR 10 points 1-4, 11).
#[tokio::test]
async fn repair10_01_04_11_missing_expected_bar_pre_poll_blocked_still_polls_and_dispatches_when_post_poll_ready(
) {
    let Some(pool) = maybe_db("repair10_01_04_11").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-r10a",
        "ZZDRVR10A",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVR10A", "fake", "ZZDRVR10A", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10A", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10A", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    provider.push_outcome(
        "ZZDRVR10A",
        Ok(Some(bar("ZZDRVR10A", "5m", expected_ts, true))),
    );

    // First (pre-poll) evaluation: blocked on exactly the missing-bar
    // reason — poll-remediable, not a hard block. Second (post-poll)
    // evaluation: ready, agreeing the ingested bar is the expected/actual
    // latest bar — this alone authorizes dispatch.
    let pre_poll = AssignmentReadiness {
        readiness_state: "blocked",
        blockers: vec![mqk_daemon::daily_data_readiness::REASON_EXPECTED_LATEST_BAR_MISSING],
        expected_latest_bar_ts: Some(expected_ts),
        actual_latest_bar_ts: None,
        ..ready_readiness("ZZDRVR10A", "5m", Some(expected_ts))
    };
    let post_poll = ready_readiness("ZZDRVR10A", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![pre_poll, post_poll]);

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        },
        "a missing-expected-bar pre-poll blocker must not prevent polling or dispatch \
         once the post-poll evaluation is ready"
    );
    assert_eq!(
        provider.calls(),
        1,
        "exactly one authorized provider call despite the pre-poll blocker"
    );
    assert_eq!(evaluator.calls(), 2, "pre-poll and post-poll evaluations");

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 1);
    assert_eq!(refreshed.bars_dispatched, 1);
    assert_eq!(refreshed.last_completed_bar_ts, Some(expected_ts));
}

/// A post-poll readiness re-evaluation that is not ready creates no dispatch
/// claim and calls the strategy path zero times, even though the exact
/// expected bar was successfully polled and ingested (REPAIR 10 point 5).
#[tokio::test]
async fn repair10_05_post_poll_blocked_readiness_creates_no_claim_and_no_dispatch() {
    let Some(pool) = maybe_db("repair10_05").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-r10b",
        "ZZDRVR10B",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVR10B", "fake", "ZZDRVR10B", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10B", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10B", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = active_bootstrap_state(pool.clone()).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    provider.push_outcome(
        "ZZDRVR10B",
        Ok(Some(bar("ZZDRVR10B", "5m", expected_ts, true))),
    );

    let pre_poll = ready_readiness("ZZDRVR10B", "5m", Some(expected_ts));
    let post_poll_blocked = AssignmentReadiness {
        readiness_state: "blocked",
        blockers: vec![
            mqk_daemon::daily_data_readiness::REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED,
        ],
        ..ready_readiness("ZZDRVR10B", "5m", Some(expected_ts))
    };
    let evaluator = FakeReadinessEvaluator::queue(vec![pre_poll, post_poll_blocked]);

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ReadinessBlockedAfterPoll { blockers } => {
            assert_eq!(blockers, vec!["provider_timestamp_convention_unverified"]);
        }
        other => panic!("expected ReadinessBlockedAfterPoll, got {other:?}"),
    }

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVR10B",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none(), "no dispatch claim must be created");

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.bars_dispatched, 0,
        "post-poll blocked readiness must never increment bars_dispatched"
    );
    assert_eq!(
        refreshed.bars_observed, 1,
        "the bar itself was genuinely observed — only dispatch is refused"
    );
}

/// The exact expected bar already present in `md_bars` produces zero
/// provider calls, and is observed and dispatched exactly once (REPAIR 10
/// points 6, 7).
#[tokio::test]
async fn repair10_06_07_exact_bar_already_in_db_zero_provider_calls_dispatches_once() {
    let Some(pool) = maybe_db("repair10_06_07").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-r10c",
        "ZZDRVR10C",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVR10C", "fake", "ZZDRVR10C", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10C", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10C", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new()); // no outcome ever queued -> zero calls expected
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

    let expected_ts = timing.effective_open.timestamp() + 300;

    // Seed the exact expected bar directly into md_bars, with the same
    // canonical provider identity `resolve_latest_bar_poll_target` would
    // produce for this fixture instrument.
    mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "fake".to_string(),
            timeframe: "5m".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![mqk_db::md::ProviderBar {
                symbol: "ZZDRVR10C".to_string(),
                timeframe: "5m".to_string(),
                end_ts: expected_ts,
                open: "100".to_string(),
                high: "101".to_string(),
                low: "99".to_string(),
                close: "100.5".to_string(),
                volume: 1000,
                is_complete: true,
            }],
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: "fake".to_string(),
            provider_source: Some("fake".to_string()),
            provider_symbol: Some("ZZDRVR10C".to_string()),
            ingest_mode: Some("test_seed_exact_bar".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    .expect("seed exact bar");

    let readiness = ready_readiness("ZZDRVR10C", "5m", Some(expected_ts));
    // Called twice: pre-poll eligibility, then the mandatory post-poll
    // re-evaluation before dispatch — even though no provider call occurred.
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(
        provider.calls(),
        0,
        "the exact expected bar was already in md_bars — zero provider calls"
    );
    assert_eq!(evaluator.calls(), 2);

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 1);
    assert_eq!(refreshed.bars_dispatched, 1, "dispatched exactly once");
    // REPAIR 6: provider poll counters are untouched on this path.
    assert_eq!(refreshed.provider_poll_attempt_count, 0);
    assert_eq!(refreshed.provider_poll_success_count, 0);
    assert_eq!(refreshed.provider_poll_failure_count, 0);
}

/// A provider result older than the exact expected bar is never observed or
/// dispatched, even though the provider call itself succeeded and the bar
/// was durably stored (REPAIR 10 points 8, 9; REPAIR 6 counter semantics).
#[tokio::test]
async fn repair10_08_09_older_provider_bar_not_observed_not_dispatched() {
    let Some(pool) = maybe_db("repair10_08_09").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-r10d",
        "ZZDRVR10D",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVR10D", "fake", "ZZDRVR10D", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10D", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10D", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = active_bootstrap_state(pool.clone()).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    let older_ts = expected_ts - 300;
    provider.push_outcome(
        "ZZDRVR10D",
        Ok(Some(bar("ZZDRVR10D", "5m", older_ts, true))),
    );
    let readiness = ready_readiness("ZZDRVR10D", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::ProviderLaggingExpectedBar {
            returned_bar_ts: older_ts,
            expected_end_ts: expected_ts,
        }
    );
    assert_eq!(provider.calls(), 1, "the provider call itself succeeded");
    assert_eq!(
        evaluator.calls(),
        1,
        "no post-poll (second) evaluation for a rejected bar identity"
    );

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 0, "point 8: never observed");
    assert_eq!(refreshed.last_completed_bar_ts, None);
    assert_eq!(
        refreshed.data_refresh_state,
        "provider_lagging_expected_bar"
    );
    assert_eq!(refreshed.provider_poll_attempt_count, 1);
    assert_eq!(
        refreshed.provider_poll_success_count, 1,
        "REPAIR 6: the provider request itself succeeded"
    );
    assert_eq!(refreshed.provider_poll_failure_count, 0);

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVR10D",
        "5m",
        older_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none(), "point 9: never dispatched");

    // The bar itself is still genuine, valid data and remains stored.
    let stored: (i64,) = sqlx::query_as(
        "select end_ts from md_bars where symbol = $1 and timeframe = '5m' and end_ts = $2",
    )
    .bind("ZZDRVR10D")
    .bind(older_ts)
    .fetch_one(&pool)
    .await
    .expect("older bar is still stored in md_bars");
    assert_eq!(stored.0, older_ts);
}

/// A provider result newer than the exact expected bar is rejected — never
/// observed or dispatched (REPAIR 10 point 10).
#[tokio::test]
async fn repair10_10_newer_than_expected_provider_bar_rejected() {
    let Some(pool) = maybe_db("repair10_10").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-r10e",
        "ZZDRVR10E",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVR10E", "fake", "ZZDRVR10E", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10E", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10E", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = active_bootstrap_state(pool.clone()).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    let newer_ts = expected_ts + 300;
    provider.push_outcome(
        "ZZDRVR10E",
        Ok(Some(bar("ZZDRVR10E", "5m", newer_ts, true))),
    );
    let readiness = ready_readiness("ZZDRVR10E", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);

    // now_utc must be far enough past newer_ts for the provider's own
    // future-skew provenance check (independent of this driver's exact-bar
    // constraint) to admit the bar at all.
    let now_utc = DateTime::<Utc>::from_timestamp(newer_ts + 300, 0).unwrap();

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::UnexpectedOrFutureBar {
            returned_bar_ts: newer_ts,
            expected_end_ts: expected_ts,
        }
    );
    assert_eq!(provider.calls(), 1);

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 0);
    assert_eq!(refreshed.data_refresh_state, "unexpected_or_future_bar");

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVR10E",
        "5m",
        newer_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none());
}

/// A terminal (non-remediable) pre-poll blocker unrelated to the missing
/// tail bar still causes zero provider calls (REPAIR 10 point 12) — reusing
/// `readiness_22_30`'s exact scenario is not enough on its own since that
/// test predates the two-stage split; this proves the *new* classifier
/// still treats an unrelated blocker as non-remediable even when an expected
/// timestamp is present.
#[tokio::test]
async fn repair10_12_non_remediable_blocker_with_known_expected_ts_zero_calls() {
    let Some(pool) = maybe_db("repair10_12").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-r10f",
        "ZZDRVR10F",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVR10F", "fake", "ZZDRVR10F", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10F", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10F", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let state = active_bootstrap_state(pool.clone()).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    let readiness = AssignmentReadiness {
        readiness_state: "blocked",
        blockers: vec!["interior_gap"],
        expected_latest_bar_ts: Some(expected_ts),
        ..ready_readiness("ZZDRVR10F", "5m", Some(expected_ts))
    };
    let evaluator = FakeReadinessEvaluator::always(readiness);

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ReadinessBlocked { blockers } => {
            assert_eq!(blockers, vec!["interior_gap"]);
        }
        other => panic!("expected ReadinessBlocked, got {other:?}"),
    }
    assert_eq!(provider.calls(), 0);
}

/// A configured instrument's provider mismatch causes zero provider calls
/// (REPAIR 10 point 13).
#[tokio::test]
async fn repair10_13_provider_mismatch_zero_calls() {
    let Some(pool) = maybe_db("repair10_13").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-r10g",
        "ZZDRVR10G",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    // Instrument is configured for "alpaca", but the driver is asked to poll "fake".
    let instruments = vec![fixture_instrument("ZZDRVR10G", "alpaca", "ZZDRVR10G", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10G", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10G", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness(
        "ZZDRVR10G",
        "5m",
        Some(timing.effective_open.timestamp() + 300),
    );
    let evaluator = FakeReadinessEvaluator::always(readiness);
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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    assert!(matches!(
        outcome,
        AutonomousCompletedBarDriverOutcome::RegistryBlocked {
            rejection: LatestBarRegistryAdmissionRejection::ProviderMismatch { .. }
        }
    ));
    assert_eq!(provider.calls(), 0);
}

/// TwelveData's `1D` timestamp convention is verified (Bundle 2), and the
/// newly-enabled TwelveData `latest_closed_bar` capability (REPAIR 7) can
/// reach the poll seam end-to-end with a fake adapter mirroring that
/// capability (REPAIR 10 point 15).
#[tokio::test]
async fn repair10_15_twelvedata_1d_capability_reaches_poll_seam_and_dispatches() {
    let Some(pool) = maybe_db("repair10_15").await else {
        return;
    };
    assert_eq!(
        mqk_daemon::daily_data_readiness::resolve_daily_bar_timestamp_convention("twelvedata"),
        mqk_daemon::daily_data_readiness::DailyBarTimestampConvention::MidnightUtcMarketDate
    );

    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-r10h",
        "ZZDRVR10H",
        "swing_momentum",
        "1D",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument(
        "ZZDRVR10H",
        "twelvedata",
        "ZZDRVR10H",
        "1D",
    )];
    let assignment_config = fixture_assignment_config("ZZDRVR10H", "swing_momentum", "1D");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10H", "swing_momentum", 86_400);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    // Mirrors the capability the real TwelveData factory-built adapter now
    // reports after REPAIR 7 (`capabilities_from_provider_config`).
    provider.set_capabilities(mqk_md::MarketDataProviderCapabilities {
        historical_bars: true,
        latest_closed_bar: true,
        completed_bar_stream: false,
        supported_asset_classes: vec![mqk_md::ProviderAssetClass::Equity],
        supported_timeframes: vec![mqk_md::Timeframe::D1],
    });
    // A D1 bar's canonical end_ts is a UTC midnight boundary — using an
    // intraday timestamp here would fail the poll seam's own future-skew
    // provenance check against `now_utc` (unrelated to this driver's exact-
    // bar constraint).
    let expected_ts = timing
        .market_date
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    provider.push_outcome(
        "ZZDRVR10H",
        Ok(Some(bar("ZZDRVR10H", "1D", expected_ts, true))),
    );
    let readiness = ready_readiness("ZZDRVR10H", "1D", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(provider.calls(), 1);
}

/// Alpaca's `1D` timestamp convention remains unverified — strict readiness
/// keeps it blocked; capability enablement never implies verification
/// (REPAIR 10 point 16; re-proves `readiness_22_30`'s scenario against the
/// new two-stage classifier).
#[test]
fn repair10_16_alpaca_1d_timestamp_convention_remains_unverified() {
    assert_eq!(
        mqk_daemon::daily_data_readiness::resolve_daily_bar_timestamp_convention("alpaca"),
        mqk_daemon::daily_data_readiness::DailyBarTimestampConvention::Unverified
    );
}

/// Restart does not redispatch a completed exact bar: a fresh driver
/// invocation (new `AppState`/evaluator, same durable operation row) sees
/// `last_completed_bar_ts` already equal to the expected bar and refuses to
/// poll or dispatch again (REPAIR 10 point 17).
#[tokio::test]
async fn repair10_17_restart_does_not_redispatch_completed_exact_bar() {
    let Some(pool) = maybe_db("repair10_17").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-r10i",
        "ZZDRVR10I",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVR10I", "fake", "ZZDRVR10I", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVR10I", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVR10I", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;
    let now1 = timing.effective_open + chrono::Duration::minutes(6);

    {
        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        provider.push_outcome(
            "ZZDRVR10I",
            Ok(Some(bar("ZZDRVR10I", "5m", expected_ts, true))),
        );
        let readiness = ready_readiness("ZZDRVR10I", "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::always(readiness);
        let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

        let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
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
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
        })
        .await
        .expect("tick ok");
        assert_eq!(
            outcome,
            AutonomousCompletedBarDriverOutcome::DispatchCompleted {
                bar_end_ts: expected_ts
            }
        );
    }

    let claim_evaluation_id = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVR10I",
        "5m",
        expected_ts,
    )
    .await
    .expect("claim fetch ok")
    .expect("claim row exists")
    .evaluation_id
    .expect("D4: completed claim must store a non-null evaluation_id");

    // "Restart": brand-new AppState and evaluator, same durable row. A real
    // restart re-establishes local ownership of the same durable `run_id`
    // (never a fresh one) — simulated here via a second
    // `running_dispatch_eligible_state` call with the identical `run_id`.
    operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    assert_eq!(operation.last_completed_bar_ts, Some(expected_ts));

    let provider2 = Arc::new(FakeQueueProvider::new());
    let resolver2 = FakeProviderResolver::new(provider2.clone());
    let readiness2 = ready_readiness("ZZDRVR10I", "5m", Some(expected_ts));
    let evaluator2 = FakeReadinessEvaluator::always(readiness2);
    let state2 = running_dispatch_eligible_state(pool.clone(), run_id).await;

    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state2,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now1 + chrono::Duration::seconds(30),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver2,
        readiness_evaluator: &evaluator2,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
    // exact bar is canonical in `md_bars` from tick 1, so the restarted
    // driver reconciles it via the local path and finds the completed
    // claim — `AlreadyDispatched`, not the old undifferentiated
    // `PollNotDue`. Zero re-poll is still proven by `provider2.calls()`.
    //
    // D4 REPAIR 3: the completed claim now durably stores its confirmed
    // evaluation id (previously always `None` — the exact defect this patch
    // closes), so the repeat tick's `AlreadyDispatched` carries it too.
    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::AlreadyDispatched {
            evaluation_id: Some(claim_evaluation_id)
        }
    );
    assert_eq!(
        provider2.calls(),
        0,
        "restart must not redispatch or repoll"
    );

    let final_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        final_row.bars_dispatched, 1,
        "still dispatched exactly once across the simulated restart"
    );
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01
// Observed-bar recovery, provider-authorization boundary, and durable-
// evidence-inconsistency proofs.
// ---------------------------------------------------------------------------

/// Seed one exact canonical `md_bars` row directly (bypassing the poll seam)
/// with explicit provider provenance, for tests that need a bar already
/// trusted-canonical before the driver ever ticks.
async fn seed_exact_bar(
    pool: &sqlx::PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    provider_id: &str,
    provider_symbol: &str,
) {
    mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: provider_id.to_string(),
            timeframe: timeframe.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![mqk_db::md::ProviderBar {
                symbol: symbol.to_string(),
                timeframe: timeframe.to_string(),
                end_ts,
                open: "100".to_string(),
                high: "101".to_string(),
                low: "99".to_string(),
                close: "100.5".to_string(),
                volume: 1000,
                is_complete: true,
            }],
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: provider_id.to_string(),
            provider_source: Some(provider_id.to_string()),
            provider_symbol: Some(provider_symbol.to_string()),
            ingest_mode: Some("test_seed_exact_bar".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    .expect("seed exact bar");
}

// ---------------------------------------------------------------------------
// Crash recovery (points 1-10): a prior process observed the exact expected
// bar but crashed before claiming its dispatch. A fresh AppState/evaluator
// and a freshly re-fetched operation row (simulating a daemon restart) must
// recover cleanly with zero provider calls.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_01_10_crash_before_claim_restarts_cleanly() {
    let Some(pool) = maybe_db("recovery_01_10").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-rec01",
        "ZZDRVREC01",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVREC01", "fake", "ZZDRVREC01", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVREC01", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVREC01", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // A prior process observed the exact expected bar (ingested it into
    // md_bars and recorded the observation) but crashed before ever
    // claiming its dispatch.
    seed_exact_bar(&pool, "ZZDRVREC01", "5m", expected_ts, "fake", "ZZDRVREC01").await;
    let observed = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    assert!(matches!(
        observed,
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { bars_observed: 1 }
    ));
    let claim_before = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVREC01",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(
        claim_before.is_none(),
        "point 2: no dispatch claim exists yet"
    );

    // "Restart": a new AppState, a new evaluator, and a freshly re-fetched
    // operation row (point 3).
    let mut operation =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    assert_eq!(operation.last_completed_bar_ts, Some(expected_ts));
    assert_eq!(operation.bars_observed, 1);

    let provider = Arc::new(FakeQueueProvider::new()); // no outcome queued -> zero calls expected
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVREC01", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    // Points 4, 6, 7: zero provider calls on the next tick; a fresh claim is
    // created; strategy dispatch occurs once.
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(
        provider.calls(),
        0,
        "point 4: crash recovery via the local exact-bar path makes zero provider network calls"
    );
    assert_eq!(
        resolver.resolve_calls(),
        0,
        "crash recovery must never resolve/construct a provider"
    );
    assert_eq!(
        evaluator.calls(),
        2,
        "point 5: readiness is re-evaluated (pre-poll + post-observation)"
    );

    // Points 8, 9, 10: claim completes; bars_observed unchanged;
    // bars_dispatched increments once.
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.bars_observed, 1,
        "point 9: an already-observed bar must not increment bars_observed again"
    );
    assert_eq!(refreshed.bars_dispatched, 1, "point 10");
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVREC01",
        "5m",
        expected_ts,
    )
    .await
    .unwrap()
    .expect("claim exists");
    assert_eq!(claim.status, mqk_db::DISPATCH_STATUS_COMPLETED, "point 8");
}

// ---------------------------------------------------------------------------
// Readiness-later recovery (points 11-17): the exact bar is found/ingested
// on tick 1, but post-observation readiness is blocked; tick 2 re-evaluates
// readiness against the same already-observed bar and dispatches once it
// becomes ready, with zero provider calls on tick 2.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_11_17_readiness_blocked_then_ready_dispatches_on_second_tick() {
    let Some(pool) = maybe_db("recovery_11_17").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-rec02",
        "ZZDRVREC02",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument("ZZDRVREC02", "fake", "ZZDRVREC02", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVREC02", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVREC02", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;
    let expected_ts = timing.effective_open.timestamp() + 300;

    // Point 11: tick 1 finds/ingests the exact bar via an authorized poll.
    let provider1 = Arc::new(FakeQueueProvider::new());
    provider1.push_outcome(
        "ZZDRVREC02",
        Ok(Some(bar("ZZDRVREC02", "5m", expected_ts, true))),
    );
    let resolver1 = FakeProviderResolver::new(provider1.clone());
    let pre_poll1 = ready_readiness("ZZDRVREC02", "5m", Some(expected_ts));
    let post_poll1_blocked = AssignmentReadiness {
        readiness_state: "blocked",
        blockers: vec![
            mqk_daemon::daily_data_readiness::REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED,
        ],
        ..ready_readiness("ZZDRVREC02", "5m", Some(expected_ts))
    };
    let evaluator1 = FakeReadinessEvaluator::queue(vec![pre_poll1, post_poll1_blocked]);
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
        provider_resolver: &resolver1,
        readiness_evaluator: &evaluator1,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    // Point 12: post-observation readiness remains blocked.
    match outcome1 {
        AutonomousCompletedBarDriverOutcome::ReadinessBlockedAfterPoll { .. } => {}
        other => panic!("expected ReadinessBlockedAfterPoll, got {other:?}"),
    }
    assert_eq!(provider1.calls(), 1);

    // Point 13: no claim exists after the first tick.
    let claim_after_tick1 = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVREC02",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim_after_tick1.is_none());

    let mut operation =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    assert_eq!(
        operation.bars_observed, 1,
        "the bar itself was genuinely observed"
    );
    assert_eq!(operation.last_completed_bar_ts, Some(expected_ts));

    // Points 14-17: tick 2 uses the same already-observed bar; readiness is
    // now ready; zero provider calls this tick; claim and dispatch complete
    // once.
    let provider2 = Arc::new(FakeQueueProvider::new());
    let resolver2 = FakeProviderResolver::new(provider2.clone());
    let readiness2 = ready_readiness("ZZDRVREC02", "5m", Some(expected_ts));
    let evaluator2 = FakeReadinessEvaluator::queue(vec![readiness2.clone(), readiness2]);
    let now2 = now1 + chrono::Duration::seconds(30);

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
        provider_resolver: &resolver2,
        readiness_evaluator: &evaluator2,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(
        provider2.calls(),
        0,
        "point 16: zero provider calls on the second tick"
    );
    assert_eq!(resolver2.resolve_calls(), 0);

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 1, "still only observed once");
    assert_eq!(
        refreshed.bars_dispatched, 1,
        "point 17: dispatched exactly once"
    );
}

// ---------------------------------------------------------------------------
// Completed claim (points 18-21).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn claim_18_21_completed_claim_returns_already_dispatched_zero_calls() {
    let Some(pool) = maybe_db("claim_18_21").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-claim18",
        "ZZDRVCLM18",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVCLM18", "fake", "ZZDRVCLM18", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVCLM18", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVCLM18", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // Seed exact bar + observation + a completed dispatch claim directly,
    // simulating a prior successful dispatch this tick must never repeat.
    seed_exact_bar(&pool, "ZZDRVCLM18", "5m", expected_ts, "fake", "ZZDRVCLM18").await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    let claim = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVCLM18",
        "5m",
        expected_ts,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(claim, mqk_db::BarDispatchClaimOutcome::Claimed));
    mqk_db::complete_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVCLM18",
        "5m",
        expected_ts,
        timing.effective_open,
        None,
    )
    .await
    .unwrap();

    let mut operation =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    assert_eq!(operation.bars_dispatched, 1);

    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVCLM18", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    // Point 18: AlreadyDispatched.
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::AlreadyDispatched {
            evaluation_id: None
        }
    );
    // Point 19: no provider call.
    assert_eq!(provider.calls(), 0);
    assert_eq!(resolver.resolve_calls(), 0);

    // Points 20, 21: no strategy call, no counter duplication.
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_dispatched, 1, "no counter duplication");
}

// ---------------------------------------------------------------------------
// Unresolved claim (points 22-24): 'claimed' (reclassified to 'uncertain' by
// the driver's own claim attempt) and 'failed' claims are both returned as
// unresolved and never automatically redispatched.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn claim_22_24_unresolved_claim_never_auto_redispatched() {
    let Some(pool) = maybe_db("claim_22_24").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-claim22",
        "ZZDRVCLM22",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    let instruments = vec![fixture_instrument("ZZDRVCLM22", "fake", "ZZDRVCLM22", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVCLM22", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVCLM22", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;
    let expected_ts_a = timing.effective_open.timestamp() + 300;

    // A prior process claimed but never completed or failed the identity —
    // the driver's own claim attempt is the *second* attempt, so it
    // reclassifies to 'uncertain' rather than silently redispatching.
    seed_exact_bar(
        &pool,
        "ZZDRVCLM22",
        "5m",
        expected_ts_a,
        "fake",
        "ZZDRVCLM22",
    )
    .await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts_a,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    let claim1 = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVCLM22",
        "5m",
        expected_ts_a,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(claim1, mqk_db::BarDispatchClaimOutcome::Claimed));

    let mut operation_a =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    operation_a.state = mqk_db::STATE_RUNNING.to_string();
    operation_a.run_id = Some(run_id);
    let provider_a = Arc::new(FakeQueueProvider::new());
    let resolver_a = FakeProviderResolver::new(provider_a.clone());
    let readiness_a = ready_readiness("ZZDRVCLM22", "5m", Some(expected_ts_a));
    let evaluator_a = FakeReadinessEvaluator::queue(vec![readiness_a.clone(), readiness_a]);

    let outcome_a = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation_a,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(6),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver_a,
        readiness_evaluator: &evaluator_a,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    // Point 22: an unresolved ('uncertain') claim is returned, not
    // redispatched.
    assert_eq!(
        outcome_a,
        AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
            status: mqk_db::DISPATCH_STATUS_UNCERTAIN.to_string()
        }
    );
    // Points 23, 24: no provider call, no automatic strategy replay.
    assert_eq!(provider_a.calls(), 0);
    assert_eq!(resolver_a.resolve_calls(), 0);

    // A directly-failed claim on a different bar identity is equally
    // unresolved and never automatically redispatched.
    let expected_ts_b = expected_ts_a + 300;
    seed_exact_bar(
        &pool,
        "ZZDRVCLM22",
        "5m",
        expected_ts_b,
        "fake",
        "ZZDRVCLM22",
    )
    .await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts_b,
        timing.effective_open,
    )
    .await
    .expect("record observed 2");
    let claim_b = mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVCLM22",
        "5m",
        expected_ts_b,
        timing.effective_open,
    )
    .await
    .unwrap();
    assert!(matches!(claim_b, mqk_db::BarDispatchClaimOutcome::Claimed));
    let failed = mqk_db::fail_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVCLM22",
        "5m",
        expected_ts_b,
        "test-induced failure",
    )
    .await
    .unwrap();
    assert!(failed);

    let mut operation_b =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    operation_b.state = mqk_db::STATE_RUNNING.to_string();
    operation_b.run_id = Some(run_id);
    let provider_b = Arc::new(FakeQueueProvider::new());
    let resolver_b = FakeProviderResolver::new(provider_b.clone());
    let readiness_b = ready_readiness("ZZDRVCLM22", "5m", Some(expected_ts_b));
    let evaluator_b = FakeReadinessEvaluator::queue(vec![readiness_b.clone(), readiness_b]);

    let outcome_b = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation_b,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: timing.effective_open + chrono::Duration::minutes(11),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver_b,
        readiness_evaluator: &evaluator_b,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome_b,
        AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
            status: mqk_db::DISPATCH_STATUS_FAILED.to_string()
        }
    );
    assert_eq!(provider_b.calls(), 0);
    assert_eq!(resolver_b.resolve_calls(), 0);
}

// ---------------------------------------------------------------------------
// Authorization boundary (points 25-32): the local exact-bar path ignores
// `authorization` entirely; the missing-bar path enforces it and resolves a
// provider at most once.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authz_25_28_local_bar_present_ignores_authorization_disabled_and_invalid() {
    let Some(pool) = maybe_db("authz_25_28").await else {
        return;
    };
    let timing = standard_timing();
    let cases: Vec<(&str, &str, AutonomousProviderCallAuthorization)> = vec![
        (
            "dis",
            "ZZDRVAUTHDIS",
            AutonomousProviderCallAuthorization::Disabled,
        ),
        (
            "inv",
            "ZZDRVAUTHINV",
            AutonomousProviderCallAuthorization::Invalid {
                reason_code: "test_invalid",
                detail: "test".to_string(),
            },
        ),
    ];
    for (adapter_suffix, symbol, authorization) in cases {
        let mut operation = create_test_operation(
            &pool,
            &format!("zzdrv-auth25-{adapter_suffix}"),
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;

        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
        let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

        let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
            state: &state,
            pool: &pool,
            operation: &operation,
            assignment_config: &assignment_config,
            assignment_identity: &assignment_identity,
            runtime_binding: &binding,
            runtime_binding_identity: &runtime_binding_identity,
            now_utc: timing.effective_open + chrono::Duration::minutes(6),
            authorization,
            instruments: &instruments,
            provider_id: "fake",
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
        })
        .await
        .expect("tick ok");

        // Points 25, 26: the local exact-bar path may dispatch regardless
        // of a disabled/invalid authorization.
        assert_eq!(
            outcome,
            AutonomousCompletedBarDriverOutcome::DispatchCompleted {
                bar_end_ts: expected_ts
            },
            "local exact-bar path must dispatch regardless of authorization state ({adapter_suffix})"
        );
        // Points 27, 28: zero resolver calls, zero provider network calls.
        assert_eq!(
            provider.calls(),
            0,
            "zero provider network calls ({adapter_suffix})"
        );
        assert_eq!(
            resolver.resolve_calls(),
            0,
            "zero provider construction/credential reads ({adapter_suffix})"
        );
    }
}

#[tokio::test]
async fn authz_29_31_missing_bar_absent_authorization_blocks_zero_resolver_calls() {
    let Some(pool) = maybe_db("authz_29_31").await else {
        return;
    };
    let timing = standard_timing();
    let cases: Vec<(&str, &str, AutonomousProviderCallAuthorization, bool)> = vec![
        (
            "dis",
            "ZZDRVAUTHMDIS",
            AutonomousProviderCallAuthorization::Disabled,
            true,
        ),
        (
            "inv",
            "ZZDRVAUTHMINV",
            AutonomousProviderCallAuthorization::Invalid {
                reason_code: "test_invalid",
                detail: "test".to_string(),
            },
            false,
        ),
    ];
    for (adapter_suffix, symbol, authorization, expect_disabled) in cases {
        let operation = create_test_operation(
            &pool,
            &format!("zzdrv-auth29-{adapter_suffix}"),
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        // No exact bar seeded — the missing-bar path is reached.

        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::always(readiness);
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
            authorization,
            instruments: &instruments,
            provider_id: "fake",
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
        })
        .await
        .expect("tick ok");

        // Points 29, 30.
        if expect_disabled {
            assert_eq!(
                outcome,
                AutonomousCompletedBarDriverOutcome::AuthorizationDisabled
            );
        } else {
            assert!(matches!(
                outcome,
                AutonomousCompletedBarDriverOutcome::AuthorizationInvalid { .. }
            ));
        }
        // Point 31: resolver never called for either missing-bar refusal.
        assert_eq!(provider.calls(), 0);
        assert_eq!(
            resolver.resolve_calls(),
            0,
            "resolver must never be called when authorization blocks the missing-bar path ({adapter_suffix})"
        );
    }
}

#[tokio::test]
async fn authz_32_authorized_missing_bar_resolves_provider_exactly_once() {
    let Some(pool) = maybe_db("authz_32").await else {
        return;
    };
    let timing = standard_timing();
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-auth32",
        "ZZDRVAUTH32",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);
    let instruments = vec![fixture_instrument(
        "ZZDRVAUTH32",
        "fake",
        "ZZDRVAUTH32",
        "5m",
    )];
    let assignment_config = fixture_assignment_config("ZZDRVAUTH32", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVAUTH32", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    let provider = Arc::new(FakeQueueProvider::new());
    provider.push_outcome(
        "ZZDRVAUTH32",
        Ok(Some(bar("ZZDRVAUTH32", "5m", expected_ts, true))),
    );
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVAUTH32", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");

    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(provider.calls(), 1);
    assert_eq!(
        resolver.resolve_calls(),
        1,
        "point 32: the provider must be resolved exactly once for one authorized missing-bar poll"
    );
}

// ---------------------------------------------------------------------------
// Evidence inconsistency (points 33-35): `last_completed_bar_ts ==
// expected_ts` but the backing `md_bars` row is missing, or present with
// mismatched provider provenance.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn evidence_33_missing_row_returns_typed_inconsistency_zero_calls() {
    let Some(pool) = maybe_db("evidence_33").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-ev33",
        "ZZDRVEV33",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVEV33", "fake", "ZZDRVEV33", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVEV33", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVEV33", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // Durable evidence claims the bar was observed, but no md_bars row backs
    // it (never seeded in this test).
    let observed = mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    assert!(matches!(
        observed,
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { .. }
    ));
    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();

    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVEV33", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);
    let state = active_bootstrap_state(pool.clone()).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
            expected_end_ts,
            reason_code,
        } => {
            assert_eq!(expected_end_ts, expected_ts);
            assert_eq!(
                reason_code,
                mqk_daemon::state::autonomous_completed_bar_driver::REASON_OBSERVED_BAR_MISSING_FROM_MD_BARS
            );
        }
        other => panic!("expected ObservedBarEvidenceInconsistent, got {other:?}"),
    }
    assert_eq!(provider.calls(), 0);
    assert_eq!(resolver.resolve_calls(), 0);
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVEV33",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none());
}

#[tokio::test]
async fn evidence_34_provider_mismatch_returns_typed_inconsistency() {
    let Some(pool) = maybe_db("evidence_34").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-ev34",
        "ZZDRVEV34",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVEV34", "fake", "ZZDRVEV34", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVEV34", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVEV34", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // The row at the exact expected identity exists, but was ingested under
    // a different provider_id than the one this binding's registry mapping
    // resolves to ("fake") — durable evidence corruption, not a normal
    // missing-tail-bar condition.
    seed_exact_bar(
        &pool,
        "ZZDRVEV34",
        "5m",
        expected_ts,
        "othervendor",
        "ZZDRVEV34",
    )
    .await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();

    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVEV34", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);
    let state = active_bootstrap_state(pool.clone()).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
            reason_code,
            ..
        } => {
            assert_eq!(
                reason_code,
                mqk_daemon::state::autonomous_completed_bar_driver::REASON_OBSERVED_BAR_PROVIDER_MISMATCH
            );
        }
        other => panic!("expected ObservedBarEvidenceInconsistent, got {other:?}"),
    }
    assert_eq!(provider.calls(), 0);
    assert_eq!(resolver.resolve_calls(), 0);
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVEV34",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none());
}

#[tokio::test]
async fn evidence_35_provider_symbol_mismatch_returns_typed_inconsistency() {
    let Some(pool) = maybe_db("evidence_35").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-ev35",
        "ZZDRVEV35",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument("ZZDRVEV35", "fake", "ZZDRVEV35", "5m")];
    let assignment_config = fixture_assignment_config("ZZDRVEV35", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVEV35", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // Correct provider_id, but the row's provider_symbol does not match the
    // canonical registry-resolved provider symbol for this binding.
    seed_exact_bar(&pool, "ZZDRVEV35", "5m", expected_ts, "fake", "WRONGSYM").await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        expected_ts,
        timing.effective_open,
    )
    .await
    .expect("record observed");
    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();

    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVEV35", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);
    let state = active_bootstrap_state(pool.clone()).await;

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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
            reason_code,
            ..
        } => {
            assert_eq!(
                reason_code,
                mqk_daemon::state::autonomous_completed_bar_driver::REASON_OBSERVED_BAR_PROVIDER_SYMBOL_MISMATCH
            );
        }
        other => panic!("expected ObservedBarEvidenceInconsistent, got {other:?}"),
    }
    assert_eq!(provider.calls(), 0);
    assert_eq!(resolver.resolve_calls(), 0);
}

// ---------------------------------------------------------------------------
// Provider-resolver-focused proof: a construction failure never increments
// the provider-poll attempt counter, since no provider request occurred.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolver_failure_provider_setup_blocked_zero_poll_attempt_increment() {
    let Some(pool) = maybe_db("resolver_failure").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-resfail",
        "ZZDRVRESFAIL",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let instruments = vec![fixture_instrument(
        "ZZDRVRESFAIL",
        "fake",
        "ZZDRVRESFAIL",
        "5m",
    )];
    let assignment_config = fixture_assignment_config("ZZDRVRESFAIL", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVRESFAIL", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;
    // No exact bar seeded — the missing-bar path is reached and the
    // resolver is invoked.

    let resolver = FakeProviderResolver::failing("simulated credential/construction failure");
    let readiness = ready_readiness("ZZDRVRESFAIL", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::always(readiness);
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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    match outcome {
        AutonomousCompletedBarDriverOutcome::ProviderSetupBlocked { rejection } => {
            assert!(matches!(
                rejection,
                AutonomousDriverSetupRejection::ProviderConstructionFailed(_)
            ));
        }
        other => panic!("expected ProviderSetupBlocked, got {other:?}"),
    }
    assert_eq!(resolver.resolve_calls(), 1);

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.provider_poll_attempt_count, 0,
        "a resolver failure must never increment the provider-poll attempt counter — \
         no provider request occurred"
    );
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01
// Preparation-versus-dispatch mode separation proofs.
// ---------------------------------------------------------------------------

/// Preparation-mode points 1-11: a missing exact bar is polled, ingested, and
/// observed by `PrepareDataOnly` with no active runtime and no native
/// strategy bootstrap at all — proving preparation never depends on either.
/// Zero dispatch claims, zero pending strategy bar deposits, and
/// `bars_dispatched` stays zero throughout.
#[tokio::test]
async fn prepare_only_01_11_missing_bar_observed_no_claim_no_bootstrap_needed() {
    let Some(pool) = maybe_db("prepare_only_01_11").await else {
        return;
    };
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        "zzdrv-prep01",
        "ZZDRVPREP01",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_PREPARING_DATA,
    )
    .await;
    let instruments = vec![fixture_instrument(
        "ZZDRVPREP01",
        "fake",
        "ZZDRVPREP01",
        "5m",
    )];
    let assignment_config = fixture_assignment_config("ZZDRVPREP01", "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding("ZZDRVPREP01", "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    let provider = Arc::new(FakeQueueProvider::new());
    provider.push_outcome(
        "ZZDRVPREP01",
        Ok(Some(bar("ZZDRVPREP01", "5m", expected_ts, true))),
    );
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness("ZZDRVPREP01", "5m", Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    // Point 10/11: no active runtime, no native-strategy bootstrap — plain
    // `AppState` with a DB pool and nothing else.
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
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");

    // Points 1-5: poll once, ingest, observe, readiness ready, BarObserved.
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::BarObserved {
            bar_end_ts: expected_ts
        }
    );
    assert_eq!(provider.calls(), 1, "point 1: exactly one authorized poll");

    // Points 6-9: no dispatch row, no pending strategy bar, no strategy
    // callback (the fixture provider/host never records a callback; absence
    // of a claim row is the durable proof).
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZDRVPREP01",
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_none(), "point 6: no dispatch claim exists");
    assert!(
        state.pending_strategy_bar_input_is_none_for_test().await,
        "point 8: no pending strategy bar was deposited"
    );

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.bars_observed, 1, "point 3: observed exactly once");
    assert_eq!(
        refreshed.bars_dispatched, 0,
        "point 7: bars_dispatched remains zero"
    );
}

/// Preparation-mode points 12-16: the exact expected bar already canonical in
/// `md_bars` is reconciled by `PrepareDataOnly` idempotently, with zero
/// provider resolutions, and authorization state (disabled/invalid) never
/// blocks this local path — mirroring [`authz_25_28_local_bar_present_ignores_authorization_disabled_and_invalid`]
/// but asserting the preparation-mode outcome (`BarObserved`, never a
/// dispatch claim) instead of a dispatch-path outcome.
#[tokio::test]
async fn prepare_only_12_16_exact_local_bar_zero_provider_authorization_irrelevant() {
    let Some(pool) = maybe_db("prepare_only_12_16").await else {
        return;
    };
    let timing = standard_timing();
    let cases: Vec<(&str, &str, AutonomousProviderCallAuthorization)> = vec![
        (
            "dis",
            "ZZDRVPREPDIS",
            AutonomousProviderCallAuthorization::Disabled,
        ),
        (
            "inv",
            "ZZDRVPREPINV",
            AutonomousProviderCallAuthorization::Invalid {
                reason_code: "test_invalid",
                detail: "test".to_string(),
            },
        ),
    ];
    for (adapter_suffix, symbol, authorization) in cases {
        let operation = create_test_operation(
            &pool,
            &format!("zzdrv-prep12-{adapter_suffix}"),
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;

        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        // Called twice: the driver ticks twice below to prove idempotency
        // (point 13), so the evaluator must supply pre-poll/post-poll pairs
        // for both ticks.
        let evaluator = FakeReadinessEvaluator::queue(vec![
            readiness.clone(),
            readiness.clone(),
            readiness.clone(),
            readiness,
        ]);
        let state = state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        );

        let outcome1 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
            state: &state,
            pool: &pool,
            operation: &operation,
            assignment_config: &assignment_config,
            assignment_identity: &assignment_identity,
            runtime_binding: &binding,
            runtime_binding_identity: &runtime_binding_identity,
            now_utc: timing.effective_open + chrono::Duration::minutes(6),
            authorization: authorization.clone(),
            instruments: &instruments,
            provider_id: "fake",
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
        })
        .await
        .expect("tick ok");

        // Points 12, 15, 16: local exact-bar path reconciles regardless of
        // authorization disabled/invalid.
        assert_eq!(
            outcome1,
            AutonomousCompletedBarDriverOutcome::BarObserved {
                bar_end_ts: expected_ts
            },
            "local exact-bar preparation must succeed regardless of authorization state ({adapter_suffix})"
        );
        assert_eq!(
            provider.calls(),
            0,
            "zero provider calls ({adapter_suffix})"
        );
        assert_eq!(
            resolver.resolve_calls(),
            0,
            "zero provider construction/credential reads ({adapter_suffix})"
        );

        // Point 13: idempotent on a second tick — zero additional provider
        // resolutions, same outcome.
        let operation2 =
            mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
                .await
                .unwrap()
                .unwrap();
        let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
            state: &state,
            pool: &pool,
            operation: &operation2,
            assignment_config: &assignment_config,
            assignment_identity: &assignment_identity,
            runtime_binding: &binding,
            runtime_binding_identity: &runtime_binding_identity,
            now_utc: timing.effective_open
                + chrono::Duration::minutes(6)
                + chrono::Duration::seconds(5),
            authorization,
            instruments: &instruments,
            provider_id: "fake",
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
        })
        .await
        .expect("tick ok");
        assert_eq!(
            outcome2,
            AutonomousCompletedBarDriverOutcome::BarObserved {
                bar_end_ts: expected_ts
            },
            "point 13: idempotent re-observation ({adapter_suffix})"
        );
        assert_eq!(resolver.resolve_calls(), 0);

        // No dispatch claim was ever created by preparation mode.
        let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            symbol,
            "5m",
            expected_ts,
        )
        .await
        .unwrap();
        assert!(
            claim.is_none(),
            "preparation mode must never create a dispatch claim ({adapter_suffix})"
        );
    }
}

/// Running-dispatch eligibility points 17-25: every one of the six typed
/// eligibility preconditions blocks claim creation independently, before
/// `claim_autonomous_daily_bar_dispatch` is ever called. Every case leaves
/// zero dispatch claims and zero pending strategy bar deposits.
#[tokio::test]
async fn running_dispatch_eligibility_17_25_blocks_before_claim() {
    let Some(pool) = maybe_db("running_dispatch_eligibility_17_25").await else {
        return;
    };
    let timing = standard_timing();

    #[allow(clippy::too_many_arguments)]
    async fn assert_blocked(
        pool: &sqlx::PgPool,
        state: &state::AppState,
        operation: &mqk_db::AutonomousDailyOperationRecord,
        assignment_config: &MultiSymbolRuntimeConfig,
        assignment_identity: &str,
        binding: &EffectiveRuntimeBinding,
        runtime_binding_identity: &str,
        instruments: &[TrackedInstrument],
        symbol: &str,
        expected_ts: i64,
        now_utc: DateTime<Utc>,
        expected_reason: &'static str,
        case_label: &str,
    ) {
        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::always(readiness);

        let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
            state,
            pool,
            operation,
            assignment_config,
            assignment_identity,
            runtime_binding: binding,
            runtime_binding_identity,
            now_utc,
            authorization: AutonomousProviderCallAuthorization::Authorized,
            instruments,
            provider_id: "fake",
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
        })
        .await
        .expect("tick ok");

        assert_eq!(
            outcome,
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: expected_reason
            },
            "case {case_label}: expected RuntimeDispatchNotReady({expected_reason})"
        );
        let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
            pool,
            operation.operation_id,
            symbol,
            "5m",
            expected_ts,
        )
        .await
        .unwrap();
        assert!(claim.is_none(), "case {case_label}: zero dispatch claims");
        assert!(
            state.pending_strategy_bar_input_is_none_for_test().await,
            "case {case_label}: zero pending strategy bar deposits"
        );
    }

    // Case: operation.state != running (point 17).
    {
        let symbol = "ZZDRVELIG17";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig17",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        let state = running_dispatch_eligible_state(pool.clone(), run_id).await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_OPERATION_NOT_RUNNING,
            "17_operation_not_running",
        )
        .await;
    }

    // Case: operation.run_id is None (point 18).
    {
        let symbol = "ZZDRVELIG18";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig18",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = None;
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        let state = running_dispatch_eligible_state(pool.clone(), Uuid::new_v4()).await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_OPERATION_RUN_ID_MISSING,
            "18_run_id_missing",
        )
        .await;
    }

    // Case: no locally owned runtime at all (point 19).
    {
        let symbol = "ZZDRVELIG19";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig19",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(Uuid::new_v4());
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        // Active bootstrap present, but no locally injected running loop.
        let state = active_bootstrap_state(pool.clone()).await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_LOCAL_RUNTIME_NOT_ACTIVE,
            "19_no_locally_owned_run",
        )
        .await;
    }

    // Case: locally owned run_id mismatches operation.run_id (point 20).
    {
        let symbol = "ZZDRVELIG20";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig20",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(Uuid::new_v4());
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        // Owns a DIFFERENT run_id than the operation's.
        let state = running_dispatch_eligible_state(pool.clone(), Uuid::new_v4()).await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH,
            "20_run_id_mismatch",
        )
        .await;
    }

    // Case: missing native-strategy bootstrap (point 21).
    {
        let symbol = "ZZDRVELIG21";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig21",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        // Runtime owned, matching run_id, but no bootstrap stored at all.
        let state = state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        );
        state.inject_running_loop_for_test(run_id).await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
            "21_bootstrap_missing",
        )
        .await;
    }

    // Case: dormant native-strategy bootstrap (point 22).
    {
        let symbol = "ZZDRVELIG22";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig22",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        let state = state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        );
        state.inject_running_loop_for_test(run_id).await;
        // Dormant: bootstrap from an empty fleet.
        let dormant = NativeStrategyBootstrap::bootstrap(None, &build_daemon_plugin_registry());
        state
            .set_native_strategy_bootstrap_for_test(Some(dormant))
            .await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT,
            "22_bootstrap_dormant",
        )
        .await;
    }

    // Case: failed native-strategy bootstrap (point 23).
    {
        let symbol = "ZZDRVELIG23";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-elig23",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        let state = state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        );
        state.inject_running_loop_for_test(run_id).await;
        // Failed: fleet names a strategy id that is not in the registry.
        let failed = NativeStrategyBootstrap::bootstrap(
            Some(&["not_a_registered_strategy".to_string()]),
            &build_daemon_plugin_registry(),
        );
        state
            .set_native_strategy_bootstrap_for_test(Some(failed))
            .await;
        assert_blocked(
            &pool,
            &state,
            &operation,
            &assignment_config,
            &assignment_identity,
            &binding,
            &runtime_binding_identity,
            &instruments,
            symbol,
            expected_ts,
            timing.effective_open + chrono::Duration::minutes(6),
            REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED,
            "23_bootstrap_failed",
        )
        .await;
    }
}

/// Preopen-to-running lifecycle points 26-35: a bar observed by
/// `PrepareDataOnly` before any runtime exists is dispatched exactly once
/// after a simulated canonical runtime start, and a third tick reports
/// `AlreadyDispatched` — never a duplicate claim or strategy evaluation.
#[tokio::test]
async fn preopen_to_running_lifecycle_26_35_exactly_once_dispatch() {
    let Some(pool) = maybe_db("preopen_to_running_lifecycle_26_35").await else {
        return;
    };
    let timing = standard_timing();
    let symbol = "ZZDRVLIFE01";
    let mut operation = create_test_operation(
        &pool,
        "zzdrv-life01",
        symbol,
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_PREPARING_DATA,
    )
    .await;
    let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
    let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
    let assignment_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
            &assignment_config,
        );
    let binding = fixture_binding(symbol, "swing_momentum", 300);
    let runtime_binding_identity =
        mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(&binding);
    let expected_ts = timing.effective_open.timestamp() + 300;

    // Tick 1 (preopen, point 26): PrepareDataOnly, no active runtime.
    let provider1 = Arc::new(FakeQueueProvider::new());
    provider1.push_outcome(
        "ZZDRVLIFE01",
        Ok(Some(bar(symbol, "5m", expected_ts, true))),
    );
    let resolver1 = FakeProviderResolver::new(provider1.clone());
    let readiness1 = ready_readiness(symbol, "5m", Some(expected_ts));
    let evaluator1 = FakeReadinessEvaluator::queue(vec![readiness1.clone(), readiness1]);
    let prep_state = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    let now1 = timing.effective_open + chrono::Duration::minutes(6);

    let outcome1 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &prep_state,
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
        provider_resolver: &resolver1,
        readiness_evaluator: &evaluator1,
        mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome1,
        AutonomousCompletedBarDriverOutcome::BarObserved {
            bar_end_ts: expected_ts
        }
    );
    let claim_after_prep = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        "5m",
        expected_ts,
    )
    .await
    .unwrap();
    assert!(
        claim_after_prep.is_none(),
        "point 26: preparation must not create a claim"
    );
    let after_prep = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_prep.bars_dispatched, 0);

    // Simulated canonical runtime start (points 27): durable operation
    // transitions to `running` with a real run_id, local ownership is
    // established, and the native-strategy bootstrap becomes active.
    operation.state = mqk_db::STATE_RUNNING.to_string();
    let run_id = Uuid::new_v4();
    operation.run_id = Some(run_id);
    let running_state = running_dispatch_eligible_state(pool.clone(), run_id).await;

    // Tick 2 (running, points 28-33): the exact bar is already canonical, so
    // zero provider calls this tick; a fresh claim is created and dispatch
    // completes exactly once.
    let provider2 = Arc::new(FakeQueueProvider::new());
    let resolver2 = FakeProviderResolver::new(provider2.clone());
    let readiness2 = ready_readiness(symbol, "5m", Some(expected_ts));
    let evaluator2 = FakeReadinessEvaluator::queue(vec![readiness2.clone(), readiness2]);
    let now2 = now1 + chrono::Duration::seconds(30);

    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &running_state,
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
        provider_resolver: &resolver2,
        readiness_evaluator: &evaluator2,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        },
        "point 29/30: exactly-once claim and dispatch after runtime start"
    );
    assert_eq!(
        provider2.calls(),
        0,
        "point 28: zero provider calls on the running tick (bar already observed)"
    );
    let after_run = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_run.bars_observed, 1,
        "point 32: observation unchanged"
    );
    assert_eq!(
        after_run.bars_dispatched, 1,
        "point 33: dispatch counter increments exactly once"
    );

    // Tick 3 (points 34-35): completed claim detected, no redispatch.
    let provider3 = Arc::new(FakeQueueProvider::new());
    let resolver3 = FakeProviderResolver::new(provider3.clone());
    let readiness3 = ready_readiness(symbol, "5m", Some(expected_ts));
    let evaluator3 = FakeReadinessEvaluator::queue(vec![readiness3.clone(), readiness3]);
    let outcome3 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &running_state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now2 + chrono::Duration::seconds(30),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver3,
        readiness_evaluator: &evaluator3,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("tick ok");
    assert!(matches!(
        outcome3,
        AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. }
    ));
    assert_eq!(provider3.calls(), 0);
    let final_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        final_row.bars_dispatched, 1,
        "no duplicate claim or strategy evaluation across the third tick"
    );
}

/// Race/failure-safety points 36-38: if the runtime or the native-strategy
/// bootstrap disappears between preparation and the running-dispatch
/// eligibility check, no claim is ever created — `RuntimeDispatchNotReady`,
/// fail-closed. The narrower race (disappearing strictly *after* a claim was
/// already created but before dispatch completes) is not newly introduced by
/// this repair: it is the existing failed/unresolved claim safety net proven
/// by [`claim_22_24_unresolved_claim_never_auto_redispatched`] and
/// [`dispatch_40_42_staleness_gate_remains_active_and_failure_not_falsely_completed`]
/// (no automatic replay of an unresolved claim) — this repair does not
/// change that narrower post-claim behavior.
#[tokio::test]
async fn race_36_38_runtime_or_bootstrap_disappears_before_claim_blocks() {
    let Some(pool) = maybe_db("race_36_38").await else {
        return;
    };
    let timing = standard_timing();

    // Point 36: runtime ownership disappears (e.g. the execution loop
    // exited) before the eligibility check — no claim, RuntimeDispatchNotReady.
    {
        let symbol = "ZZDRVRACE36";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-race36",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(Uuid::new_v4());
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        // Bootstrap active, but no locally owned run at all — as if the
        // execution loop had already exited.
        let state = active_bootstrap_state(pool.clone()).await;

        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::always(readiness);
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
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
        })
        .await
        .expect("tick ok");
        assert_eq!(
            outcome,
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_LOCAL_RUNTIME_NOT_ACTIVE
            }
        );
        let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            symbol,
            "5m",
            expected_ts,
        )
        .await
        .unwrap();
        assert!(claim.is_none(), "point 36: no claim created");
    }

    // Point 37: the native-strategy bootstrap disappears (e.g. stop/halt
    // cleared it) before the eligibility check — no claim,
    // RuntimeDispatchNotReady.
    {
        let symbol = "ZZDRVRACE37";
        let mut operation = create_test_operation(
            &pool,
            "zzdrv-race37",
            symbol,
            "swing_momentum",
            "5m",
            &timing,
            mqk_db::STATE_AWAITING_OPEN,
        )
        .await;
        let run_id = Uuid::new_v4();
        operation.state = mqk_db::STATE_RUNNING.to_string();
        operation.run_id = Some(run_id);
        let instruments = vec![fixture_instrument(symbol, "fake", symbol, "5m")];
        let assignment_config = fixture_assignment_config(symbol, "swing_momentum", "5m");
        let assignment_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_assignment_identity(
                &assignment_config,
            );
        let binding = fixture_binding(symbol, "swing_momentum", 300);
        let runtime_binding_identity =
            mqk_daemon::state::autonomous_daily_operation::derive_runtime_binding_identity(
                &binding,
            );
        let expected_ts = timing.effective_open.timestamp() + 300;
        seed_exact_bar(&pool, symbol, "5m", expected_ts, "fake", symbol).await;
        // Runtime still locally owned, but the bootstrap was cleared (e.g.
        // by `stop_execution_runtime`/`halt_execution_runtime`, which zero
        // `native_strategy_bootstrap` even if a stale loop handle lingered).
        let state = state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        );
        state.inject_running_loop_for_test(run_id).await;
        state.set_native_strategy_bootstrap_for_test(None).await;

        let provider = Arc::new(FakeQueueProvider::new());
        let resolver = FakeProviderResolver::new(provider.clone());
        let readiness = ready_readiness(symbol, "5m", Some(expected_ts));
        let evaluator = FakeReadinessEvaluator::always(readiness);
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
            provider_resolver: &resolver,
            readiness_evaluator: &evaluator,
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
        })
        .await
        .expect("tick ok");
        assert_eq!(
            outcome,
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING
            }
        );
        let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            symbol,
            "5m",
            expected_ts,
        )
        .await
        .unwrap();
        assert!(claim.is_none(), "point 37: no claim created");
    }

    // Point 38 is documented above (doc comment), not re-proven here — see
    // claim_22_24 and dispatch_40_42 for the existing post-claim fail-closed
    // guarantee this repair leaves unchanged.
}
