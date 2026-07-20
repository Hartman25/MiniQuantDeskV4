//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-INTEGRATED-LIFECYCLE-PROOF-AND-
//! PHASE-D-CLOSURE: integrated proof that the durable daily coordinator, the
//! completed-bar production adapter/task, the durable bar-dispatch claim,
//! and canonical native-strategy dispatch cooperate correctly across one
//! synthetic Paper+Alpaca trading day, and that the dispatch-ownership race
//! closed by D4.2 (`autonomous_completed_bar_driver::claim_and_dispatch_observed_bar`
//! now calls `AppState::dispatch_native_strategy_for_symbol_with_bar`
//! directly instead of depositing into the shared `pending_strategy_bar_input`
//! mailbox) actually holds under real concurrent scheduling.
//!
//! DB-backed; every test requires `MQK_DATABASE_URL` and is marked
//! `#[ignore]`, matching every other DB-backed scenario file in this crate.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_phase_d_integration_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! `phase_d_full_day_lifecycle` uses the exact real-start fixture pattern
//! proven by `scenario_autonomous_paper_day_lifecycle_auton12.rs`'s AL-03
//! (synthetic instrument/provider registry files, seeded canonical
//! `md_bars`, a loopback in-process mock Alpaca REST server, a registered
//! `intraday_scalper` strategy, a Live broker cursor) — the first test in
//! this crate to additionally drive the completed-bar production adapter
//! (`tick_autonomous_completed_bar_driver_from_state`) and a real coordinator
//! recovery cycle on top of that real start, which AL-03 itself explicitly
//! disclaims ("AL-03 ... never drives the completed-bar driver").
//!
//! The concurrency-ownership proof and the task-supervision/shutdown/
//! permanent-failure proofs use the lighter DB-backed fixture pattern from
//! `scenario_autonomous_completed_bar_driver_01.rs` / `_task_01.rs`
//! (`active_bootstrap_state` + `inject_running_loop_for_test`, fake
//! provider/readiness seams) — full production-gate realism is not needed
//! to prove claim-ownership or task-supervision mechanics, and the lighter
//! fixture keeps those proofs fast and fully deterministic.
//!
//! No Phase E outcome/no-trade finalization is performed or asserted by
//! this file.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use mqk_broker_alpaca::{encode_fetch_cursor, types::AlpacaFetchCursor};
use mqk_daemon::daily_data_readiness::AssignmentReadiness;
use mqk_daemon::state;
use mqk_daemon::state::autonomous_completed_bar_driver::{
    tick_autonomous_completed_bar_driver, AutonomousAssignmentReadinessEvaluator,
    AutonomousCompletedBarDriverInput, AutonomousCompletedBarDriverMode,
    AutonomousCompletedBarDriverOutcome, AutonomousCompletedBarPostClaimTestHook,
    AutonomousDriverSetupRejection, AutonomousLatestBarProviderResolver,
    AutonomousProviderCallAuthorization, ResolvedSingleBinding,
};
use mqk_daemon::state::autonomous_completed_bar_task::{
    spawn_autonomous_completed_bar_driver_task, spawn_supervised_completed_bar_task_for_test,
    tick_autonomous_completed_bar_driver_from_state, AutonomousCompletedBarProductionTickOutcome,
    AutonomousCompletedBarRestartPolicy, AutonomousCompletedBarTaskSpawnOutcome,
    COMPLETED_BAR_TICK_SECS_ENV,
};
use mqk_daemon::state::autonomous_daily_coordinator::{
    handle_running, tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
    AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::{
    self as daemon_state, AppState, AutonomousDailyPlanTiming, AutonomousSessionTruth, BrokerKind,
    DeploymentMode, MultiSymbolConfigSource, MultiSymbolRuntimeConfig, OperatorAuthMode,
    StrategyBarInput, SymbolStrategyAssignment,
};
use mqk_daemon::state::{
    resolve_autonomous_daily_session_plan_from_env, AutonomousDailySessionPlanResolution,
};
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_runtime::native_strategy::{
    build_daemon_plugin_registry, EffectiveRuntimeBinding, NativeStrategyBootstrap,
};
use tokio::net::TcpListener;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared test-env plumbing
// ---------------------------------------------------------------------------

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
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
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    Some(pool)
}

async fn cleanup_adapter_slot(pool: &sqlx::PgPool, adapter_id: &str) {
    let _ = sqlx::query(
        "delete from sys_autonomous_daily_bar_dispatches where operation_id in \
         (select operation_id from sys_autonomous_daily_operations where adapter_id = $1)",
    )
    .bind(adapter_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "delete from sys_autonomous_daily_operation_events where operation_id in \
         (select operation_id from sys_autonomous_daily_operations where adapter_id = $1)",
    )
    .bind(adapter_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from sys_autonomous_daily_operations where adapter_id = $1")
        .bind(adapter_id)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Light fixtures (mirrors scenario_autonomous_completed_bar_driver_01.rs) —
// used by the concurrency-ownership and task-supervision proofs, which need
// fast, fully deterministic control rather than a real gate-chain start.
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
        notes: "phase-d integration test fixture".to_string(),
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

struct FakeReadinessEvaluator {
    items: StdMutex<VecDeque<AssignmentReadiness>>,
    repeat_last: bool,
}

impl FakeReadinessEvaluator {
    fn queue(items: Vec<AssignmentReadiness>) -> Self {
        Self {
            items: StdMutex::new(VecDeque::from(items)),
            repeat_last: false,
        }
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
        let mut items = self.items.lock().unwrap();
        match items.pop_front() {
            Some(readiness) => {
                if self.repeat_last {
                    items.push_back(readiness.clone());
                }
                Ok(readiness)
            }
            None => panic!("FakeReadinessEvaluator queue exhausted"),
        }
    }
}

type FakeLatestBarOutcome = Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError>;
type FakeLatestBarOutcomeQueues = std::collections::HashMap<String, VecDeque<FakeLatestBarOutcome>>;

#[derive(Default)]
struct FakeQueueProvider {
    outcomes: StdMutex<FakeLatestBarOutcomeQueues>,
    calls: AtomicUsize,
}

impl FakeQueueProvider {
    fn new() -> Self {
        Self::default()
    }

    fn push_outcome(&self, symbol: &str, outcome: FakeLatestBarOutcome) {
        self.outcomes
            .lock()
            .unwrap()
            .entry(symbol.to_string())
            .or_default()
            .push_back(outcome);
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
        mqk_md::MarketDataProviderCapabilities {
            historical_bars: false,
            latest_closed_bar: true,
            completed_bar_stream: false,
            supported_asset_classes: vec![mqk_md::ProviderAssetClass::Equity],
            supported_timeframes: vec![mqk_md::Timeframe::M5, mqk_md::Timeframe::D1],
        }
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

struct FakeProviderResolver {
    provider: Arc<FakeQueueProvider>,
}

impl FakeProviderResolver {
    fn new(provider: Arc<FakeQueueProvider>) -> Self {
        Self { provider }
    }
}

impl AutonomousLatestBarProviderResolver for FakeProviderResolver {
    fn resolve(
        &self,
        _provider_id: &str,
    ) -> Result<mqk_md::MarketDataProviderBox, AutonomousDriverSetupRejection> {
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

/// Seed one or more canonical completed bars into `md_bars` for `symbol` in
/// one wipe-then-insert-all pass, so
/// `AppState::dispatch_native_strategy_for_symbol_with_bar`'s DB-backed
/// context load (used by *both* the completed-bar claim's exact-input
/// dispatch and the ordinary execution-loop's mailbox-drain dispatch) finds
/// real, non-empty, non-stale history instead of silently falling through
/// to the empty-context fail-closed path — a dispatch that fails closed for
/// an unrelated reason (no history) cannot prove the claim-ownership
/// concurrency contract, since both call sites would trivially return
/// `None` regardless of ordering.
///
/// D4 REPAIR 5: accepts a slice so the concurrency proofs can seed claim A's
/// exact expected bar and the execution-loop decoy's own distinct bar
/// identity (bar B) together — a single wipe that seeded only one of them
/// would delete the other.
async fn seed_light_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts_list: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("cleanup light bars failed");
    for &end_ts in end_ts_list {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,$2,$3,100000000,100000000,100000000,100000000,1000000,true,
                      'fake','fake',$1,'historical_sync',$4)
            "#,
        )
        .bind(symbol)
        .bind(timeframe)
        .bind(end_ts)
        .bind(
            Utc.timestamp_opt(end_ts + 60, 0)
                .single()
                .expect("valid ingested_at ts"),
        )
        .execute(pool)
        .await
        .expect("seed light bars insert failed");
    }
}

/// The legacy `market_data_freshness` staleness gate evaluates bar age
/// against the real wall clock even when the rest of the fixture uses a
/// fixed historical `end_ts` — raise `MQK_INTRADAY_BAR_MAX_AGE_SECS` just
/// enough to clear that gate for this process, mirroring
/// `pd_legacy_freshness_max_age_secs`.
fn set_light_freshness_override_for(latest_bar_end_ts: i64) {
    let real_now = Utc::now().timestamp();
    let max_age = (real_now - latest_bar_end_ts).max(0) + 3600;
    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_INTRADAY_BAR_MAX_AGE_SECS", max_age.to_string());
    }
}

struct LightTiming {
    preopen_start_utc: DateTime<Utc>,
    effective_open: DateTime<Utc>,
    effective_close: DateTime<Utc>,
    postclose_finalize_utc: DateTime<Utc>,
    previous_trading_date: chrono::NaiveDate,
    market_date: chrono::NaiveDate,
}

fn light_timing() -> LightTiming {
    LightTiming {
        market_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(), // Monday
        previous_trading_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(),
        preopen_start_utc: DateTime::parse_from_rfc3339("2026-07-20T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        effective_open: DateTime::parse_from_rfc3339("2026-07-20T13:30:00Z")
            .unwrap()
            .with_timezone(&Utc),
        effective_close: DateTime::parse_from_rfc3339("2026-07-20T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        postclose_finalize_utc: DateTime::parse_from_rfc3339("2026-07-20T20:15:00Z")
            .unwrap()
            .with_timezone(&Utc),
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_light_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    symbol: &str,
    strategy_id: &str,
    timeframe: &str,
    timing: &LightTiming,
    initial_state: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let assignment_config = fixture_assignment_config(symbol, strategy_id, timeframe);
    let assignment_identity = daemon_state::derive_assignment_identity(&assignment_config);
    let binding = fixture_binding(symbol, strategy_id, 300);
    let runtime_binding_identity = daemon_state::derive_runtime_binding_identity(&binding);
    let session_plan_identity = format!("test-session-plan|{adapter_id}");
    let operation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.autonomous-daily-operation.v1|test|{adapter_id}").as_bytes(),
    );

    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: timing.market_date,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: timing.effective_open,
        effective_operation_close_utc: timing.effective_close,
        exchange_session_open_utc: timing.effective_open,
        exchange_session_close_utc: timing.effective_close,
        exchange_is_early_close: false,
        previous_trading_date: timing.previous_trading_date,
        preopen_start_utc: timing.preopen_start_utc,
        postclose_finalize_utc: timing.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "awaiting_preopen".to_string(),
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "phase-d integration test fixture".to_string(),
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

async fn active_bootstrap_state(pool: sqlx::PgPool) -> AppState {
    let state = AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    let bootstrap = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    state
        .set_native_strategy_bootstrap_for_test(Some(bootstrap))
        .await;
    state
}

async fn running_dispatch_eligible_state(pool: sqlx::PgPool, run_id: Uuid) -> AppState {
    let state = active_bootstrap_state(pool).await;
    state.inject_running_loop_for_test(run_id).await;
    // D4 REPAIR 2: `AppState::record_signal_evaluation` derives its
    // evaluation identity from `status.active_run_id`, not from
    // `execution_loop`'s injected ownership — production keeps both in sync
    // via `publish_status` on every real start; this light fixture must do
    // the same explicitly, or the durable evaluation row's run_id would
    // silently diverge from the exact run_id this claim's own identity
    // expects (an evaluation-lineage mismatch, not a real production gap).
    state.status.write().await.active_run_id = Some(run_id);
    state
}

// ---------------------------------------------------------------------------
// D4.4 — concurrency-ownership proof
// ---------------------------------------------------------------------------
//
// Required assertions 9-11: a concurrent execution-loop-style dispatch tick
// cannot steal claim-owned input in either scheduling order; no operation
// degradation occurs; no duplicate/second evaluation of the claimed bar.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_concurrency_forward_ordering_execution_loop_cannot_steal_claimed_bar() {
    let Some(pool) = maybe_db("pd_concurrency_fwd").await else {
        return;
    };
    let adapter_id = format!("zzpdcc-fwd-{}", unique_suffix());
    let timing = light_timing();
    let symbol = "ZZPDCCFWD1";
    let strategy_id = "swing_momentum";
    let timeframe = "5m";

    let mut operation = create_light_operation(
        &pool,
        &adapter_id,
        symbol,
        strategy_id,
        timeframe,
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);

    let assignment_config = fixture_assignment_config(symbol, strategy_id, timeframe);
    let assignment_identity = daemon_state::derive_assignment_identity(&assignment_config);
    let binding = fixture_binding(symbol, strategy_id, 300);
    let runtime_binding_identity = daemon_state::derive_runtime_binding_identity(&binding);
    let instruments = vec![fixture_instrument(symbol, "fake", symbol, timeframe)];

    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;
    let hook = Arc::new(AutonomousCompletedBarPostClaimTestHook::default());
    state
        .set_completed_bar_post_claim_test_hook_for_test(Some(hook.clone()))
        .await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    set_light_freshness_override_for(expected_ts);
    let provider = Arc::new(FakeQueueProvider::new());
    provider.push_outcome(symbol, Ok(Some(bar(symbol, timeframe, expected_ts, true))));
    let resolver = FakeProviderResolver::new(provider.clone());
    let readiness = ready_readiness(symbol, timeframe, Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let now = DateTime::<Utc>::from_timestamp(expected_ts + 10, 0).unwrap();

    // D4 REPAIR 5: bar B (the decoy) must carry a genuinely different bar
    // identity from claim A's own expected bar — not merely a different
    // `now_tick` — so this proof cannot be read as depending on `now_tick`
    // alone to keep the two evaluations distinct. `decoy_ts` is a real,
    // independently seeded, already-complete prior bar.
    let decoy_ts = expected_ts - 300;
    seed_light_bars(&pool, symbol, timeframe, &[decoy_ts, expected_ts]).await;
    // A decoy bar for the concurrent "execution loop" tick to drain from the
    // shared mailbox — proves the mailbox path is fully independent of the
    // completed-bar claim's exact-input dispatch after the D4.2 fix.
    state
        .deposit_strategy_bar_input(StrategyBarInput {
            now_tick: 999,
            end_ts: decoy_ts,
            limit_price: Some(10025),
            qty: 1,
        })
        .await;
    let assignments = vec![SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe: timeframe.to_string(),
    }];

    let driver_fut = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    });

    let exec_loop_fut = async {
        // Wait until the completed-bar path has created its fresh durable
        // claim and is paused immediately before calling the canonical
        // exact-input dispatch seam, then run the ordinary execution-loop
        // dispatch tick concurrently, then release the completed-bar path.
        hook.claimed.notified().await;
        let results = state
            .tick_strategy_dispatch_multi_symbol(&assignments)
            .await;
        hook.release.notify_waiters();
        results
    };

    let (driver_result, exec_loop_result) = tokio::join!(driver_fut, exec_loop_fut);

    let outcome = driver_result.expect("driver tick must not error");
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        },
        "the completed-bar claim's own evaluation must complete despite a concurrent \
         execution-loop tick draining the shared mailbox in between"
    );
    assert_eq!(
        exec_loop_result.len(),
        1,
        "the concurrent execution-loop tick must independently and successfully dispatch \
         its own mailbox-deposited bar — the two dispatch paths must not interfere"
    );
    assert!(
        state.pending_strategy_bar_input_is_none_for_test().await,
        "the mailbox must be fully drained by the execution-loop tick alone; the \
         completed-bar claim must never have touched it"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        after.bars_dispatched, 1,
        "exactly one canonical strategy evaluation for the claimed bar"
    );
    assert_ne!(
        after.state,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        "no operation degradation on a successful concurrent proof"
    );

    // D4 REPAIR 3: the completed claim durably stores the confirmed
    // evaluation id, and the exact evaluation row it points to exists
    // exactly once with matching identity.
    let claim_a = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        timeframe,
        expected_ts,
    )
    .await
    .expect("claim A fetch ok")
    .expect("claim A row exists");
    assert_eq!(claim_a.status, mqk_db::DISPATCH_STATUS_COMPLETED);
    let claim_a_evaluation_id = claim_a
        .evaluation_id
        .expect("claim A must store a non-null evaluation_id");
    let claim_a_eval = mqk_db::fetch_strategy_signal_evaluation(&pool, claim_a_evaluation_id)
        .await
        .expect("claim A evaluation fetch ok")
        .expect("claim A's exact evaluation row must exist");
    assert_eq!(claim_a_eval.run_id, Some(run_id));
    assert_eq!(claim_a_eval.strategy_id, strategy_id);
    assert_eq!(claim_a_eval.symbol, symbol);
    assert_eq!(claim_a_eval.timeframe, timeframe);
    assert_eq!(claim_a_eval.decision_stage, "strategy_evaluated");
    let claim_a_eval_count: i64 = sqlx::query_scalar(
        "select count(*) from strategy_signal_evaluations where evaluation_id = $1",
    )
    .bind(claim_a_evaluation_id)
    .fetch_one(&pool)
    .await
    .expect("count ok");
    assert_eq!(
        claim_a_eval_count, 1,
        "claim A's evaluation identity must exist exactly once"
    );
    assert_ne!(after.state, mqk_db::STATE_CONTROLLER_DEGRADED);

    // Repeat tick: the claim is already completed, so no second evaluation.
    let evaluator2 = FakeReadinessEvaluator::queue(vec![
        ready_readiness(symbol, timeframe, Some(expected_ts)),
        ready_readiness(symbol, timeframe, Some(expected_ts)),
    ]);
    let provider2 = Arc::new(FakeQueueProvider::new());
    let resolver2 = FakeProviderResolver::new(provider2);
    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now + chrono::Duration::seconds(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver2,
        readiness_evaluator: &evaluator2,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("second tick must not error");
    assert_eq!(
        outcome2,
        AutonomousCompletedBarDriverOutcome::AlreadyDispatched {
            evaluation_id: Some(claim_a_evaluation_id)
        },
        "repeat tick must never produce a second evaluation and must report the same \
         durably stored evaluation id, got {outcome2:?}"
    );
    let claim_a_eval_count_after_repeat: i64 = sqlx::query_scalar(
        "select count(*) from strategy_signal_evaluations where evaluation_id = $1",
    )
    .bind(claim_a_evaluation_id)
    .fetch_one(&pool)
    .await
    .expect("count ok");
    assert_eq!(
        claim_a_eval_count_after_repeat, 1,
        "repeat tick must never create a second evaluation row for claim A's identity"
    );

    cleanup_adapter_slot(&pool, &adapter_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_concurrency_reverse_ordering_execution_loop_first_is_also_safe() {
    let Some(pool) = maybe_db("pd_concurrency_rev").await else {
        return;
    };
    let adapter_id = format!("zzpdcc-rev-{}", unique_suffix());
    let timing = light_timing();
    let symbol = "ZZPDCCREV1";
    let strategy_id = "swing_momentum";
    let timeframe = "5m";

    let mut operation = create_light_operation(
        &pool,
        &adapter_id,
        symbol,
        strategy_id,
        timeframe,
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);

    let assignment_config = fixture_assignment_config(symbol, strategy_id, timeframe);
    let assignment_identity = daemon_state::derive_assignment_identity(&assignment_config);
    let binding = fixture_binding(symbol, strategy_id, 300);
    let runtime_binding_identity = daemon_state::derive_runtime_binding_identity(&binding);
    let instruments = vec![fixture_instrument(symbol, "fake", symbol, timeframe)];
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;

    let expected_ts = timing.effective_open.timestamp() + 300;
    set_light_freshness_override_for(expected_ts);
    // D4 REPAIR 5: bar B (the decoy) must carry a genuinely different bar
    // identity from claim A's own expected bar (see the forward-ordering
    // test's comment for the full rationale).
    let decoy_ts = expected_ts - 300;
    seed_light_bars(&pool, symbol, timeframe, &[decoy_ts, expected_ts]).await;
    let provider = Arc::new(FakeQueueProvider::new());
    provider.push_outcome(symbol, Ok(Some(bar(symbol, timeframe, expected_ts, true))));
    let resolver = FakeProviderResolver::new(provider);
    let readiness = ready_readiness(symbol, timeframe, Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let now = DateTime::<Utc>::from_timestamp(expected_ts + 10, 0).unwrap();

    // Inverse ordering: the ordinary execution-loop tick runs to completion
    // first, fully draining the mailbox, *before* the completed-bar claim
    // is even created.
    state
        .deposit_strategy_bar_input(StrategyBarInput {
            now_tick: 999,
            end_ts: decoy_ts,
            limit_price: Some(10025),
            qty: 1,
        })
        .await;
    let assignments = vec![SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe: timeframe.to_string(),
    }];
    let exec_loop_result = state
        .tick_strategy_dispatch_multi_symbol(&assignments)
        .await;
    assert_eq!(exec_loop_result.len(), 1);
    assert!(state.pending_strategy_bar_input_is_none_for_test().await);

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("driver tick must not error");
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompleted {
            bar_end_ts: expected_ts
        },
        "the completed-bar claim must still evaluate exactly once even when the mailbox \
         was already fully drained by an unrelated execution-loop tick beforehand"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(after.bars_dispatched, 1);
    assert_ne!(after.state, mqk_db::STATE_EVIDENCE_DEGRADED);

    cleanup_adapter_slot(&pool, &adapter_id).await;
}

// ---------------------------------------------------------------------------
// D4 REPAIR 2 — missing evaluation evidence cannot complete a claim
// ---------------------------------------------------------------------------
//
// Required assertion 13: a strategy callback result alone is never
// sufficient. Here the bootstrap's actually-active strategy
// ("intraday_scalper") diverges from the assignment's configured strategy
// ("swing_momentum") — a real config-drift scenario — so the journal writer
// durably records its evaluation under a different identity than the one the
// claim expects. No fabricated fault injection is needed: this is the same
// class of defect REPAIR 2 exists to catch.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_missing_evaluation_evidence_fails_closed_never_completes_claim() {
    let Some(pool) = maybe_db("pd_eval_missing").await else {
        return;
    };
    let adapter_id = format!("zzpdem-{}", unique_suffix());
    let timing = light_timing();
    let symbol = "ZZPDEVMISS";
    let configured_strategy_id = "swing_momentum";
    let actually_active_strategy_id = "intraday_scalper";
    let timeframe = "5m";

    let mut operation = create_light_operation(
        &pool,
        &adapter_id,
        symbol,
        configured_strategy_id,
        timeframe,
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);

    let assignment_config = fixture_assignment_config(symbol, configured_strategy_id, timeframe);
    let assignment_identity = daemon_state::derive_assignment_identity(&assignment_config);
    let binding = fixture_binding(symbol, configured_strategy_id, 300);
    let runtime_binding_identity = daemon_state::derive_runtime_binding_identity(&binding);
    let instruments = vec![fixture_instrument(symbol, "fake", symbol, timeframe)];

    // Bootstrap a *different* strategy than the assignment configures.
    // `autonomous_strategy_dispatch_runtime_truth` only checks bootstrap
    // presence/liveness, never strategy-id equality against the binding, so
    // runtime-dispatch eligibility still proves `Active` here — exactly the
    // gap REPAIR 2 closes at the evaluation-confirmation step instead.
    let state =
        AppState::new_with_db_and_operator_auth(pool.clone(), OperatorAuthMode::ExplicitDevNoToken);
    let reg = build_daemon_plugin_registry();
    let ids = vec![actually_active_strategy_id.to_string()];
    let bootstrap = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    state
        .set_native_strategy_bootstrap_for_test(Some(bootstrap))
        .await;
    state.inject_running_loop_for_test(run_id).await;
    state.status.write().await.active_run_id = Some(run_id);

    let expected_ts = timing.effective_open.timestamp() + 300;
    set_light_freshness_override_for(expected_ts);
    seed_light_bars(&pool, symbol, timeframe, &[expected_ts]).await;
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider);
    let readiness = ready_readiness(symbol, timeframe, Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let now = DateTime::<Utc>::from_timestamp(expected_ts + 10, 0).unwrap();

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("driver tick must not error");
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchEvaluationEvidenceMissing {
            bar_end_ts: expected_ts,
            reason_code: "evaluation_row_absent",
        },
        "a strategy callback result under a diverged identity must never complete the claim"
    );

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        timeframe,
        expected_ts,
    )
    .await
    .expect("claim fetch ok")
    .expect("claim row exists");
    assert_eq!(
        claim.status,
        mqk_db::DISPATCH_STATUS_FAILED,
        "the claim must be marked failed, never left ambiguously claimed"
    );
    assert!(claim.evaluation_id.is_none());

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        after.bars_dispatched, 0,
        "a failed claim must never advance the dispatched-bar counter"
    );

    cleanup_adapter_slot(&pool, &adapter_id).await;
}

// ---------------------------------------------------------------------------
// D4 REPAIR 4 — completion store-error/uncertainty is never silently
// reported as success
// ---------------------------------------------------------------------------
//
// Required assertion 12: a `complete_autonomous_daily_bar_dispatch` failure
// must trigger one authoritative re-read; when that re-read cannot confirm
// the exact expected evaluation id as durably `completed` (here: the real
// write never happened at all, via the test-only fault seam), the claim must
// never be reported `DispatchCompleted`, and the strategy evaluation itself
// must never be automatically rerun.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_completion_store_error_reconfirms_via_authoritative_re_read_and_fails_closed() {
    let Some(pool) = maybe_db("pd_completion_fault").await else {
        return;
    };
    let adapter_id = format!("zzpdcf-{}", unique_suffix());
    let timing = light_timing();
    let symbol = "ZZPDCFAULT";
    let strategy_id = "swing_momentum";
    let timeframe = "5m";

    let mut operation = create_light_operation(
        &pool,
        &adapter_id,
        symbol,
        strategy_id,
        timeframe,
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let run_id = Uuid::new_v4();
    operation.state = mqk_db::STATE_RUNNING.to_string();
    operation.run_id = Some(run_id);

    let assignment_config = fixture_assignment_config(symbol, strategy_id, timeframe);
    let assignment_identity = daemon_state::derive_assignment_identity(&assignment_config);
    let binding = fixture_binding(symbol, strategy_id, 300);
    let runtime_binding_identity = daemon_state::derive_runtime_binding_identity(&binding);
    let instruments = vec![fixture_instrument(symbol, "fake", symbol, timeframe)];
    let state = running_dispatch_eligible_state(pool.clone(), run_id).await;
    state
        .set_completed_bar_completion_fault_for_test(true)
        .await;

    // A hardcoded symbol is reused across runs of this test file; a prior
    // interrupted run could leave a stray row that would otherwise pollute
    // the by-symbol evaluation-count assertions below.
    sqlx::query("delete from strategy_signal_evaluations where symbol = $1")
        .bind(symbol)
        .execute(&pool)
        .await
        .expect("cleanup stray evaluation rows failed");

    let expected_ts = timing.effective_open.timestamp() + 300;
    set_light_freshness_override_for(expected_ts);
    seed_light_bars(&pool, symbol, timeframe, &[expected_ts]).await;
    let provider = Arc::new(FakeQueueProvider::new());
    let resolver = FakeProviderResolver::new(provider);
    let readiness = ready_readiness(symbol, timeframe, Some(expected_ts));
    let evaluator = FakeReadinessEvaluator::queue(vec![readiness.clone(), readiness]);
    let now = DateTime::<Utc>::from_timestamp(expected_ts + 10, 0).unwrap();

    let outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now,
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver,
        readiness_evaluator: &evaluator,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("driver tick must not error");
    assert_eq!(
        outcome,
        AutonomousCompletedBarDriverOutcome::DispatchCompletionUnconfirmed {
            bar_end_ts: expected_ts,
            reason_code: "completion_write_error",
        },
        "a simulated completion store error whose re-read shows the claim still \
         unconfirmed must never be reported as DispatchCompleted"
    );

    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        timeframe,
        expected_ts,
    )
    .await
    .expect("claim fetch ok")
    .expect("claim row exists");
    assert_eq!(
        claim.status,
        mqk_db::DISPATCH_STATUS_CLAIMED,
        "the real completion write never ran, so the claim remains claimed, not completed"
    );

    let evals: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where symbol = $1")
            .bind(symbol)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals, 1,
        "the strategy evaluation itself already ran exactly once and is never rerun"
    );

    // Clear the fault seam and tick again: the still-`claimed` row must be
    // reclassified to `uncertain` on the second claim attempt (never a
    // second strategy invocation, never automatic redispatch).
    state
        .set_completed_bar_completion_fault_for_test(false)
        .await;
    let readiness2 = ready_readiness(symbol, timeframe, Some(expected_ts));
    let evaluator2 = FakeReadinessEvaluator::queue(vec![readiness2.clone(), readiness2]);
    let provider2 = Arc::new(FakeQueueProvider::new());
    let resolver2 = FakeProviderResolver::new(provider2);
    let outcome2 = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state: &state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc: now + chrono::Duration::seconds(5),
        authorization: AutonomousProviderCallAuthorization::Authorized,
        instruments: &instruments,
        provider_id: "fake",
        provider_resolver: &resolver2,
        readiness_evaluator: &evaluator2,
        mode: AutonomousCompletedBarDriverMode::RunningDispatch,
    })
    .await
    .expect("second driver tick must not error");
    assert!(
        matches!(
            outcome2,
            AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved { .. }
        ),
        "no automatic redispatch of an unconfirmed claim, got {outcome2:?}"
    );
    let evals_after: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where symbol = $1")
            .bind(symbol)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals_after, 1,
        "no second strategy invocation for the same claim identity"
    );

    cleanup_adapter_slot(&pool, &adapter_id).await;
}

// ---------------------------------------------------------------------------
// D4 — task-supervision liveness and shutdown-ordering proof
// ---------------------------------------------------------------------------
//
// Required assertions 21-23: shutdown awaits task completion, no tick
// remains after shutdown returns, and the legacy ticker is never spawned
// (proven separately by scenario_autonomous_completed_bar_task_01.rs's i01;
// this test focuses on the liveness->shutdown ordering with a live,
// supervised, real-cadence-shaped task).

fn fake_paper_alpaca_state(pool: sqlx::PgPool) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(&format!("zzpdtask-{}", unique_suffix()));
    Arc::new(st)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_task_liveness_then_shutdown_blocks_further_ticks() {
    let Some(pool) = maybe_db("pd_task_liveness").await else {
        return;
    };
    let st = fake_paper_alpaca_state(pool);
    let tick_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&tick_count);

    let spawn_outcome = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        AutonomousCompletedBarRestartPolicy {
            max_restarts: 3,
            delays: vec![Duration::ZERO, Duration::ZERO, Duration::ZERO],
        },
        move |_st, _gen| {
            let counter = Arc::clone(&counter);
            move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }
        },
    )
    .await;
    assert_eq!(
        spawn_outcome,
        AutonomousCompletedBarTaskSpawnOutcome::Started
    );

    // Wait for at least one real tick and Running liveness.
    let mut observed_running = false;
    for _ in 0..200 {
        if st.completed_bar_task_truth().await.liveness
            == mqk_daemon::state::autonomous_completed_bar_driver::AutonomousCompletedBarDriverTaskLiveness::Running
            && tick_count.load(Ordering::SeqCst) > 0
        {
            observed_running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        observed_running,
        "completed-bar task must reach Running liveness with real ticks"
    );

    // Shutdown must await the in-flight/next tick and release ownership.
    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
    let count_at_shutdown = tick_count.load(Ordering::SeqCst);
    assert!(
        !st.completed_bar_task_claimed_for_test(),
        "spawn claim must be released after shutdown"
    );
    assert_eq!(
        st.completed_bar_task_truth().await.liveness,
        mqk_daemon::state::autonomous_completed_bar_driver::AutonomousCompletedBarDriverTaskLiveness::Stopped
    );

    // No tick after shutdown returns.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        tick_count.load(Ordering::SeqCst),
        count_at_shutdown,
        "no tick may begin or remain in progress after cancel_and_wait_completed_bar_task_for_shutdown returns"
    );
}

// ---------------------------------------------------------------------------
// D4 — task permanent-failure truth proof (separate fresh fixture, per
// mission allowance, so the full-day happy path fixture stays uncontaminated)
// ---------------------------------------------------------------------------
//
// Required assertions 16-17: permanent task failure degrades the relevant
// running operation exactly once and remains operator-visible even though
// the session-controller's own Running projection would otherwise hide it.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_task_permanent_failure_degrades_operation_once_and_stays_visible() {
    let Some(pool) = maybe_db("pd_task_failure").await else {
        return;
    };
    let adapter_id = format!("zzpdfail-{}", unique_suffix());
    let timing = light_timing();
    let symbol = "ZZPDFAIL01";
    let operation = create_light_operation(
        &pool,
        &adapter_id,
        symbol,
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    // `transition_autonomous_daily_operation_to_running` only accepts
    // `start_retrying -> running` as a legal edge; reach it via the same
    // legal `awaiting_open -> start_retrying` transition production code
    // uses (matching scenario_autonomous_completed_bar_task_01.rs's k03).
    let start_retrying_args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_START_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: timing.preopen_start_utc,
        run_id: None,
        bounded_detail: "test fixture: force start_retrying".to_string(),
    };
    let operation = match mqk_db::transition_autonomous_daily_operation(&pool, &start_retrying_args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let run_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_START_RETRYING.to_string(),
        expected_state_version: operation.state_version,
        run_id: Uuid::new_v4(),
        started_at_utc: timing.preopen_start_utc,
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "test: force running".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &run_args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_) => {}
        other => panic!("expected Applied, got {other:?}"),
    }

    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(&adapter_id);
    let st = Arc::new(st);

    let spawn_outcome = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        AutonomousCompletedBarRestartPolicy {
            max_restarts: 0,
            delays: vec![],
        },
        |_st, _gen| {
            move || async move {
                panic!("pd_task_failure: deliberate permanent failure");
            }
        },
    )
    .await;
    assert_eq!(
        spawn_outcome,
        AutonomousCompletedBarTaskSpawnOutcome::Started
    );

    let mut failed = false;
    for _ in 0..300 {
        if st.completed_bar_task_truth().await.liveness
            == mqk_daemon::state::autonomous_completed_bar_driver::AutonomousCompletedBarDriverTaskLiveness::Failed
        {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        failed,
        "task must reach permanent Failed liveness after exhausting its restart budget"
    );
    for _ in 0..300 {
        if !st.completed_bar_task_claimed_for_test() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        degraded.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "permanent task failure must degrade the relevant running operation to controller_degraded"
    );
    assert_eq!(
        degraded.state_reason_code.as_deref(),
        Some("completed_bar_task_permanently_failed")
    );
    let run_id_before = degraded.run_id;

    let event_count: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events \
         where operation_id = $1 and reason_code = 'completed_bar_task_permanently_failed'",
    )
    .bind(operation.operation_id)
    .fetch_one(&pool)
    .await
    .expect("count query ok");
    assert_eq!(event_count, 1, "no duplicate permanent-failure event");

    // Operator truth surfaces the failure even though nothing else has
    // touched `autonomous_session_truth` — the session-controller's own
    // Running-style projection can never hide this overlay (see
    // scenario_autonomous_completed_bar_task_01.rs's k06 for the underlying
    // overlay mechanism this test exercises through the real task).
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));

    // No runtime ownership change: this test never established local
    // ownership, and none was created by the failure path.
    assert!(st.locally_owned_run_id().await.is_none());
    assert_eq!(
        degraded.run_id, run_id_before,
        "run_id must be untouched by task failure"
    );

    cleanup_adapter_slot(&pool, &adapter_id).await;
}

// ---------------------------------------------------------------------------
// D4.5 — full synthetic Paper+Alpaca day through the production seams
// ---------------------------------------------------------------------------
//
// Fixture pattern mirrors scenario_autonomous_paper_day_lifecycle_auton12.rs
// AL-03 exactly (synthetic registries, seeded md_bars, loopback mock Alpaca
// server, registered intraday_scalper strategy, Live broker cursor), then
// additionally drives the completed-bar production adapter and a real
// coordinator recovery cycle on top of that real start.

const PD_SYMBOL: &str = "ZZPHASED01";
const PD_PROVIDER: &str = "zz_phased_provider";
const PD_STRATEGY: &str = "intraday_scalper";
const PD_TIMEFRAME: &str = "5m";
const PD_TIMEFRAME_SECS: i64 = 300;
const PD_REQUIRED_BARS: usize = 5;
// `BrokerKind::parse` (state/types.rs) only recognizes the literal string
// "alpaca" for `MQK_DAEMON_ADAPTER_ID` — matching AL-03's own
// `AL03_ADAPTER_ID` convention exactly, since `AppState::new_with_db_and_operator_auth`
// derives `runtime_selection` (deployment mode + broker kind) from that real
// env var rather than a constructor parameter. This shares the daily-slot
// uniqueness key `(market_date, "PAPER", "alpaca")` with AL-03's own fixture;
// both tests clean up their own adapter-id slot before creating a row, and
// the master patch mandates running one named regression binary at a time,
// so this is safe under the required sequential test-execution discipline.
const PD_ADAPTER_ID: &str = "alpaca";

const PD_READINESS_FIXED_TS: i64 = 1_713_188_100 + 10 * 300;

fn pd_now() -> DateTime<Utc> {
    Utc.timestamp_opt(PD_READINESS_FIXED_TS, 0)
        .single()
        .expect("valid fixed PD readiness timestamp")
}

fn pd_write_registries() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_phased01_registry_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create PD registry dir");

    let instruments_path = dir.join("instruments.json");
    std::fs::write(
        &instruments_path,
        format!(
            r#"[
  {{
    "instrument_id": "equity:US:{sym}",
    "symbol": "{sym}",
    "asset_class": "equity",
    "provider": "{prov}",
    "provider_symbol": "{sym}",
    "venue": "TEST",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["5m"],
    "notes": "AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4 phase-d integration fixture"
  }}
]"#,
            sym = PD_SYMBOL,
            prov = PD_PROVIDER,
        ),
    )
    .expect("write PD instruments fixture");

    let providers_path = dir.join("providers.json");
    std::fs::write(
        &providers_path,
        format!(
            r#"[
  {{
    "provider_id": "{prov}",
    "display_name": "PD Test Provider",
    "asset_classes": ["equity"],
    "free_tier_available": true,
    "api_key_required": false,
    "credential_env_vars": [],
    "rate_limit_notes": "",
    "supported_timeframes": ["5m"],
    "historical_depth_notes": "",
    "realtime_support_notes": "",
    "licensing_notes": "",
    "implementation_status": "test",
    "enabled": true,
    "verification_status": "test",
    "docs_url": ""
  }}
]"#,
            prov = PD_PROVIDER,
        ),
    )
    .expect("write PD providers fixture");

    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_INSTRUMENT_REGISTRY_PATH", &instruments_path);
        std::env::set_var("MQK_PROVIDER_REGISTRY_PATH", &providers_path);
    }
}

fn pd_expected_bar_window() -> Vec<i64> {
    let calendar_provider = daemon_state::market_calendar::NyseWeekdaysProvider;
    let now = pd_now();
    let schedule =
        daemon_state::market_calendar::resolve_market_session_schedule(&calendar_provider, now);
    mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        PD_TIMEFRAME_SECS,
        0,
        PD_REQUIRED_BARS,
    )
    .expect("PD expected 5m window resolves")
}

/// D4 REPAIR 6: the exact completed-bar history expected at a genuine
/// preopen instant (before the current session's own grid has produced any
/// closed bar) — `expected_intraday_end_ts_window` spills entirely into the
/// previous trading session's own tail grid in that case (see its own doc
/// comment), never into `pd_expected_bar_window`'s today-dated window. Using
/// the wrong window here would seed bars the preopen readiness evaluation
/// (evaluated at `preopen_now`, not the later `pd_now()`) never actually
/// expects, and the gate would still — correctly — refuse.
fn pd_preopen_expected_bar_window(preopen_now: DateTime<Utc>) -> Vec<i64> {
    let calendar_provider = daemon_state::market_calendar::NyseWeekdaysProvider;
    let schedule = daemon_state::market_calendar::resolve_market_session_schedule(
        &calendar_provider,
        preopen_now,
    );
    mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        preopen_now.timestamp(),
        PD_TIMEFRAME_SECS,
        0,
        PD_REQUIRED_BARS,
    )
    .expect("PD preopen expected 5m window resolves")
}

async fn pd_seed_bars(pool: &sqlx::PgPool, expected_ts: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(PD_SYMBOL)
        .bind(PD_TIMEFRAME)
        .execute(pool)
        .await
        .expect("cleanup PD bars failed");
    for &end_ts in expected_ts {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,$2,$3,100000000,100000000,100000000,100000000,1000000,true,
                      $4,$4,$1,'historical_sync',$5)
            "#,
        )
        .bind(PD_SYMBOL)
        .bind(PD_TIMEFRAME)
        .bind(end_ts)
        .bind(PD_PROVIDER)
        .bind(
            Utc.timestamp_opt(end_ts + 60, 0)
                .single()
                .expect("valid ingested_at ts"),
        )
        .execute(pool)
        .await
        .expect("seed PD bar insert failed");
    }
}

fn pd_legacy_freshness_max_age_secs(latest_bar_end_ts: i64) -> i64 {
    let real_now = Utc::now().timestamp();
    (real_now - latest_bar_end_ts).max(0) + 3600
}

async fn pd_start_mock_alpaca_server() -> String {
    let app = axum::Router::new().route(
        "/v2/account/activities/FILL",
        axum::routing::get(|| async { axum::Json(serde_json::json!([])) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{}", addr.port())
}

async fn pd_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "phase_d_full_day_lifecycle requires MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_autonomous_daily_phase_d_integration_01 \
             -- --include-ignored"
        )
    });
    // A generous pool: this fixture keeps two `AppState` instances alive at
    // once (the original `st` and the post-"restart" `st2`), and `st`'s
    // background reconcile-tick task from its own real start keeps polling
    // the DB throughout — a starved pool produces spurious
    // `temporary_database_operation_failure` retryable-transient outcomes
    // that are an artifact of this fixture, not of the coordinator logic
    // under test.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("PD connect failed");
    mqk_db::migrate(&pool).await.expect("PD migrate failed");

    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_leader_lease");
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_control_state");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");
    sqlx::query("DELETE FROM sys_reconcile_status_state")
        .execute(&pool)
        .await
        .expect("cleanup sys_reconcile_status_state");
    sqlx::query("DELETE FROM broker_event_cursor WHERE adapter_id = 'alpaca'")
        .execute(&pool)
        .await
        .expect("cleanup broker_event_cursor");
    sqlx::query("DELETE FROM md_bars WHERE symbol = $1 AND timeframe = $2")
        .bind(PD_SYMBOL)
        .bind(PD_TIMEFRAME)
        .execute(&pool)
        .await
        .expect("cleanup PD md_bars");
    sqlx::query("DELETE FROM strategy_signal_evaluations WHERE symbol = $1")
        .bind(PD_SYMBOL)
        .execute(&pool)
        .await
        .expect("cleanup PD strategy_signal_evaluations");
    cleanup_adapter_slot(&pool, PD_ADAPTER_ID).await;
    sqlx::query(
        "INSERT INTO sys_strategy_registry \
         (strategy_id, display_name, enabled, kind, registered_at_utc, updated_at_utc, note) \
         VALUES ($1, 'Intraday Scalper', true, 'bar_driven', NOW(), NOW(), 'phased01-fixture') \
         ON CONFLICT (strategy_id) DO UPDATE SET enabled = true, updated_at_utc = NOW()",
    )
    .bind(PD_STRATEGY)
    .execute(&pool)
    .await
    .expect("upsert intraday_scalper for PD");

    pool
}

async fn pd_daemon_state() -> (Arc<AppState>, sqlx::PgPool) {
    let mock_url = pd_start_mock_alpaca_server().await;
    pd_write_registries();
    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
        std::env::set_var("MQK_DAEMON_ADAPTER_ID", PD_ADAPTER_ID);
        std::env::set_var("ALPACA_API_KEY_PAPER", "test-paper-key");
        std::env::set_var("ALPACA_API_SECRET_PAPER", "test-paper-secret");
        std::env::set_var("ALPACA_PAPER_BASE_URL", &mock_url);
        std::env::set_var("MQK_STRATEGY_IDS", PD_STRATEGY);
        std::env::set_var("MQK_STRATEGY_SYMBOL", PD_SYMBOL);
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", PD_TIMEFRAME);
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
        std::env::set_var(COMPLETED_BAR_TICK_SECS_ENV, "1");
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_SESSION_START_HH_MM");
        std::env::remove_var("MQK_SESSION_STOP_HH_MM");
    }

    let pool = pd_pool().await;
    // Bars are deliberately NOT seeded here. A genuine preopen tick must see
    // zero completed bars — seeding the full window this early would make
    // every bar look like it closed in the future relative to an
    // authentic preopen `now_utc`, which is correctly refused by the
    // readiness gate rather than being a bug in this fixture. The full-day
    // lifecycle test seeds bars itself right before its "open and start"
    // phase; other callers of this helper that skip the preopen phase must
    // seed bars themselves before ticking.
    let expected_bars = pd_expected_bar_window();
    let latest_bar_end_ts = *expected_bars
        .last()
        .expect("PD expected window is non-empty");
    let legacy_max_age_secs = pd_legacy_freshness_max_age_secs(latest_bar_end_ts);
    #[allow(deprecated)]
    unsafe {
        std::env::set_var(
            "MQK_INTRADAY_BAR_MAX_AGE_SECS",
            legacy_max_age_secs.to_string(),
        );
    }

    let state = Arc::new(AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    state
        .set_daily_data_readiness_clock_override_for_test(Some(pd_now()))
        .await;

    let resolved_config = daemon_state::build_multi_symbol_runtime_config_from_env()
        .expect("PD fixture assignment must resolve");
    assert_eq!(resolved_config.symbols.len(), 1);
    assert_eq!(expected_bars.len(), PD_REQUIRED_BARS);

    let live_cursor =
        AlpacaFetchCursor::live(None, "alpaca:phased01:start", "2026-01-01T00:00:00Z");
    let cursor_json = encode_fetch_cursor(&live_cursor).expect("encode live cursor");
    mqk_db::advance_broker_cursor(
        state.db.as_ref().expect("db must be set"),
        "alpaca",
        &cursor_json,
        chrono::Utc::now(),
    )
    .await
    .expect("persist live broker cursor for PD");
    state
        .update_ws_continuity(state::AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:phased01:start".to_string(),
            last_event_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await;
    {
        let mut broker = state.broker_snapshot.write().await;
        *broker = Some(mqk_schemas::BrokerSnapshot {
            captured_at_utc: chrono::Utc::now(),
            account: mqk_schemas::BrokerAccount {
                equity: "100000".to_string(),
                cash: "100000".to_string(),
                currency: "USD".to_string(),
            },
            orders: vec![],
            fills: vec![],
            positions: vec![],
        });
    }
    {
        let mut execution = state.execution_snapshot.write().await;
        *execution = Some(mqk_runtime::observability::ExecutionSnapshot {
            run_id: None,
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: mqk_runtime::observability::PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        });
    }
    (state, pool)
}

/// Re-establish the exact same in-memory truth `pd_daemon_state` sets on a
/// *fresh* `AppState` pointed at the same pool/DB — representing "the
/// daemon process restarted" for the runtime-interruption/recovery phase.
/// The durable arm/reconcile/broker-cursor truth is DB-resident and already
/// persists; only per-process in-memory truth needs re-seeding, exactly as
/// production `main.rs` re-seeds it on every real boot.
async fn pd_restarted_daemon_state(pool: sqlx::PgPool) -> Arc<AppState> {
    let state = Arc::new(AppState::new_with_db_and_operator_auth(
        pool,
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    state
        .set_daily_data_readiness_clock_override_for_test(Some(pd_now()))
        .await;
    {
        let mut ig = state.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // BRK-07R/broker_rules.md: a `Live` cursor from a prior session
    // correctly demotes to `ColdStartUnproven` on restart (`seed_ws_continuity_from_db`)
    // — WS must re-establish, never resume `Live` from mere DB seeding. This
    // fixture is not testing WS reconnection itself; it simulates "the WS
    // transport already re-established a live connection after restart" the
    // same explicit way `pd_daemon_state` establishes it for the initial
    // start, so the recovery proof below exercises coordinator/claim
    // behavior rather than re-deriving broker transport mechanics.
    state
        .update_ws_continuity(state::AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:phased01:recovery".to_string(),
            last_event_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await;
    {
        let mut broker = state.broker_snapshot.write().await;
        *broker = Some(mqk_schemas::BrokerSnapshot {
            captured_at_utc: chrono::Utc::now(),
            account: mqk_schemas::BrokerAccount {
                equity: "100000".to_string(),
                cash: "100000".to_string(),
                currency: "USD".to_string(),
            },
            orders: vec![],
            fills: vec![],
            positions: vec![],
        });
    }
    {
        let mut execution = state.execution_snapshot.write().await;
        *execution = Some(mqk_runtime::observability::ExecutionSnapshot {
            run_id: None,
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: mqk_runtime::observability::PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        });
    }
    state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_full_day_lifecycle() {
    let (st, pool) = pd_daemon_state().await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("PD: persist ARMED failed");

    let market_date = chrono::NaiveDate::from_ymd_opt(2024, 4, 15).unwrap();

    // ---------------------------------------------------------------------
    // Preopen: durable daily operation created, prepared, zero claim, zero
    // strategy evaluation, zero order.
    // ---------------------------------------------------------------------
    let timing = AutonomousDailyPlanTiming::production_default();
    let plan = match resolve_autonomous_daily_session_plan_from_env(pd_now(), &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => panic!("PD: expected an applicable session plan, got {other:?}"),
    };
    let preopen_now = plan.preopen_start_utc + chrono::Duration::minutes(1);

    // D4 REPAIR 6: the completed-bar driver's own readiness evaluation is
    // keyed on whatever `now_utc` its caller passes — the preopen tick below
    // passes `preopen_now`, not the later `pd_now()`. At a genuine preopen
    // instant (before the current session's grid has closed any bar),
    // `expected_intraday_end_ts_window` spills entirely into the *previous*
    // trading session's own tail grid — a different, earlier bar window than
    // `pd_expected_bar_window` (today's window, expected once running
    // dispatch begins at `pd_now()`). Seeding only the preopen-relevant tail
    // *now* — the later today-dated window is seeded further below,
    // immediately before the "open and start" phase, exactly as real ingest
    // timing would produce it — is what "the exact completed-bar history
    // expected at the preopen instant" concretely means: seeding today's
    // still-in-the-future bars this early would make the readiness gate see
    // provenance it cannot yet honestly have (`latest_bar_future`), which is
    // itself a real, correct block this patch must not paper over.
    let preopen_expected_bars = pd_preopen_expected_bar_window(preopen_now);
    let preopen_expected_ts = *preopen_expected_bars
        .last()
        .expect("non-empty preopen window");
    let expected_bars = pd_expected_bar_window();
    let last_expected_ts = *expected_bars.last().expect("non-empty window");
    pd_seed_bars(&pool, &preopen_expected_bars).await;

    let preopen_outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: preopen_now,
    })
    .await
    .expect("PD: preopen tick must not error");
    assert!(
        !matches!(
            preopen_outcome,
            AutonomousDailyCoordinatorTickOutcome::Started { .. }
        ),
        "PD: preopen must never itself reach a real start"
    );

    // Zero claim, zero strategy evaluation, zero order regardless of which
    // specific non-running state the row lands in — the safety invariant
    // Phase D exists to prove, independent of readiness outcome.
    let preopen_operation = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        PD_ADAPTER_ID,
    )
    .await
    .expect("PD: fetch preopen operation failed")
    .expect("PD: operation row must exist after preopen tick");
    assert_ne!(preopen_operation.state, mqk_db::STATE_RUNNING);
    assert!(preopen_operation.run_id.is_none());

    let claim_before_start = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        preopen_operation.operation_id,
        PD_SYMBOL,
        PD_TIMEFRAME,
        preopen_expected_ts,
    )
    .await
    .expect("PD: claim lookup ok");
    assert!(
        claim_before_start.is_none(),
        "preopen must create zero dispatch claim"
    );

    // D4 REPAIR 6: the real production completed-bar adapter, ticked at the
    // real preopen instant, must select `PrepareDataOnly` and durably
    // observe the exact expected preopen bar — never a claim, never a
    // strategy call, never a provider call (the exact bar is already local).
    let preopen_driver_tick = tick_autonomous_completed_bar_driver_from_state(&st, preopen_now)
        .await
        .expect("PD: preopen driver tick must not error");
    assert_eq!(
        preopen_driver_tick,
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            operation_id: preopen_operation.operation_id,
            mode: AutonomousCompletedBarDriverMode::PrepareDataOnly,
            outcome: AutonomousCompletedBarDriverOutcome::BarObserved {
                bar_end_ts: preopen_expected_ts
            },
        },
        "PD: preopen must select PrepareDataOnly and durably observe the exact expected bar, \
         got {preopen_driver_tick:?}"
    );

    // Mode selection itself (preparing_data/awaiting_open/... ->
    // PrepareDataOnly, running -> RunningDispatch) is the pure,
    // DB-independent classifier `select_driver_mode_for_state`; proving it
    // directly is exact and immune to this fixture's bar/clock timing.
    for pre_running_state in [
        mqk_db::STATE_AWAITING_PREOPEN,
        mqk_db::STATE_PREPARING_DATA,
        mqk_db::STATE_AWAITING_OPEN,
        mqk_db::STATE_PREFLIGHT_BLOCKED,
        mqk_db::STATE_START_RETRYING,
    ] {
        assert_eq!(
            daemon_state::select_driver_mode_for_state(pre_running_state),
            Some(AutonomousCompletedBarDriverMode::PrepareDataOnly),
            "state {pre_running_state} must select PrepareDataOnly"
        );
    }
    assert_eq!(
        daemon_state::select_driver_mode_for_state(mqk_db::STATE_RUNNING),
        Some(AutonomousCompletedBarDriverMode::RunningDispatch)
    );

    let evals_before_start: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where symbol = $1")
            .bind(PD_SYMBOL)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals_before_start, 0,
        "zero strategy evaluation before runtime start"
    );

    // D4 REPAIR 6: no manual unstick transition is used anywhere in this
    // fixture. The row must never have entered `manual_intervention_required`
    // at all, the exact preopen expected bar must be durably recorded as
    // observed, and zero provider calls were made (the bar was already
    // local).
    let preopen_row =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, preopen_operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_ne!(
        preopen_row.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "PD: real autonomous preopen must never enter manual_intervention_required"
    );
    assert_eq!(
        preopen_row.last_completed_bar_ts,
        Some(preopen_expected_ts),
        "PD: the exact preopen expected bar must become durably observed"
    );
    assert_eq!(
        preopen_row.provider_poll_attempt_count, 0,
        "PD: zero provider calls when the exact expected bar is already local"
    );

    // ---------------------------------------------------------------------
    // Open and start: canonical coordinator start binds an exact run_id.
    // Bars are seeded now, immediately before the instant they are dated
    // for (`pd_now()`) — matching real ingest timing, where a bar exists
    // only once its own close time has genuinely passed. This replaces the
    // preopen tail window seeded above (`pd_seed_bars` wipes and reinserts);
    // the earlier preopen bar's observation is already durably recorded on
    // the operation row and does not depend on the row still being present
    // in `md_bars`.
    // ---------------------------------------------------------------------
    pd_seed_bars(&pool, &expected_bars).await;
    // `dispatch_by_state` advances at most one durable state per tick (e.g.
    // preparing_data -> awaiting_open on one tick, the real start attempt on
    // the next) when a row already exists from an earlier tick — unlike a
    // brand-new row created for the first time already past open, which
    // reaches its initial legal state directly. Poll a bounded number of
    // ticks at the same `now_utc`, matching how the real session controller
    // simply ticks again every 30s until the row advances.
    let mut run_id_1: Option<Uuid> = None;
    for _ in 0..5 {
        let start_outcome =
            tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
                state: &st,
                now_utc: pd_now(),
            })
            .await
            .expect("PD: start tick must not error");
        match start_outcome {
            AutonomousDailyCoordinatorTickOutcome::Started { run_id } => {
                run_id_1 = Some(run_id);
                break;
            }
            AutonomousDailyCoordinatorTickOutcome::AwaitingOpen
            | AutonomousDailyCoordinatorTickOutcome::PreparingData
            | AutonomousDailyCoordinatorTickOutcome::RetryNotDue => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
            other => panic!("PD: unexpected outcome advancing toward Started: {other:?}"),
        }
    }
    let run_id_1 = run_id_1.expect("PD: coordinator must reach Started within the bounded poll");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let running_operation = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        PD_ADAPTER_ID,
    )
    .await
    .expect("PD: fetch running operation failed")
    .expect("PD: operation row must exist after Started");
    assert_eq!(running_operation.state, mqk_db::STATE_RUNNING);
    assert_eq!(running_operation.run_id, Some(run_id_1));
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id_1),
        "local ownership must bind to the exact run_id"
    );

    // ---------------------------------------------------------------------
    // Running dispatch: fresh claim, exactly one evaluation, claim
    // completed; repeat tick is AlreadyDispatched with zero second eval.
    // ---------------------------------------------------------------------
    let dispatch_tick_now = pd_now() + chrono::Duration::seconds(5);
    let dispatch_outcome = tick_autonomous_completed_bar_driver_from_state(&st, dispatch_tick_now)
        .await
        .expect("PD: dispatch tick must not error");
    let dispatched_bar_ts = match dispatch_outcome {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            mode: AutonomousCompletedBarDriverMode::RunningDispatch,
            outcome: AutonomousCompletedBarDriverOutcome::DispatchCompleted { bar_end_ts },
            ..
        } => bar_end_ts,
        other => panic!("PD: expected RunningDispatch/DispatchCompleted, got {other:?}"),
    };
    assert_eq!(dispatched_bar_ts, last_expected_ts);

    let evals_after_one_dispatch: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where run_id = $1")
            .bind(run_id_1)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals_after_one_dispatch, 1,
        "exactly one canonical strategy evaluation"
    );

    let already_outcome = tick_autonomous_completed_bar_driver_from_state(
        &st,
        dispatch_tick_now + chrono::Duration::seconds(1),
    )
    .await
    .expect("PD: repeat dispatch tick must not error");
    assert!(
        matches!(
            already_outcome,
            AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
                outcome: AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. },
                ..
            }
        ),
        "PD: repeat tick must be AlreadyDispatched, got {already_outcome:?}"
    );
    let evals_after_repeat: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where run_id = $1")
            .bind(run_id_1)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals_after_repeat, 1,
        "no second evaluation on the repeat tick"
    );

    // ---------------------------------------------------------------------
    // Runtime interruption and recovery: a terminal prior run without halt
    // is observed by a fresh process (no local ownership); recovery is
    // scheduled with bounded timing, then a canonical recovery start binds
    // an exact replacement run_id; the already-dispatched bar is not
    // reevaluated.
    // ---------------------------------------------------------------------
    // Terminate `st`'s own execution loop and background resources before
    // simulating the crash — an actual process crash would kill them too,
    // and leaving `st`'s loop ticking against a `runs` row this test then
    // mutates out from under it produces spurious safety halts on the
    // *next* run this pool creates (a fixture artifact, not a coordinator
    // defect). `stop_for_shutdown` marks `run_id_1` terminal itself; it
    // never touches the durable daily-operation row, which is the
    // coordinator's own truth and must remain `running` for `handle_running`
    // to observe a genuinely terminal-run-without-halt condition below.
    st.stop_for_shutdown().await;
    let st2 = pd_restarted_daemon_state(pool.clone()).await;
    {
        let mut restarted = AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        );
        restarted.set_adapter_id_for_test(PD_ADAPTER_ID);
        // st2 already carries the correct deployment/broker defaults from
        // `new_with_db_and_operator_auth`; this scratch value only proves
        // the adapter_id fixture constant matches the production wiring
        // env var already set globally for this process.
        assert_eq!(restarted.adapter_id(), PD_ADAPTER_ID);
    }
    let crash_detected_now = pd_now() + chrono::Duration::minutes(2);
    let recovery_scheduled =
        handle_running(&st2, &pool, running_operation.clone(), crash_detected_now)
            .await
            .expect("PD: handle_running must not error");
    assert_eq!(
        recovery_scheduled,
        AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
    );
    let recovering_row =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, running_operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(recovering_row.state, mqk_db::STATE_RECOVERY_RETRYING);
    let next_retry_utc = recovering_row
        .next_retry_utc
        .expect("recovery must schedule a bounded retry");
    assert!(
        next_retry_utc > crash_detected_now,
        "recovery retry must be scheduled in the future, not immediate"
    );

    let mut recovery_due_now = next_retry_utc + chrono::Duration::seconds(1);
    let mut run_id_2: Option<Uuid> = None;
    for _ in 0..5 {
        let recovery_outcome =
            tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
                state: &st2,
                now_utc: recovery_due_now,
            })
            .await
            .expect("PD: recovery tick must not error");
        match recovery_outcome {
            AutonomousDailyCoordinatorTickOutcome::Recovered { run_id } => {
                run_id_2 = Some(run_id);
                break;
            }
            AutonomousDailyCoordinatorTickOutcome::StartAttempted
            | AutonomousDailyCoordinatorTickOutcome::RetryNotDue => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(
                    &pool,
                    running_operation.operation_id,
                )
                .await
                .expect("fetch ok")
                .expect("row exists");
                recovery_due_now = refreshed
                    .next_retry_utc
                    .map(|t| t + chrono::Duration::seconds(1))
                    .unwrap_or(recovery_due_now + chrono::Duration::seconds(1));
                continue;
            }
            other => panic!("PD: unexpected outcome advancing toward Recovered: {other:?}"),
        }
    }
    let run_id_2 = run_id_2.expect("PD: coordinator must reach Recovered within the bounded poll");
    assert_ne!(
        run_id_2, run_id_1,
        "recovery must bind an exact replacement run_id"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;

    let recovered_row = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        PD_ADAPTER_ID,
    )
    .await
    .expect("fetch ok")
    .expect("row exists");
    assert_eq!(recovered_row.state, mqk_db::STATE_RUNNING);
    assert_eq!(recovered_row.run_id, Some(run_id_2));
    assert_eq!(st2.locally_owned_run_id().await, Some(run_id_2));

    // The already-completed bar must not be reevaluated after recovery.
    let post_recovery_tick = tick_autonomous_completed_bar_driver_from_state(
        &st2,
        recovery_due_now + chrono::Duration::seconds(5),
    )
    .await
    .expect("PD: post-recovery driver tick must not error");
    assert!(
        matches!(
            post_recovery_tick,
            AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
                outcome: AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. },
                ..
            }
        ),
        "PD: already-dispatched bar must remain AlreadyDispatched after recovery, got {post_recovery_tick:?}"
    );
    let evals_after_recovery: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where symbol = $1")
            .bind(PD_SYMBOL)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        evals_after_recovery, 1,
        "recovery must never reevaluate an already-dispatched bar"
    );

    // ---------------------------------------------------------------------
    // Session close: no new dispatch begins; matching runtime stops
    // canonically; operation remains stopping with stopped_at_utc.
    // ---------------------------------------------------------------------
    let close_now = recovered_row.effective_operation_close_utc;
    let close_outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st2,
        now_utc: close_now,
    })
    .await
    .expect("PD: close tick must not error");
    assert_eq!(
        close_outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
    );

    let stopped_row = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        PD_ADAPTER_ID,
    )
    .await
    .expect("fetch ok")
    .expect("row exists");
    assert_eq!(stopped_row.state, mqk_db::STATE_STOPPING);
    assert!(stopped_row.stopped_at_utc.is_some());
    assert!(
        st2.locally_owned_run_id().await.is_none(),
        "local ownership must be cleared after the canonical stop"
    );

    // Completed-bar task must not process the stopping state.
    let post_close_tick = tick_autonomous_completed_bar_driver_from_state(
        &st2,
        close_now + chrono::Duration::seconds(5),
    )
    .await
    .expect("PD: post-close driver tick must not error");
    assert!(
        matches!(
            &post_close_tick,
            AutonomousCompletedBarProductionTickOutcome::ModeNotApplicable {
                state,
                ..
            } if state == mqk_db::STATE_STOPPING
        ),
        "PD: stopping state must select no automated driver invocation, got {post_close_tick:?}"
    );

    // No paper or live order was ever submitted across the whole day.
    let outbox_count: (i64,) =
        sqlx::query_as("select count(*) from oms_outbox where run_id = any($1)")
            .bind(vec![run_id_1, run_id_2])
            .fetch_one(&pool)
            .await
            .expect("PD: outbox count query failed");
    assert_eq!(
        outbox_count.0, 0,
        "PD: no order may ever be submitted by this proof"
    );

    // ---------------------------------------------------------------------
    // Shutdown: no completed-bar task was left running on either AppState
    // instance in this narrative (dispatch was driven manually throughout),
    // so shutdown is a clean no-op on both — proving it never panics or
    // hangs when nothing is live.
    // ---------------------------------------------------------------------
    st2.cancel_and_wait_completed_bar_task_for_shutdown().await;
    st2.stop_for_shutdown().await;
    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
    st.stop_for_shutdown().await;

    cleanup_adapter_slot(&pool, PD_ADAPTER_ID).await;
}

// ---------------------------------------------------------------------------
// D4 — spawn wiring proof: exactly one production adapter, driven by the
// real spawn seam, never the legacy ticker (main.rs itself is proven by
// scenario_autonomous_completed_bar_task_01.rs's i01/i02/i03; this proves
// the same spawn seam remains safe to call from an isolated fixture).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn phase_d_spawn_seam_starts_at_most_one_completed_bar_task() {
    let Some(pool) = maybe_db("pd_spawn_seam").await else {
        return;
    };
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(&format!("zzpdspawn-{}", unique_suffix()));
    let st = Arc::new(st);

    std::env::remove_var(COMPLETED_BAR_TICK_SECS_ENV);
    let first = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(first, AutonomousCompletedBarTaskSpawnOutcome::Started);
    let second = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(
        second,
        AutonomousCompletedBarTaskSpawnOutcome::AlreadyRunning,
        "at most one completed-bar task may be active per AppState"
    );

    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
}
