//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE:
//! the integrated Phase E closure proof. Every test in this file drives the
//! *real* production seams end-to-end against the real, isolated port-5434
//! test database:
//!
//!   E2A coverage authority (`state::autonomous_daily_coverage_authority`)
//!   -> E2B classifier/finalizer (`state::autonomous_daily_outcome`)
//!   -> E3 coordinator routing (`state::autonomous_daily_coordinator::tick_autonomous_daily_coordinator`
//!      via `state::run_durable_session_controller_tick`)
//!   -> E4 read-only API (`routes::build_router`, the real `GET
//!      /api/v1/autonomous/daily-operation[s]`/`readiness`/`paper-status`/
//!      `system/preflight` routes)
//!
//! No production behavior is added or changed by this file. Discord
//! notifications are captured by an in-process loopback HTTP sink -- no real
//! Discord/network call is ever made anywhere in this file. No real
//! market-data provider or broker call is made; no paper or live order is
//! submitted. This file's fixture/helper style deliberately mirrors the
//! accepted `scenario_autonomous_daily_outcome_coordinator_integration_01.rs`
//! (E3) and `scenario_autonomous_daily_operation_api_01.rs` (E4) files --
//! each Rust integration-test binary is its own crate, so the helpers below
//! are necessarily re-declared here rather than imported, but the identity
//! plumbing, fixture-construction technique, and restart-proof convention
//! (a brand-new `Arc<AppState>` standing in for a fresh process, exactly as
//! `ci_09_10` already established as this crate's accepted restart-proof
//! pattern) are unchanged from those two accepted files.
//!
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_phase_e_closure_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use mqk_daemon::daily_data_readiness;
use mqk_daemon::notify::DiscordNotifier;
use mqk_daemon::routes::build_router;
use mqk_daemon::state::autonomous_daily_coordinator::{
    tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
    AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::autonomous_daily_coverage_authority::{
    construct_coverage_bound_detail, coverage_construction_inputs_from_operation,
    resolve_current_coverage_policy_inputs, write_and_confirm_coverage_authority,
    CoverageAuthorityEnsureResult,
};
use mqk_daemon::state::autonomous_daily_outcome::derive_expected_bar_set;
use mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
use mqk_daemon::state::{
    resolve_autonomous_daily_session_plan_from_env, run_durable_session_controller_tick, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    BrokerKind, DeploymentMode, OperatorAuthMode,
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared test-env plumbing (mirrors ci_*/e4api_* helpers in the accepted E3/
// E4 scenario files -- necessarily re-declared, each integration-test binary
// is its own crate).
// ---------------------------------------------------------------------------

const STRATEGY_ID: &str = "intraday_scalper";
const SYMBOL: &str = "ZZE5PE";
const TIMEFRAME: &str = "5m";

/// Same fixed, known-applicable-trading-day anchor
/// `scenario_autonomous_daily_phase_d_integration_01.rs`'s `pd_now` and E3's
/// `ci_now` both use.
const PE_READINESS_FIXED_TS: i64 = 1_713_188_100 + 10 * 300;

fn pe_now() -> DateTime<Utc> {
    Utc.timestamp_opt(PE_READINESS_FIXED_TS, 0)
        .single()
        .expect("valid fixed readiness timestamp")
}

fn pe_clear_session_override_env() {
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");
}

fn pe_plan() -> AutonomousDailySessionPlan {
    pe_clear_session_override_env();
    let timing = AutonomousDailyPlanTiming::production_default();
    match resolve_autonomous_daily_session_plan_from_env(pe_now(), &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => panic!("phase-e5: expected an applicable session plan, got {other:?}"),
    }
}

fn pe_parse_market_date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("valid market_date")
}

fn unique_adapter_id(label: &str) -> String {
    format!("zze5pe-{}-{}", label, &Uuid::new_v4().to_string()[..8])
}

/// Set `MQK_DAEMON_ADAPTER_ID`/`MQK_DAEMON_DEPLOYMENT_MODE` (read once at
/// `AppState` construction) and the strategy env vars a real coordinator
/// tick's own `build_multi_symbol_runtime_config_from_env`/
/// `resolve_autonomous_runtime_context` will resolve. Must be called before
/// constructing `AppState` for this test.
fn pe_set_env(adapter_id: &str) {
    std::env::set_var("MQK_DAEMON_ADAPTER_ID", adapter_id);
    std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
    std::env::set_var("MQK_STRATEGY_IDS", STRATEGY_ID);
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", TIMEFRAME);
    std::env::set_var(daily_data_readiness::GRACE_SECS_ENV, "0");
    pe_clear_session_override_env();
}

/// Clears the strategy identity env vars only -- models a current
/// policy-resolution failure without disturbing which operation identity the
/// coordinator looks up. Unused by this file's required proofs directly but
/// kept for parity/documentation with the accepted E3 fixture family; no
/// dead-code warning results because every helper below is exercised by at
/// least one test.
#[allow(dead_code)]
fn pe_clear_strategy_env() {
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_IDS");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
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
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from oms_inbox where run_id in (select run_id from runs where engine_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean inbox");
    sqlx::query("delete from oms_outbox where run_id in (select run_id from runs where engine_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean outbox");
    sqlx::query("delete from strategy_signal_evaluations where run_id in (select run_id from runs where engine_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean evaluations");
    sqlx::query("delete from runs where engine_id like 'zze5pe%'")
        .execute(&pool)
        .await
        .expect("clean runs");
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean claims");
    sqlx::query("delete from sys_autonomous_session_events where id like 'autonomous_daily_coverage_bound:%' and id in (select 'autonomous_daily_coverage_bound:' || operation_id::text from sys_autonomous_daily_operations where adapter_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean coverage events");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze5pe%')")
        .execute(&pool).await.expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zze5pe%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    Some(pool)
}

/// Deterministic recorder for the loopback Discord sink's captured payloads.
///
/// Every payload is stored, and only *after* it is durably stored is the new
/// count published on a `tokio::sync::watch` channel -- the watch channel is
/// the sink's own deterministic completion signal. A `watch::Receiver`
/// always exposes the latest published value via `borrow()` and its
/// `changed()` future cannot miss an update that happened before it was
/// polled, so this carries no arbitrary-delay race the way a fixed sleep
/// would.
struct PeAlertRecorder {
    received: tokio::sync::Mutex<Vec<serde_json::Value>>,
    count_tx: tokio::sync::watch::Sender<usize>,
}

impl PeAlertRecorder {
    fn new() -> (Arc<Self>, tokio::sync::watch::Receiver<usize>) {
        let (count_tx, count_rx) = tokio::sync::watch::channel(0usize);
        (
            Arc::new(Self {
                received: tokio::sync::Mutex::new(Vec::new()),
                count_tx,
            }),
            count_rx,
        )
    }

    /// Store one received payload, then publish the new count. The publish
    /// happens strictly after the payload is stored, so any waiter observing
    /// the new count via `wait_for_alert_count` is guaranteed to observe the
    /// payload too.
    async fn record(&self, payload: serde_json::Value) {
        let new_len = {
            let mut guard = self.received.lock().await;
            guard.push(payload);
            guard.len()
        };
        let _ = self.count_tx.send(new_len);
    }

    /// Immediate (non-waiting) count read. Valid only where the caller has
    /// independently established that the production notifier call for the
    /// preceding tick was already fully `.await`ed before that tick's own
    /// `.await` returned -- true for every coordinator tick in this file:
    /// `notify_run_status`/`notify_critical_alert` are called with a direct
    /// `.await` inside `run_durable_session_controller_tick`'s outcome match
    /// arm, never spawned onto a separate task. Used only for the
    /// zero-new-notification (replay) proofs, immediately after the
    /// coordinator tick whose absence of a new notification is being proven.
    async fn count_now(&self) -> usize {
        self.received.lock().await.len()
    }

    async fn snapshot(&self) -> Vec<serde_json::Value> {
        self.received.lock().await.clone()
    }
}

/// Deterministically wait for the sink to have recorded at least `expected`
/// payloads, driven entirely by [`PeAlertRecorder`]'s own watch-channel
/// completion signal -- never an arbitrary delay. The bounded
/// `tokio::time::timeout` is deadlock/failure protection only: it never
/// makes the assertion pass, it only prevents a genuinely stuck test (e.g. a
/// notification that never arrives) from hanging forever.
async fn wait_for_alert_count(
    recorder: &Arc<PeAlertRecorder>,
    count_rx: &mut tokio::sync::watch::Receiver<usize>,
    expected: usize,
) -> Vec<serde_json::Value> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if *count_rx.borrow() >= expected {
                return;
            }
            count_rx
                .changed()
                .await
                .expect("alert recorder sender dropped while a test still awaited it");
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for alert count >= {expected} (deadlock/failure protection only); \
             last observed count = {}",
            *count_rx.borrow()
        )
    });
    recorder.snapshot().await
}

/// In-process loopback HTTP sink capturing every Discord webhook POST body --
/// never a real network call.
async fn pe_webhook_sink() -> (
    String,
    Arc<PeAlertRecorder>,
    tokio::sync::watch::Receiver<usize>,
) {
    let (recorder, count_rx) = PeAlertRecorder::new();
    let sink = Arc::clone(&recorder);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = Arc::clone(&sink);
            async move {
                sink.record(body).await;
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (
        format!("http://127.0.0.1:{}/", addr.port()),
        recorder,
        count_rx,
    )
}

/// Build a real `Arc<AppState>` wired to the test DB pool, with
/// `discord_notifier` overridden to the loopback sink -- bypasses
/// `DiscordNotifier`'s env-based constructor, so no real webhook URL is ever
/// consulted. Caller must have already called [`pe_set_env`] this tick.
async fn pe_daemon_state(pool: sqlx::PgPool, webhook_url: &str) -> Arc<AppState> {
    let mut inner =
        AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    inner.discord_notifier = DiscordNotifier::from_url(webhook_url);
    Arc::new(inner)
}

fn pe_create_args(
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    assignment_identity: String,
    runtime_binding_identity: String,
    initial_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::CreateAutonomousDailyOperationArgs {
    mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: Uuid::new_v4(),
        market_date: pe_parse_market_date(&plan.market_date),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity,
        runtime_binding_identity,
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: pe_parse_market_date(&plan.previous_trading_date),
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "phase-e5 closure test fixture".to_string(),
        stop_attempt_count: 0,
    }
}

async fn pe_transition(
    pool: &sqlx::PgPool,
    operation: &mqk_db::AutonomousDailyOperationRecord,
    new_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: new_state.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "phase-e5 closure test transition".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

fn pe_matching_config() -> mqk_daemon::state::MultiSymbolRuntimeConfig {
    mqk_daemon::state::MultiSymbolRuntimeConfig {
        schema_version: mqk_daemon::state::MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: vec![mqk_daemon::state::SymbolStrategyAssignment {
            symbol: SYMBOL.to_string(),
            strategy_id: STRATEGY_ID.to_string(),
            timeframe: TIMEFRAME.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: mqk_daemon::state::MultiSymbolConfigSource::EnvSingleSymbolFallback,
    }
}

fn pe_matching_binding() -> mqk_runtime::native_strategy::EffectiveRuntimeBinding {
    mqk_runtime::native_strategy::EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(STRATEGY_ID.to_string()),
        effective_runtime_target_symbol: Some(SYMBOL.to_string()),
        effective_runtime_timeframe_secs: Some(300),
    }
}

fn pe_matching_identities() -> (String, String) {
    let config = pe_matching_config();
    let binding = pe_matching_binding();
    (
        mqk_daemon::state::derive_assignment_identity(&config),
        mqk_daemon::state::derive_runtime_binding_identity(&binding),
    )
}

/// Build a real, durably-anchored `stopping` operation, bind a real coverage
/// authority, run it under exactly one `run_id`, and complete every expected
/// bar's dispatch claim with a real, flat (or filled) strategy evaluation --
/// adapted verbatim from the accepted E3 `ci_clean_fixture`/E2B
/// `integrated_fixture` pattern.
async fn pe_clean_fixture(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
    with_fill: bool,
) -> (mqk_db::AutonomousDailyOperationRecord, Uuid) {
    let config = pe_matching_config();
    let binding = pe_matching_binding();
    let mut registry = mqk_strategy::PluginRegistry::new();
    let _ = mqk_strategy::engines::register_builtin_strategies(&mut registry, String::new());
    let (assignment_identity, runtime_binding_identity) = pe_matching_identities();

    let exchange_open = plan.exchange_session_open_utc;
    let exchange_close = plan.exchange_session_close_utc;
    let effective_open = exchange_open + chrono::Duration::minutes(30);
    let effective_close = effective_open + chrono::Duration::minutes(11);

    let mut args = pe_create_args(
        plan,
        adapter_id,
        assignment_identity,
        runtime_binding_identity,
        mqk_db::STATE_AWAITING_OPEN,
        now_utc,
    );
    args.effective_operation_open_utc = effective_open;
    args.effective_operation_close_utc = effective_close;
    args.exchange_session_open_utc = exchange_open;
    args.exchange_session_close_utc = exchange_close;
    args.postclose_finalize_utc = effective_close + chrono::Duration::minutes(15);
    let operation_id = args.operation_id;

    let operation = match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };

    let policy = resolve_current_coverage_policy_inputs(&config, &binding, &registry)
        .expect("policy resolves for the fixture's own config/binding/registry");
    let construction_inputs = coverage_construction_inputs_from_operation(
        &operation,
        &operation.assignment_identity,
        &operation.runtime_binding_identity,
        &policy,
    )
    .expect("operation carries exchange-calendar columns");
    let detail = construct_coverage_bound_detail(&NyseWeekdaysProvider, &construction_inputs)
        .expect("construction succeeds for an ordinary trading day");
    match write_and_confirm_coverage_authority(pool, &detail, now_utc)
        .await
        .expect("write ok")
    {
        CoverageAuthorityEnsureResult::Bound(_) => {}
        other => panic!("expected Bound, got {other:?}"),
    }

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: adapter_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now_utc,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert run ok");

    let started = pe_transition(pool, &operation, mqk_db::STATE_START_RETRYING, now_utc).await;
    let to_running = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: started.operation_id,
        expected_state: started.state.clone(),
        expected_state_version: started.state_version,
        run_id,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "phase-e5 fixture: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running)
        .await
        .expect("ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    pe_complete_expected_bars(pool, operation_id, run_id, &detail, now_utc).await;

    if with_fill {
        mqk_db::inbox_insert_deduped_with_identity(
            pool,
            run_id,
            "zze5pe-broker-msg-1",
            Some("zze5pe-fill-1"),
            "zze5pe-internal-order-1",
            "zze5pe-broker-order-1",
            "fill",
            &serde_json::json!({"event_kind": "fill"}),
            now_utc.timestamp_millis(),
            now_utc,
        )
        .await
        .expect("insert fill ok");
    }

    let stopping = pe_transition(pool, &running, mqk_db::STATE_STOPPING, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, stopping.operation_id, now_utc)
        .await
        .expect("stop ok");
    let final_op = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");

    (final_op, run_id)
}

/// Complete every expected bar's dispatch claim under `run_id` with a real,
/// flat (`signal_generated=false`, `signal_qty=0`) `strategy_evaluated` row.
async fn pe_complete_expected_bars(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    run_id: Uuid,
    detail: &mqk_daemon::state::autonomous_daily_coverage_authority::CoverageBoundDetail,
    now_utc: DateTime<Utc>,
) {
    for &bar_end_ts in &derive_expected_bar_set(detail) {
        match mqk_db::claim_autonomous_daily_bar_dispatch(
            pool,
            operation_id,
            SYMBOL,
            TIMEFRAME,
            bar_end_ts,
            now_utc,
        )
        .await
        .expect("claim ok")
        {
            mqk_db::BarDispatchClaimOutcome::Claimed => {}
            other => panic!("expected Claimed, got {other:?}"),
        }
        let evaluation_id = Uuid::new_v4();
        mqk_db::insert_strategy_signal_evaluation(
            pool,
            &mqk_db::InsertStrategySignalEvaluationArgs {
                evaluation_id,
                ts_utc: now_utc,
                run_id: Some(run_id),
                strategy_id: STRATEGY_ID.to_string(),
                symbol: SYMBOL.to_string(),
                timeframe: TIMEFRAME.to_string(),
                bar_context_source: "db_loaded".to_string(),
                bars_loaded: 1,
                latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(bar_end_ts, 0),
                signal_generated: false,
                signal_qty: Some(0),
                signal_side: None,
                reason_code: "flat".to_string(),
                reason: "flat".to_string(),
                decision_stage: "strategy_evaluated".to_string(),
                source: "test".to_string(),
            },
        )
        .await
        .expect("insert evaluation ok");
        mqk_db::record_completed_bar_observed(pool, operation_id, bar_end_ts, now_utc)
            .await
            .expect("observe ok");
        mqk_db::complete_autonomous_daily_bar_dispatch(
            pool,
            operation_id,
            SYMBOL,
            TIMEFRAME,
            bar_end_ts,
            now_utc,
            Some(evaluation_id),
        )
        .await
        .expect("complete ok");
    }
}

/// INTEGRATED PROOF B fixture: two runs in one operation's full lineage.
/// Run A durably records a broker fill *and* a broker ack; the operation
/// then recovers (a real `running -> recovery_retrying -> running` cycle,
/// binding run B); run B completes every expected bar's dispatch claim with
/// a real flat evaluation; the operation stops. The operation's mutable
/// `run_id` column ends up pointing at run B, while run A's fill/ack
/// evidence must still be counted (E1 contract §5/§6b: full-lineage scoping,
/// never the current `run_id` column alone).
async fn pe_two_run_activity_fixture(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
) -> (mqk_db::AutonomousDailyOperationRecord, Uuid, Uuid) {
    let config = pe_matching_config();
    let binding = pe_matching_binding();
    let mut registry = mqk_strategy::PluginRegistry::new();
    let _ = mqk_strategy::engines::register_builtin_strategies(&mut registry, String::new());
    let (assignment_identity, runtime_binding_identity) = pe_matching_identities();

    let exchange_open = plan.exchange_session_open_utc;
    let exchange_close = plan.exchange_session_close_utc;
    let effective_open = exchange_open + chrono::Duration::minutes(30);
    let effective_close = effective_open + chrono::Duration::minutes(11);

    let mut args = pe_create_args(
        plan,
        adapter_id,
        assignment_identity,
        runtime_binding_identity,
        mqk_db::STATE_AWAITING_OPEN,
        now_utc,
    );
    args.effective_operation_open_utc = effective_open;
    args.effective_operation_close_utc = effective_close;
    args.exchange_session_open_utc = exchange_open;
    args.exchange_session_close_utc = exchange_close;
    args.postclose_finalize_utc = effective_close + chrono::Duration::minutes(15);
    let operation_id = args.operation_id;

    let operation = match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };

    let policy = resolve_current_coverage_policy_inputs(&config, &binding, &registry)
        .expect("policy resolves");
    let construction_inputs = coverage_construction_inputs_from_operation(
        &operation,
        &operation.assignment_identity,
        &operation.runtime_binding_identity,
        &policy,
    )
    .expect("operation carries exchange-calendar columns");
    let detail = construct_coverage_bound_detail(&NyseWeekdaysProvider, &construction_inputs)
        .expect("construction succeeds for an ordinary trading day");
    match write_and_confirm_coverage_authority(pool, &detail, now_utc)
        .await
        .expect("write ok")
    {
        CoverageAuthorityEnsureResult::Bound(_) => {}
        other => panic!("expected Bound, got {other:?}"),
    }

    // Run A: bound, records real fill + ack evidence, then interrupted.
    let run_a = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id: run_a,
            engine_id: adapter_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now_utc,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert run A ok");

    let started = pe_transition(pool, &operation, mqk_db::STATE_START_RETRYING, now_utc).await;
    let to_running_a = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: started.operation_id,
        expected_state: started.state.clone(),
        expected_state_version: started.state_version,
        run_id: run_a,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "phase-e5 proof-b fixture: run A running".to_string(),
    };
    let running_a =
        match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running_a)
            .await
            .expect("ok")
        {
            mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };

    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_a,
        "zze5pe-b-broker-msg-fill",
        Some("zze5pe-b-fill-1"),
        "zze5pe-b-internal-order-1",
        "zze5pe-b-broker-order-1",
        "fill",
        &serde_json::json!({"event_kind": "fill"}),
        now_utc.timestamp_millis(),
        now_utc,
    )
    .await
    .expect("insert run-A fill ok");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_a,
        "zze5pe-b-broker-msg-ack",
        None,
        "zze5pe-b-internal-order-2",
        "zze5pe-b-broker-order-2",
        "ack",
        &serde_json::json!({"event_kind": "ack"}),
        now_utc.timestamp_millis(),
        now_utc,
    )
    .await
    .expect("insert run-A ack ok");

    // Interrupted -> recovery -> run B (mirrors the accepted E2A
    // `h01_initial_and_recovery_run_lineage_read_and_validated` cycle).
    let interrupted_args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: running_a.operation_id,
        expected_state: running_a.state.clone(),
        expected_state_version: running_a.state_version,
        new_state: mqk_db::STATE_RECOVERY_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "phase-e5 proof-b fixture: interrupted".to_string(),
    };
    let recovering = match mqk_db::transition_autonomous_daily_operation(pool, &interrupted_args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    let run_b = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id: run_b,
            engine_id: adapter_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now_utc,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert run B ok");
    let to_running_b = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: recovering.operation_id,
        expected_state: recovering.state.clone(),
        expected_state_version: recovering.state_version,
        run_id: run_b,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "phase-e5 proof-b fixture: run B (recovery)".to_string(),
    };
    let running_b =
        match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running_b)
            .await
            .expect("ok")
        {
            mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };

    // Run B completes every expected bar -- full coverage proof, scoped to
    // the operation (not to any one run_id).
    pe_complete_expected_bars(pool, operation_id, run_b, &detail, now_utc).await;

    let stopping = pe_transition(pool, &running_b, mqk_db::STATE_STOPPING, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, stopping.operation_id, now_utc)
        .await
        .expect("stop ok");
    let final_op = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        final_op.run_id,
        Some(run_b),
        "fixture: mutable run_id column points at run B (the later run)"
    );

    (final_op, run_a, run_b)
}

/// Drives one real production tick -- the sole seam that performs the
/// durable coordinator tick *and* its bounded logging/notification, exactly
/// as production behaves.
async fn pe_tick(st: &Arc<AppState>, now_utc: DateTime<Utc>) {
    run_durable_session_controller_tick(st, now_utc).await;
}

/// Direct call to the coordinator entry point, returning its typed outcome
/// (no notification is sent by this path).
async fn pe_tick_typed(
    st: &Arc<AppState>,
    now_utc: DateTime<Utc>,
) -> AutonomousDailyCoordinatorTickOutcome {
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput { state: st, now_utc })
        .await
        .expect("coordinator tick must not error")
}

async fn pe_events_count(pool: &sqlx::PgPool, operation_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("count ok")
}

/// A durable, before/after-comparable snapshot of every fact INTEGRATED
/// PROOF E's read-only guarantee requires unchanged.
///
/// Scope authority is the *validated full run lineage*
/// (`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`),
/// never a single caller-supplied `run_id` -- every lineage-scoped count
/// below sums across every run in that lineage, exactly as the accepted E4
/// read routes themselves are required to. The global table totals exist
/// so a GET route that creates an unrelated row under a newly generated
/// identity cannot escape this snapshot by falling outside the
/// operation-scoped counts.
#[derive(Debug, PartialEq, Eq)]
struct PeSnapshot {
    // Operation-scoped, validated-lineage-derived facts.
    state: String,
    state_version: i64,
    lifecycle_event_count: i64,
    coverage_event_count: i64,
    raw_running_transition_count: i64,
    validated_lineage_run_count: i64,
    lineage_run_table_row_count: i64,
    claim_count: i64,
    evaluation_count: i64,
    outbox_count: i64,
    inbox_count: i64,
    // Global evidence-table totals (unrelated-row escape-hatch proof).
    global_runs: i64,
    global_bar_dispatches: i64,
    global_strategy_evaluations: i64,
    global_outbox: i64,
    global_inbox: i64,
    global_operation_events: i64,
    global_session_events: i64,
}

async fn pe_snapshot(pool: &sqlx::PgPool, operation_id: Uuid) -> PeSnapshot {
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");

    // Full validated run lineage -- never a caller-supplied single run_id.
    let raw_rows =
        mqk_db::fetch_autonomous_daily_operation_running_transitions_raw(pool, operation_id)
            .await
            .expect("fetch raw lineage rows ok");
    let raw_running_transition_count = raw_rows.len() as i64;
    let lineage =
        match mqk_db::validate_autonomous_daily_operation_run_lineage(&raw_rows, record.run_id) {
            Ok(lineage) => lineage,
            Err(err) => panic!(
                "pe_snapshot: the operation's run lineage must never be contradictory, got {err:?}"
            ),
        };
    let validated_lineage_run_count = lineage.len() as i64;

    let lifecycle_event_count = pe_events_count(pool, operation_id).await;
    let coverage_event_count: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_session_events where id = $1")
            .bind(format!("autonomous_daily_coverage_bound:{operation_id}"))
            .fetch_one(pool)
            .await
            .expect("count ok");

    // Dispatch claims are scoped to the operation itself, never to a single
    // run_id -- the same shape `pe_complete_expected_bars` writes against.
    let claim_count: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_bar_dispatches where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("count ok");

    let mut lineage_run_table_row_count: i64 = 0;
    let mut evaluation_count: i64 = 0;
    let mut outbox_count: i64 = 0;
    let mut inbox_count: i64 = 0;
    for run_id in &lineage {
        lineage_run_table_row_count +=
            sqlx::query_scalar::<_, i64>("select count(*) from runs where run_id = $1")
                .bind(*run_id)
                .fetch_one(pool)
                .await
                .expect("count ok");
        evaluation_count += sqlx::query_scalar::<_, i64>(
            "select count(*) from strategy_signal_evaluations where run_id = $1",
        )
        .bind(*run_id)
        .fetch_one(pool)
        .await
        .expect("count ok");
        outbox_count +=
            sqlx::query_scalar::<_, i64>("select count(*) from oms_outbox where run_id = $1")
                .bind(*run_id)
                .fetch_one(pool)
                .await
                .expect("count ok");
        inbox_count +=
            sqlx::query_scalar::<_, i64>("select count(*) from oms_inbox where run_id = $1")
                .bind(*run_id)
                .fetch_one(pool)
                .await
                .expect("count ok");
    }

    let global_runs: i64 = sqlx::query_scalar("select count(*) from runs")
        .fetch_one(pool)
        .await
        .expect("count ok");
    let global_bar_dispatches: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_daily_bar_dispatches")
            .fetch_one(pool)
            .await
            .expect("count ok");
    let global_strategy_evaluations: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations")
            .fetch_one(pool)
            .await
            .expect("count ok");
    let global_outbox: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(pool)
        .await
        .expect("count ok");
    let global_inbox: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(pool)
        .await
        .expect("count ok");
    let global_operation_events: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_daily_operation_events")
            .fetch_one(pool)
            .await
            .expect("count ok");
    let global_session_events: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_session_events")
            .fetch_one(pool)
            .await
            .expect("count ok");

    PeSnapshot {
        state: record.state,
        state_version: record.state_version,
        lifecycle_event_count,
        coverage_event_count,
        raw_running_transition_count,
        validated_lineage_run_count,
        lineage_run_table_row_count,
        claim_count,
        evaluation_count,
        outbox_count,
        inbox_count,
        global_runs,
        global_bar_dispatches,
        global_strategy_evaluations,
        global_outbox,
        global_inbox,
        global_operation_events,
        global_session_events,
    }
}

async fn get_json(router: axum::Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, j)
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF A -- clean no-trade day
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e5_proof_a_clean_no_trade_day_full_pipeline_and_replay() {
    let Some(pool) = maybe_db("e5_proof_a").await else {
        return;
    };
    let adapter_id = unique_adapter_id("proofa");
    pe_set_env(&adapter_id);
    let plan = pe_plan();
    let (webhook_url, alerts, mut alert_count_rx) = pe_webhook_sink().await;
    let (operation, _run_id) = pe_clean_fixture(&pool, &plan, &adapter_id, pe_now(), false).await;
    let st = pe_daemon_state(pool.clone(), &webhook_url).await;

    // Durable path: creation -> coverage authority -> validated lineage ->
    // every expected bar completed with a matching evaluation -> durable
    // stop -> coordinator finalization -> completed_no_trade.
    pe_tick(&st, pe_now()).await;

    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
    assert_eq!(
        record.outcome.as_deref(),
        Some(mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL)
    );
    assert!(record.finalized_at_utc.is_some());

    let captured = wait_for_alert_count(&alerts, &mut alert_count_rx, 1).await;
    assert_eq!(
        captured.len(),
        1,
        "exactly one lifecycle finalization notification, got {captured:?}"
    );
    assert_eq!(captured[0]["event"], "autonomous.daily_operation.outcome");

    // E4 single-route read: truth_state active, outcome_class no_trade,
    // finalization_status finalized, evidence_state complete, exact
    // non-null count fields.
    let router = build_router(Arc::clone(&st));
    let (status, body) = get_json(
        router,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    let row = &body["operation"];
    assert_eq!(row["outcome_class"].as_str().unwrap(), "no_trade");
    assert_eq!(
        row["outcome_reason_code"].as_str().unwrap(),
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
    );
    assert_eq!(row["finalization_status"].as_str().unwrap(), "finalized");
    assert_eq!(row["evidence_state"].as_str().unwrap(), "complete");
    assert!(row["strategy_evaluation_count"].as_i64().unwrap() > 0);
    assert_eq!(row["order_activity_count"].as_i64().unwrap(), 0);
    assert_eq!(row["fill_count"].as_i64().unwrap(), 0);

    // A direct typed coordinator-tick call (no notification path) against
    // the already-terminal row must itself report the bounded typed
    // already-finalized projection, read-only.
    let typed_replay = pe_tick_typed(&st, pe_now() + chrono::Duration::milliseconds(500)).await;
    assert!(
        matches!(
            typed_replay,
            AutonomousDailyCoordinatorTickOutcome::OutcomeAlreadyFinalized { .. }
        ),
        "expected OutcomeAlreadyFinalized, got {typed_replay:?}"
    );

    let events_before_tick_replay = pe_events_count(&pool, operation.operation_id).await;

    // Repeat the coordinator tick: zero duplicate lifecycle event, zero
    // duplicate notification, same durable outcome. The tick's own accepted
    // compatibility read-surface write (`AutonomousSessionTruth` ->
    // `sys_autonomous_session_events`, orthogonal to this operation's E1–E4
    // durable lifecycle/outcome truth and unconditionally re-asserted by
    // `log_coordinator_outcome` on every tick, including a read-only
    // `OutcomeAlreadyFinalized` replay) is expected here and is deliberately
    // outside the read-only-API snapshot window below, which instead proves
    // the GET call itself -- not the coordinator tick -- causes zero DB
    // mutation of any kind.
    pe_tick(&st, pe_now() + chrono::Duration::seconds(1)).await;
    let events_after_tick_replay = pe_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_before_tick_replay, events_after_tick_replay,
        "zero duplicate lifecycle event from the replay coordinator tick"
    );

    let snapshot_before_api_read = pe_snapshot(&pool, operation.operation_id).await;
    let router2 = build_router(Arc::clone(&st));
    let (status2, body2) = get_json(
        router2,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(
        body2["operation"]["outcome_class"], body["operation"]["outcome_class"],
        "replay must observe the identical durable outcome"
    );

    let snapshot_after_api_read = pe_snapshot(&pool, operation.operation_id).await;
    assert_eq!(
        snapshot_before_api_read, snapshot_after_api_read,
        "zero DB mutation of any kind (operation-scoped or global) from the read-only API call"
    );

    assert_eq!(
        alerts.count_now().await,
        1,
        "zero duplicate notification on replay"
    );
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF B -- activity day, full lineage across two runs
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e5_proof_b_activity_day_full_lineage_across_two_runs() {
    let Some(pool) = maybe_db("e5_proof_b").await else {
        return;
    };
    let adapter_id = unique_adapter_id("proofb");
    pe_set_env(&adapter_id);
    let plan = pe_plan();
    let (webhook_url, alerts, mut alert_count_rx) = pe_webhook_sink().await;
    let (operation, run_a, run_b) =
        pe_two_run_activity_fixture(&pool, &plan, &adapter_id, pe_now()).await;
    let st = pe_daemon_state(pool.clone(), &webhook_url).await;

    pe_tick(&st, pe_now()).await;

    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
    assert_eq!(
        record.outcome.as_deref(),
        Some(mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED),
        "run A's fill evidence must decide the classification even though the mutable \
         run_id column points at run B"
    );
    assert_eq!(record.run_id, Some(run_b));

    let captured_b = wait_for_alert_count(&alerts, &mut alert_count_rx, 1).await;
    assert_eq!(captured_b.len(), 1, "exactly one terminal notification");

    let router = build_router(Arc::clone(&st));
    let (status, body) = get_json(
        router,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["outcome_class"].as_str().unwrap(), "with_activity");
    assert_eq!(row["finalization_status"].as_str().unwrap(), "finalized");
    assert_eq!(row["evidence_state"].as_str().unwrap(), "complete");

    // E4's full-lineage counts must include run A's activity.
    assert_eq!(
        row["fill_count"].as_i64().unwrap(),
        1,
        "fill_count must include run A's fill even though it is not the current run"
    );
    assert!(
        row["order_activity_count"].as_i64().unwrap() >= 1,
        "order_activity_count must include run A's ack (the accepted E4 definition: \
         outbox rows plus ack/cancel_ack/replace_ack/reject inbox rows)"
    );
    let strategy_eval_count = row["strategy_evaluation_count"].as_i64().unwrap();
    assert!(
        strategy_eval_count > 0,
        "strategy_evaluation_count must span the full lineage (run B's flat evaluations)"
    );

    // Independently verify the full-lineage count spans both runs via a
    // direct DB read, cross-checking the API's own claim.
    let eval_count_run_a: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where run_id = $1")
            .bind(run_a)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    let eval_count_run_b: i64 =
        sqlx::query_scalar("select count(*) from strategy_signal_evaluations where run_id = $1")
            .bind(run_b)
            .fetch_one(&pool)
            .await
            .expect("count ok");
    assert_eq!(
        eval_count_run_a, 0,
        "fixture: run A recorded no evaluations"
    );
    assert_eq!(
        strategy_eval_count, eval_count_run_b,
        "the API's lineage-scoped count must equal run B's real evaluation count"
    );

    // Replay: zero duplicate notification.
    pe_tick(&st, pe_now() + chrono::Duration::seconds(1)).await;
    assert_eq!(
        alerts.count_now().await,
        1,
        "zero replay notification for the activity-day terminal row"
    );
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF C -- evidence blocker and recovery
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e5_proof_c_evidence_blocker_notifies_once_and_recovers() {
    let Some(pool) = maybe_db("e5_proof_c").await else {
        return;
    };
    let adapter_id = unique_adapter_id("proofc");
    pe_set_env(&adapter_id);
    let plan = pe_plan();
    let (webhook_url, alerts, mut alert_count_rx) = pe_webhook_sink().await;
    let (operation, run_id) = pe_clean_fixture(&pool, &plan, &adapter_id, pe_now(), false).await;

    // Corrupt one expected bar's claim to `failed` -- the finalization-
    // eligible operation's expected claim/evaluation evidence is now
    // incomplete.
    let corrupted_bar_end_ts: i64 = sqlx::query_scalar(
        "select bar_end_ts from sys_autonomous_daily_bar_dispatches where operation_id = $1 \
         order by bar_end_ts limit 1",
    )
    .bind(operation.operation_id)
    .fetch_one(&pool)
    .await
    .expect("query ok");
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'failed', evaluation_id = null, \
         completed_at_utc = null where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation.operation_id)
    .bind(corrupted_bar_end_ts)
    .execute(&pool)
    .await
    .expect("corrupt claim ok");

    let st = pe_daemon_state(pool.clone(), &webhook_url).await;

    // coordinator tick -> evidence_degraded, exact closed unknown_* reason,
    // outcome/finalized_at_utc null, one warning notification.
    pe_tick(&st, pe_now()).await;
    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(degraded.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert_eq!(
        degraded.state_reason_code.as_deref(),
        Some("unknown_unresolved_dispatch_claim")
    );
    assert!(degraded.outcome.is_none());
    assert!(degraded.finalized_at_utc.is_none());

    let captured_c1 = wait_for_alert_count(&alerts, &mut alert_count_rx, 1).await;
    assert_eq!(
        captured_c1.len(),
        1,
        "one warning notification for the newly applied evidence blocker"
    );

    // E4 before repair: blocked_insufficient_evidence, evidence_state
    // degraded, bounded blocker reason.
    let router_before = build_router(Arc::clone(&st));
    let (status_before, body_before) = get_json(
        router_before,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
    )
    .await;
    assert_eq!(status_before, StatusCode::OK);
    let row_before = &body_before["operation"];
    assert_eq!(
        row_before["finalization_status"].as_str().unwrap(),
        "blocked_insufficient_evidence"
    );
    assert_eq!(row_before["evidence_state"].as_str().unwrap(), "degraded");
    let blockers_before = row_before["evidence_blockers"].as_array().unwrap();
    assert!(blockers_before
        .iter()
        .any(|v| v.as_str() == Some("unknown_unresolved_dispatch_claim")));

    let events_after_blocker = pe_events_count(&pool, operation.operation_id).await;

    // Repeat unchanged: zero duplicate lifecycle event, zero duplicate
    // warning.
    pe_tick(&st, pe_now() + chrono::Duration::seconds(1)).await;
    let replay = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(replay.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    let events_after_replay = pe_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_replay, events_after_blocker,
        "an unchanged blocker replay must not durably advance truth"
    );
    assert_eq!(
        alerts.count_now().await,
        1,
        "zero duplicate warning on an unchanged replay"
    );

    // Repair only the test fixture's durable evidence.
    let repaired_evaluation_id: Uuid = sqlx::query_scalar(
        "select evaluation_id from strategy_signal_evaluations where run_id = $1 limit 1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("query ok");
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'completed', \
         evaluation_id = $3, completed_at_utc = now() \
         where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation.operation_id)
    .bind(corrupted_bar_end_ts)
    .bind(repaired_evaluation_id)
    .execute(&pool)
    .await
    .expect("repair claim ok");

    // evidence_degraded -> stopping -> no direct completion during the
    // recovery transition.
    pe_tick(&st, pe_now() + chrono::Duration::seconds(2)).await;
    let recovered = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        recovered.state,
        mqk_db::STATE_STOPPING,
        "repaired evidence must recover to stopping, never complete directly"
    );
    assert!(recovered.outcome.is_none());

    // Later coordinator tick -> completed_no_trade, one terminal
    // notification.
    pe_tick(&st, pe_now() + chrono::Duration::seconds(3)).await;
    let finalized = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(finalized.state, mqk_db::STATE_COMPLETED_NO_TRADE);
    assert!(finalized.finalized_at_utc.is_some());

    let captured_c2 = wait_for_alert_count(&alerts, &mut alert_count_rx, 2).await;
    assert_eq!(
        captured_c2.len(),
        2,
        "one evidence-degraded warning plus one terminal outcome notification"
    );

    // E4 after completion: finalized terminal truth, evidence_state
    // complete.
    let router_after = build_router(Arc::clone(&st));
    let (status_after, body_after) = get_json(
        router_after,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
    )
    .await;
    assert_eq!(status_after, StatusCode::OK);
    let row_after = &body_after["operation"];
    assert_eq!(
        row_after["finalization_status"].as_str().unwrap(),
        "finalized"
    );
    assert_eq!(row_after["evidence_state"].as_str().unwrap(), "complete");
    assert_eq!(row_after["outcome_class"].as_str().unwrap(), "no_trade");
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF D -- restart safety
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e5_proof_d_restart_safety_stop_terminal_and_evidence_blocker() {
    let Some(pool) = maybe_db("e5_proof_d").await else {
        return;
    };

    // --- D1: restart after durable stop, before finalization. ---
    let adapter_id_1 = unique_adapter_id("proofd1");
    pe_set_env(&adapter_id_1);
    let plan_1 = pe_plan();
    let (webhook_url_1, alerts_1, mut alert_count_rx_1) = pe_webhook_sink().await;
    let (operation_1, _run_1) =
        pe_clean_fixture(&pool, &plan_1, &adapter_id_1, pe_now(), false).await;
    assert!(
        operation_1.stopped_at_utc.is_some(),
        "fixture: durably stopped before any AppState observed it"
    );

    // A brand-new AppState -- no in-memory continuity with the raw DB calls
    // that built the fixture above (which never constructed an AppState at
    // all). Mirrors the accepted E3 `ci_09_10` restart-proof convention.
    let st_after_stop = pe_daemon_state(pool.clone(), &webhook_url_1).await;
    pe_tick(&st_after_stop, pe_now()).await;
    let record_after_stop_restart =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_1.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        record_after_stop_restart.state,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        "the next coordinator tick against a fresh AppState must finalize exactly once"
    );
    // D1's own finalizing tick must deterministically notify exactly once --
    // the baseline the D2 zero-delta proof below is measured against.
    let captured_d1 = wait_for_alert_count(&alerts_1, &mut alert_count_rx_1, 1).await;
    assert_eq!(
        captured_d1.len(),
        1,
        "D1's finalizing tick against a fresh AppState must notify exactly once"
    );

    // --- D2: restart after terminal commit. ---
    let st_after_terminal = pe_daemon_state(pool.clone(), &webhook_url_1).await;
    let events_before_terminal_restart = pe_events_count(&pool, operation_1.operation_id).await;
    let alert_count_before_terminal_restart = alerts_1.count_now().await;
    pe_tick(&st_after_terminal, pe_now() + chrono::Duration::seconds(1)).await;
    let record_after_terminal_restart =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_1.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        record_after_terminal_restart.state,
        mqk_db::STATE_COMPLETED_NO_TRADE
    );
    let events_after_terminal_restart = pe_events_count(&pool, operation_1.operation_id).await;
    assert_eq!(
        events_before_terminal_restart, events_after_terminal_restart,
        "zero lifecycle-event delta from a fresh AppState's tick against an already-terminal row"
    );

    // `notify_run_status`/`notify_critical_alert` are `.await`ed directly
    // inside the coordinator's outcome match arm (never spawned), so by the
    // time the tick above has returned any notification it would send has
    // already been fully delivered -- an immediate read proves the delta.
    assert_eq!(
        alerts_1.count_now().await,
        alert_count_before_terminal_restart,
        "zero outcome-notification delta across both restarts"
    );

    // --- D3: restart after evidence blocker, then repair and recover. ---
    let adapter_id_3 = unique_adapter_id("proofd3");
    pe_set_env(&adapter_id_3);
    let plan_3 = pe_plan();
    let (webhook_url_3, alerts_3, mut alert_count_rx_3) = pe_webhook_sink().await;
    let (operation_3, run_3) =
        pe_clean_fixture(&pool, &plan_3, &adapter_id_3, pe_now(), false).await;
    let corrupted_bar_end_ts: i64 = sqlx::query_scalar(
        "select bar_end_ts from sys_autonomous_daily_bar_dispatches where operation_id = $1 \
         order by bar_end_ts limit 1",
    )
    .bind(operation_3.operation_id)
    .fetch_one(&pool)
    .await
    .expect("query ok");
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'failed', evaluation_id = null, \
         completed_at_utc = null where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation_3.operation_id)
    .bind(corrupted_bar_end_ts)
    .execute(&pool)
    .await
    .expect("corrupt claim ok");

    let st_blocker = pe_daemon_state(pool.clone(), &webhook_url_3).await;
    pe_tick(&st_blocker, pe_now()).await;
    let degraded_3 =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_3.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(degraded_3.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    let events_after_first_blocker = pe_events_count(&pool, operation_3.operation_id).await;
    let captured_d3_first = wait_for_alert_count(&alerts_3, &mut alert_count_rx_3, 1).await;
    assert_eq!(captured_d3_first.len(), 1);

    // A fresh AppState replays the unchanged blocker -- silent (zero event
    // delta, zero notification delta).
    let alert_count_before_restarted_replay = alerts_3.count_now().await;
    let st_blocker_restarted = pe_daemon_state(pool.clone(), &webhook_url_3).await;
    pe_tick(
        &st_blocker_restarted,
        pe_now() + chrono::Duration::seconds(1),
    )
    .await;
    let events_after_restarted_replay = pe_events_count(&pool, operation_3.operation_id).await;
    assert_eq!(
        events_after_first_blocker, events_after_restarted_replay,
        "a fresh AppState replaying an unchanged blocker must not durably advance truth"
    );
    // Direct-await notifier call (see D2's comment above): the delta is
    // provable immediately after the tick returns, no wait required.
    assert_eq!(
        alerts_3.count_now().await,
        alert_count_before_restarted_replay,
        "zero additional warning from the restarted replay"
    );

    // Repair, then a fresh AppState recovers through stopping.
    let repaired_evaluation_id: Uuid = sqlx::query_scalar(
        "select evaluation_id from strategy_signal_evaluations where run_id = $1 limit 1",
    )
    .bind(run_3)
    .fetch_one(&pool)
    .await
    .expect("query ok");
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'completed', \
         evaluation_id = $3, completed_at_utc = now() \
         where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation_3.operation_id)
    .bind(corrupted_bar_end_ts)
    .bind(repaired_evaluation_id)
    .execute(&pool)
    .await
    .expect("repair claim ok");

    let st_recovery = pe_daemon_state(pool.clone(), &webhook_url_3).await;
    pe_tick(&st_recovery, pe_now() + chrono::Duration::seconds(2)).await;
    let recovered_3 =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_3.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        recovered_3.state,
        mqk_db::STATE_STOPPING,
        "a fresh AppState's repaired-evidence tick must recover through stopping, not complete \
         directly"
    );

    // A later fresh AppState terminalizes.
    let st_terminal = pe_daemon_state(pool.clone(), &webhook_url_3).await;
    pe_tick(&st_terminal, pe_now() + chrono::Duration::seconds(3)).await;
    let terminal_3 =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_3.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(terminal_3.state, mqk_db::STATE_COMPLETED_NO_TRADE);

    let captured_d3_final = wait_for_alert_count(&alerts_3, &mut alert_count_rx_3, 2).await;
    assert_eq!(
        captured_d3_final.len(),
        2,
        "one warning plus one terminal notification across the full restart-safe blocker/\
         recovery cycle"
    );
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF E -- API read-only guarantee
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e5_proof_e_api_read_only_guarantee() {
    let Some(pool) = maybe_db("e5_proof_e").await else {
        return;
    };
    let adapter_id = unique_adapter_id("proofe");
    pe_set_env(&adapter_id);
    let plan = pe_plan();
    let (webhook_url, _alerts, _alert_count_rx) = pe_webhook_sink().await;
    let (operation, run_a, run_b) =
        pe_two_run_activity_fixture(&pool, &plan, &adapter_id, pe_now()).await;
    let st = pe_daemon_state(pool.clone(), &webhook_url).await;
    // Finalize durably first, via the real coordinator, so the read-only
    // routes below have real terminal truth to project.
    pe_tick(&st, pe_now()).await;

    // Prove a genuine two-run lineage exists before taking the snapshot --
    // Proof E's read-only guarantee must be exercised over the full
    // multi-run lineage, never trivially satisfied by a single-run
    // operation.
    let raw_rows = mqk_db::fetch_autonomous_daily_operation_running_transitions_raw(
        &pool,
        operation.operation_id,
    )
    .await
    .expect("fetch raw lineage rows ok");
    let record_for_lineage =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    let lineage = mqk_db::validate_autonomous_daily_operation_run_lineage(
        &raw_rows,
        record_for_lineage.run_id,
    )
    .expect("proof-e fixture: lineage must validate cleanly");
    assert_eq!(
        lineage.len(),
        2,
        "proof-e fixture must carry exactly two validated lineage runs, got {lineage:?}"
    );
    assert!(
        lineage.contains(&run_a) && lineage.contains(&run_b),
        "the validated lineage must contain both fixture runs"
    );

    let before = pe_snapshot(&pool, operation.operation_id).await;
    assert_eq!(
        before.validated_lineage_run_count, 2,
        "the snapshot itself must observe both lineage runs: {before:?}"
    );
    assert!(
        before.inbox_count >= 2,
        "the snapshot must include run A's fill+ack inbox evidence, not just run B's: {before:?}"
    );
    assert!(
        before.evaluation_count > 0,
        "the snapshot must include run B's flat strategy evaluations: {before:?}"
    );

    let routes = [
        format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan.market_date
        ),
        "/api/v1/autonomous/daily-operations?limit=10".to_string(),
        "/api/v1/autonomous/readiness".to_string(),
        "/api/v1/autonomous/paper-status".to_string(),
        "/api/v1/system/preflight".to_string(),
    ];

    for uri in &routes {
        let router = build_router(Arc::clone(&st));
        let (status, _body) = get_json(router, uri).await;
        assert_eq!(status, StatusCode::OK, "route {uri} must return 200");
    }

    let after = pe_snapshot(&pool, operation.operation_id).await;
    assert_eq!(
        before, after,
        "every delta caused by the five GET routes must be zero: {before:?} vs {after:?}"
    );
}

// ---------------------------------------------------------------------------
// INTEGRATED PROOF F -- fail-soft API truth
// ---------------------------------------------------------------------------

fn pe_no_db_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper))
}

#[tokio::test]
#[ignore]
async fn e5_proof_f_fail_soft_api_truth() {
    let Some(pool) = maybe_db("e5_proof_f").await else {
        return;
    };

    // no operation -> not_found
    let adapter_id_nf = unique_adapter_id("prooffnf");
    let mut st_nf = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_nf.set_adapter_id_for_test(&adapter_id_nf);
    let router_nf = build_router(Arc::new(st_nf));
    let (status_nf, body_nf) = get_json(
        router_nf,
        "/api/v1/autonomous/daily-operation?market_date=2026-07-20",
    )
    .await;
    assert_eq!(status_nf, StatusCode::OK);
    assert_eq!(body_nf["truth_state"].as_str().unwrap(), "not_found");
    assert!(body_nf["operation"].is_null());

    // no DB -> backend_unavailable
    let router_no_db = build_router(pe_no_db_state());
    let (status_no_db, body_no_db) =
        get_json(router_no_db, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status_no_db, StatusCode::OK);
    assert_eq!(
        body_no_db["truth_state"].as_str().unwrap(),
        "backend_unavailable"
    );

    // required DB count read failure -> query_failed
    let adapter_id_qf = unique_adapter_id("prooffqf");
    pe_set_env(&adapter_id_qf);
    let plan_qf = pe_plan();
    let op_qf = match mqk_db::create_or_recover_autonomous_daily_operation(
        &pool,
        &pe_create_args(
            &plan_qf,
            &adapter_id_qf,
            pe_matching_identities().0,
            pe_matching_identities().1,
            mqk_db::STATE_AWAITING_OPEN,
            pe_now(),
        ),
    )
    .await
    .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };
    let mut st_qf_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_qf_inner.set_adapter_id_for_test(&adapter_id_qf);
    st_qf_inner.set_force_activity_counts_database_unavailable_for_test(true);
    let router_qf = build_router(Arc::new(st_qf_inner));
    let (status_qf, body_qf) = get_json(
        router_qf,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan_qf.market_date
        ),
    )
    .await;
    assert_eq!(status_qf, StatusCode::OK);
    assert_eq!(body_qf["truth_state"].as_str().unwrap(), "query_failed");
    assert_eq!(
        body_qf["operation"]["operation_id"].as_str().unwrap(),
        op_qf.operation_id.to_string(),
        "the known durable operation row must be retained even on a count-read failure"
    );
    assert!(body_qf["operation"]["strategy_evaluation_count"].is_null());

    // invalid lineage -> active + evidence unavailable + null counts
    let adapter_id_il = unique_adapter_id("prooffil");
    pe_set_env(&adapter_id_il);
    let plan_il = pe_plan();
    let (assignment_identity_il, runtime_binding_identity_il) = pe_matching_identities();
    let op_il = match mqk_db::create_or_recover_autonomous_daily_operation(
        &pool,
        &pe_create_args(
            &plan_il,
            &adapter_id_il,
            assignment_identity_il,
            runtime_binding_identity_il,
            mqk_db::STATE_AWAITING_OPEN,
            pe_now(),
        ),
    )
    .await
    .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };
    let op_il = pe_transition(&pool, &op_il, mqk_db::STATE_START_RETRYING, pe_now()).await;
    let run_il = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run_il,
            engine_id: adapter_id_il.clone(),
            mode: "PAPER".to_string(),
            started_at_utc: pe_now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert run ok");
    let to_running_il = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: op_il.operation_id,
        expected_state: op_il.state.clone(),
        expected_state_version: op_il.state_version,
        run_id: run_il,
        started_at_utc: pe_now(),
        occurred_at_utc: pe_now(),
        bounded_detail: "phase-e5 proof-f invalid-lineage fixture".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_il)
        .await
        .expect("ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_) => {}
        other => panic!("expected Applied, got {other:?}"),
    }
    // Corrupt the lineage: the current run_id column no longer matches any
    // to_state='running' transition's run_id (CurrentRunMismatch).
    sqlx::query("update sys_autonomous_daily_operations set run_id = $1 where operation_id = $2")
        .bind(Uuid::new_v4())
        .bind(op_il.operation_id)
        .execute(&pool)
        .await
        .expect("corrupt run_id ok");

    let mut st_il_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_il_inner.set_adapter_id_for_test(&adapter_id_il);
    let router_il = build_router(Arc::new(st_il_inner));
    let (status_il, body_il) = get_json(
        router_il,
        &format!(
            "/api/v1/autonomous/daily-operation?market_date={}",
            plan_il.market_date
        ),
    )
    .await;
    assert_eq!(status_il, StatusCode::OK);
    assert_eq!(
        body_il["truth_state"].as_str().unwrap(),
        "active",
        "a lineage contradiction is a read-model evidence gap, not a query failure"
    );
    let row_il = &body_il["operation"];
    assert!(row_il["strategy_evaluation_count"].is_null());
    assert!(row_il["order_activity_count"].is_null());
    assert!(row_il["fill_count"].is_null());
    assert_eq!(row_il["evidence_state"].as_str().unwrap(), "unavailable");
    let blockers_il = row_il["evidence_blockers"].as_array().unwrap();
    assert!(blockers_il
        .iter()
        .any(|v| v.as_str() == Some("unknown_run_lineage_unavailable")));

    // exact malformed market_date -> 400 invalid_request with bounded fixed
    // message
    let router_malformed = build_router(pe_no_db_state());
    let (status_malformed, body_malformed) = get_json(
        router_malformed,
        "/api/v1/autonomous/daily-operation?market_date=not-a-date",
    )
    .await;
    assert_eq!(status_malformed, StatusCode::BAD_REQUEST);
    assert_eq!(
        body_malformed["truth_state"].as_str().unwrap(),
        "invalid_request"
    );
    let message = body_malformed["message"].as_str().unwrap();
    assert_eq!(message, "market_date must use exact YYYY-MM-DD format");
    assert!(message.len() < 200);
}
