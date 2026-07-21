//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
//! INTEGRATION-AND-NOTIFICATION: integrated proof that the durable daily
//! coordinator (`state::autonomous_daily_coordinator::tick_autonomous_daily_coordinator`)
//! correctly invokes the accepted E2B finalizer
//! (`state::autonomous_daily_outcome::classify_and_finalize_autonomous_daily_operation`)
//! at exactly the eligible moment, projects every E2B result into a bounded
//! typed coordinator outcome, and that `session_controller.rs`'s
//! `log_coordinator_outcome` sends exactly the notifications the E1 §12
//! contract requires -- never more, never fewer.
//!
//! DB-backed; every test requires `MQK_DATABASE_URL` and is marked
//! `#[ignore]`, matching every other DB-backed scenario file in this crate.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_outcome_coordinator_integration_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! Every test drives the *real* production coordinator/finalizer seams
//! (`tick_autonomous_daily_coordinator` -> `dispatch_by_state`/`handle_stopping`
//! -> `handle_outcome_finalization` -> `classify_and_finalize_autonomous_daily_operation`)
//! against the real, isolated test database. Discord notifications are
//! captured by an in-process loopback HTTP sink (`ci_webhook_sink`) -- no
//! real Discord/network call is ever made. No real provider or broker call
//! is made anywhere in this file; no paper or live order is submitted.
//!
//! **Identity plumbing.** `tick_autonomous_daily_coordinator` derives its own
//! canonical `(market_date, deployment_mode, adapter_id)` operation identity
//! from `AppState`/env on every tick -- a test cannot simply hand it an
//! arbitrary pre-built operation row. Every fixture below therefore:
//! (1) resolves the *real* production session plan (`ci_plan`) for a fixed,
//! known-applicable trading-day instant (`ci_now`, the same anchor
//! `scenario_autonomous_daily_phase_d_integration_01.rs` uses); (2) sets
//! `MQK_DAEMON_ADAPTER_ID`/`MQK_DAEMON_DEPLOYMENT_MODE` to a unique per-test
//! adapter id *before* constructing `AppState` (`runtime_selection` is
//! resolved once, at construction); (3) builds the operation row's
//! `market_date`/`deployment_mode`/`adapter_id` to match exactly what that
//! same tick will independently resolve, so `create_or_recover` durably
//! *recovers* the fixture's row rather than creating a fresh one.
//! `ci_clean_fixture` additionally derives `assignment_identity`/
//! `runtime_binding_identity` from the same `MultiSymbolRuntimeConfig`/
//! `EffectiveRuntimeBinding` a real coordinator tick will independently
//! reconstruct from `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_IDS`/
//! `MQK_STRATEGY_MD_TIMEFRAME` (`MultiSymbolConfigSource::EnvSingleSymbolFallback`),
//! and writes a real coverage-bound event, so the E2B classifier can reach a
//! genuine clean terminal classification. `ci_stopping_operation` uses the
//! *same* matching identities but never writes a coverage-bound event, so it
//! deterministically fails closed to `unknown_incomplete_bar_coverage` --
//! useful for proving coordinator *routing* without needing full evidence
//! construction for every scenario.
//!
//! Known scope limitation (documented, not fabricated): E1 contract §3.2
//! condition 4 (no matching locally-owned runtime) is already exhaustively
//! proven at the pure-function level by E2B's own accepted
//! `eligibility_*`/`check_finalization_eligibility` tests (regression-run by
//! this bundle). Proving the *true* (blocking) case at this file's
//! integration level would require driving a real canonical start through a
//! full mock-Alpaca-backed production fixture (the `phase_d_full_day_lifecycle`
//! pattern) merely to obtain a live `AppState::execution_loop` handle -- no
//! lighter `pub` test seam exists to inject one directly from this external
//! test crate, and adding one would widen production surface outside this
//! patch's stated scope. `ci_03_matching_local_runtime_active_is_always_false_with_no_local_runtime`
//! below proves the *false* (non-blocking) side of the same fact end-to-end
//! through a real coordinator tick against a bound `run_id`.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use mqk_daemon::daily_data_readiness;
use mqk_daemon::notify::DiscordNotifier;
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
    OperatorAuthMode,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared test-env plumbing
// ---------------------------------------------------------------------------

const STRATEGY_ID: &str = "intraday_scalper";
const SYMBOL: &str = "ZZE3CI";
const TIMEFRAME: &str = "5m";

/// Same fixed, known-applicable-trading-day anchor
/// `scenario_autonomous_daily_phase_d_integration_01.rs`'s `pd_now` uses.
const CI_READINESS_FIXED_TS: i64 = 1_713_188_100 + 10 * 300;

fn ci_now() -> DateTime<Utc> {
    Utc.timestamp_opt(CI_READINESS_FIXED_TS, 0)
        .single()
        .expect("valid fixed CI readiness timestamp")
}

fn ci_clear_session_override_env() {
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");
}

fn ci_plan() -> AutonomousDailySessionPlan {
    ci_clear_session_override_env();
    let timing = AutonomousDailyPlanTiming::production_default();
    match resolve_autonomous_daily_session_plan_from_env(ci_now(), &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => panic!("CI: expected an applicable session plan, got {other:?}"),
    }
}

fn ci_parse_market_date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("valid market_date")
}

fn unique_adapter_id(label: &str) -> String {
    format!("zze3ci-{}-{}", label, &Uuid::new_v4().to_string()[..8])
}

/// Set `MQK_DAEMON_ADAPTER_ID`/`MQK_DAEMON_DEPLOYMENT_MODE` (read once at
/// `AppState` construction) and the strategy env vars a real coordinator
/// tick's own `build_multi_symbol_runtime_config_from_env`/
/// `resolve_autonomous_runtime_context` will resolve. Must be called before
/// constructing `AppState` for this test.
fn ci_set_env(adapter_id: &str) {
    std::env::set_var("MQK_DAEMON_ADAPTER_ID", adapter_id);
    std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
    std::env::set_var("MQK_STRATEGY_IDS", STRATEGY_ID);
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", TIMEFRAME);
    std::env::set_var(daily_data_readiness::GRACE_SECS_ENV, "0");
    ci_clear_session_override_env();
}

/// Clears the strategy identity env vars only (leaves adapter/deployment
/// mode untouched) -- models a current policy-resolution failure (E3.4)
/// without disturbing which operation identity the coordinator looks up.
fn ci_clear_strategy_env() {
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
    sqlx::query("delete from oms_inbox where run_id in (select run_id from runs where engine_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean inbox");
    sqlx::query("delete from oms_outbox where run_id in (select run_id from runs where engine_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean outbox");
    sqlx::query("delete from strategy_signal_evaluations where run_id in (select run_id from runs where engine_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean evaluations");
    sqlx::query("delete from runs where engine_id like 'zze3ci%'")
        .execute(&pool)
        .await
        .expect("clean runs");
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean claims");
    sqlx::query("delete from sys_autonomous_session_events where id like 'autonomous_daily_coverage_bound:%' and id in (select 'autonomous_daily_coverage_bound:' || operation_id::text from sys_autonomous_daily_operations where adapter_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean coverage events");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze3ci%')")
        .execute(&pool).await.expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zze3ci%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    Some(pool)
}

/// In-process loopback HTTP sink capturing every Discord webhook POST body --
/// never a real network call. Mirrors `scenario_autonomous_completed_bar_task_01.rs`'s
/// `start_alert_counting_webhook` pattern.
async fn ci_webhook_sink() -> (
    String,
    std::sync::Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let received: std::sync::Arc<tokio::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&received);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = std::sync::Arc::clone(&sink);
            async move {
                sink.lock().await.push(body);
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://127.0.0.1:{}/", addr.port()), received)
}

/// Build a real `Arc<AppState>` wired to the test DB pool, with
/// `discord_notifier` overridden to the loopback sink before `Arc::new` --
/// this deliberately bypasses `DiscordNotifier`'s env-based constructor, so
/// no real webhook URL is ever consulted. Caller must have already called
/// [`ci_set_env`] this tick.
async fn ci_daemon_state(pool: sqlx::PgPool, webhook_url: &str) -> std::sync::Arc<AppState> {
    let mut inner =
        AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    inner.discord_notifier = DiscordNotifier::from_url(webhook_url);
    std::sync::Arc::new(inner)
}

/// Same as [`ci_daemon_state`] but with notification delivery a true no-op
/// (`DiscordNotifier::noop()`) -- used by the one test proving notifier
/// absence never affects durable terminal truth.
async fn ci_daemon_state_noop_notifier(pool: sqlx::PgPool) -> std::sync::Arc<AppState> {
    let mut inner =
        AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    inner.discord_notifier = DiscordNotifier::noop();
    std::sync::Arc::new(inner)
}

fn ci_create_args(
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    assignment_identity: String,
    runtime_binding_identity: String,
    initial_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::CreateAutonomousDailyOperationArgs {
    mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: Uuid::new_v4(),
        market_date: ci_parse_market_date(&plan.market_date),
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
        previous_trading_date: ci_parse_market_date(&plan.previous_trading_date),
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "e3 coordinator-integration test fixture".to_string(),
        stop_attempt_count: 0,
    }
}

async fn ci_transition(
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
        bounded_detail: "e3 coordinator-integration test transition".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

/// A durably stopped operation with a *matching* assignment/runtime-binding
/// identity (so it is never blocked on identity alone) but **no** coverage-
/// bound event -- deterministically fails closed to
/// `unknown_incomplete_bar_coverage` on every finalization attempt. Used by
/// tests that only need to prove coordinator *routing*, not a clean terminal
/// classification.
async fn ci_stopping_operation(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let (assignment_identity, runtime_binding_identity) = ci_matching_identities();
    let args = ci_create_args(
        plan,
        adapter_id,
        assignment_identity,
        runtime_binding_identity,
        mqk_db::STATE_AWAITING_OPEN,
        now_utc,
    );
    let op = match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };
    let op = ci_transition(pool, &op, mqk_db::STATE_STOPPING, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, op.operation_id, now_utc)
        .await
        .expect("record stopped ok");
    mqk_db::fetch_autonomous_daily_operation_by_id(pool, op.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists")
}

fn ci_matching_config() -> mqk_daemon::state::MultiSymbolRuntimeConfig {
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

fn ci_matching_binding() -> mqk_runtime::native_strategy::EffectiveRuntimeBinding {
    mqk_runtime::native_strategy::EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(STRATEGY_ID.to_string()),
        effective_runtime_target_symbol: Some(SYMBOL.to_string()),
        effective_runtime_timeframe_secs: Some(300),
    }
}

fn ci_matching_identities() -> (String, String) {
    let config = ci_matching_config();
    let binding = ci_matching_binding();
    (
        mqk_daemon::state::derive_assignment_identity(&config),
        mqk_daemon::state::derive_runtime_binding_identity(&binding),
    )
}

/// Build a durably-anchored `stopping` operation whose persisted
/// `assignment_identity`/`runtime_binding_identity` exactly match what a real
/// coordinator tick will independently (re-)resolve from [`ci_set_env`]'s env
/// vars, with a real coverage-bound event and real completed dispatch claims
/// plus flat strategy evaluations for every expected bar. Adapted from
/// `scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs`'s
/// `integrated_fixture`.
async fn ci_clean_fixture(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
    with_fill: bool,
) -> (mqk_db::AutonomousDailyOperationRecord, Uuid) {
    let config = ci_matching_config();
    let binding = ci_matching_binding();
    let mut registry = mqk_strategy::PluginRegistry::new();
    let _ = mqk_strategy::engines::register_builtin_strategies(&mut registry, String::new());
    let (assignment_identity, runtime_binding_identity) = ci_matching_identities();

    // A fixture-local, hand-picked session window (self-consistent; only
    // `market_date`/`deployment_mode`/`adapter_id` -- not these session
    // fields -- are the natural-key lookup `create_or_recover` uses).
    // Anchored 30 minutes after this fixture's own exchange open so
    // `intraday_scalper`'s real `required_history_bars` (5) has already
    // closed enough current-session bars to avoid previous-session
    // spillover, exactly like the accepted E2B `integrated_fixture`.
    let exchange_open = plan.exchange_session_open_utc;
    let exchange_close = plan.exchange_session_close_utc;
    let effective_open = exchange_open + chrono::Duration::minutes(30);
    let effective_close = effective_open + chrono::Duration::minutes(11);

    let mut args = ci_create_args(
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

    let started = ci_transition(pool, &operation, mqk_db::STATE_START_RETRYING, now_utc).await;
    let to_running = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: started.operation_id,
        expected_state: started.state.clone(),
        expected_state_version: started.state_version,
        run_id,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "e3 coordinator-integration fixture: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running)
        .await
        .expect("ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    for &bar_end_ts in &derive_expected_bar_set(&detail) {
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

    if with_fill {
        mqk_db::inbox_insert_deduped_with_identity(
            pool,
            run_id,
            "zze3ci-broker-msg-1",
            Some("zze3ci-fill-1"),
            "zze3ci-internal-order-1",
            "zze3ci-broker-order-1",
            "fill",
            &serde_json::json!({"event_kind": "fill"}),
            now_utc.timestamp_millis(),
            now_utc,
        )
        .await
        .expect("insert fill ok");
    }

    let stopping = ci_transition(pool, &running, mqk_db::STATE_STOPPING, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, stopping.operation_id, now_utc)
        .await
        .expect("stop ok");
    let final_op = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");

    (final_op, run_id)
}

/// Drives one real production tick -- `run_durable_session_controller_tick`
/// is the sole production seam that performs the durable coordinator tick
/// *and* the bounded logging/notification of its result
/// (`session_controller.rs`'s own doc comment: "never bypasses the durable
/// coordinator's own gates"). Calling `tick_autonomous_daily_coordinator`
/// directly (as `ci_tick_typed` below does, for one dedicated test) proves
/// the typed projection in isolation but never sends a notification --
/// notification-relevant tests in this file must use this function, not
/// `tick_autonomous_daily_coordinator` directly, or they will never observe
/// a real Discord POST.
async fn ci_tick(st: &std::sync::Arc<AppState>, now_utc: DateTime<Utc>) {
    run_durable_session_controller_tick(st, now_utc).await;
}

/// Direct call to the coordinator entry point, returning its typed outcome.
/// Used by exactly one dedicated test proving E3.5's bounded typed
/// projection; every other test uses [`ci_tick`] (the real notifying
/// production seam) plus durable DB-row assertions.
async fn ci_tick_typed(
    st: &std::sync::Arc<AppState>,
    now_utc: DateTime<Utc>,
) -> AutonomousDailyCoordinatorTickOutcome {
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput { state: st, now_utc })
        .await
        .expect("coordinator tick must not error")
}

async fn ci_events_count(pool: &sqlx::PgPool, operation_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("count ok")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Req 1: a durably stopped, clean no-trade operation finalizes through a
/// real coordinator tick, and sends exactly one outcome notification.
#[tokio::test]
#[ignore]
async fn ci_01_clean_no_trade_operation_finalizes_through_real_coordinator_tick() {
    let Some(pool) = maybe_db("ci_01").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci01");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    ci_tick(&st, ci_now()).await;

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

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let captured = alerts.lock().await;
    assert_eq!(
        captured.len(),
        1,
        "expected exactly one outcome notification, got {captured:?}"
    );
    assert_eq!(
        captured[0]["event"], "autonomous.daily_operation.outcome",
        "notification event must be the stable, documented event string"
    );

    // Req 23: no provider/broker/order surface touched by E3 itself.
    let outbox_count: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count ok");
    assert_eq!(outbox_count, 0, "E3 must never create order/outbox rows");
}

/// Req 2: a durably stopped, fill-evidence operation finalizes as
/// `activity_fill_confirmed`.
#[tokio::test]
#[ignore]
async fn ci_02_fill_evidence_operation_finalizes_as_activity_fill_confirmed() {
    let Some(pool) = maybe_db("ci_02").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci02");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), true).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
    assert_eq!(
        record.outcome.as_deref(),
        Some(mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED)
    );

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(alerts.lock().await.len(), 1);
}

/// Req 3 (false side): `operation.run_id` is present, but no local runtime is
/// bound in this process's `AppState` -- the coordinator must still finalize
/// (never blocked merely because `run_id` is `Some`). Proves
/// `matching_local_runtime_active` is computed from the real tuple match, not
/// from `run_id.is_some()` alone.
#[tokio::test]
#[ignore]
async fn ci_03_matching_local_runtime_active_is_always_false_with_no_local_runtime() {
    let Some(pool) = maybe_db("ci_03").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci03");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, _alerts) = ci_webhook_sink().await;
    let (operation, run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    assert_eq!(operation.run_id, Some(run_id), "fixture bound a run_id");
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;
    assert_eq!(
        st.locally_owned_run_id().await,
        None,
        "a freshly-constructed AppState never owns a local execution loop"
    );

    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        record.state,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        "a bound run_id with no local runtime must not block finalization: {record:?}"
    );
}

/// Req 4: `stopped_at_utc = NULL` preserves existing stop/retry handling and
/// never invokes E2B (the tick that first sets it returns `RuntimeStopped`,
/// never a finalization outcome).
#[tokio::test]
#[ignore]
async fn ci_04_stopped_at_utc_null_preserves_stop_retry_and_never_invokes_e2b() {
    let Some(pool) = maybe_db("ci_04").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci04");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (assignment_identity, runtime_binding_identity) = ci_matching_identities();
    let args = ci_create_args(
        &plan,
        &adapter_id,
        assignment_identity,
        runtime_binding_identity,
        mqk_db::STATE_AWAITING_OPEN,
        ci_now(),
    );
    let op = match mqk_db::create_or_recover_autonomous_daily_operation(&pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };
    let op = ci_transition(&pool, &op, mqk_db::STATE_STOPPING, ci_now()).await;
    assert!(op.stopped_at_utc.is_none());
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    // No local runtime is bound, so `retry_stop`'s `(None, None)` arm
    // applies -- the real production stop path records `stopped_at_utc`
    // itself. The tick that first records it must return `RuntimeStopped`,
    // never a finalization outcome (E3.1: finalization on that same tick is
    // never attempted).
    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        record.state,
        mqk_db::STATE_STOPPING,
        "the tick that first records stopped_at_utc must not itself finalize: {record:?}"
    );
    assert!(record.stopped_at_utc.is_some());
    assert!(
        record.outcome.is_none(),
        "E2B must not have been invoked yet this tick"
    );
    assert!(record.finalized_at_utc.is_none());

    // The pre-existing (pre-E3) `RuntimeStopped` outcome already sends its
    // own `autonomous.run.stop` notification (session_controller.rs,
    // unrelated to and unchanged by E3) -- this proves *that* fires, and
    // that it is not the new E1 §12 finalization notification (which must
    // never fire before stopped_at_utc is durable).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let captured = alerts.lock().await;
    assert_eq!(
        captured.len(),
        1,
        "only the pre-existing stop notification: {captured:?}"
    );
    assert_eq!(captured[0]["event"], "autonomous.run.stop");
}

/// Req 5/6: the stop-completion tick does not itself finalize; a *subsequent*
/// tick performs finalization exactly once (one new lifecycle event, one
/// notification total).
#[tokio::test]
#[ignore]
async fn ci_05_06_stop_completion_tick_defers_and_next_tick_finalizes_exactly_once() {
    let Some(pool) = maybe_db("ci_05_06").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci0506");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    // The fixture already durably set stopped_at_utc via
    // record_autonomous_runtime_stopped (mirrors the real stop-completion
    // tick's own write) -- the first tick against it is the "subsequent
    // tick" under test here. Req 5's "does not duplicate the stop call" half
    // is covered structurally by `handle_stopping`'s top-of-function gate
    // (never reaches `retry_stop` once `stopped_at_utc` is set) and by ci_04
    // above.
    let events_before = ci_events_count(&pool, operation.operation_id).await;
    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
    let events_after_1 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_1,
        events_before + 1,
        "exactly one new lifecycle event for the finalization transition"
    );

    ci_tick(&st, ci_now() + chrono::Duration::seconds(1)).await;
    let record_2 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        (record_2.state, record_2.state_version),
        (record.state.clone(), record.state_version),
        "a second tick must observe the already-terminal row read-only"
    );
    let events_after_2 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_2, events_after_1,
        "zero new lifecycle events on replay"
    );

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        alerts.lock().await.len(),
        1,
        "exactly one notification total across both ticks"
    );
}

/// Req 7/8: a repeated terminal tick writes zero lifecycle events and emits
/// zero outcome notifications.
#[tokio::test]
#[ignore]
async fn ci_07_08_repeated_terminal_tick_writes_and_notifies_nothing() {
    let Some(pool) = maybe_db("ci_07_08").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci0708");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;
    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    alerts.lock().await.clear();
    let events_before = ci_events_count(&pool, operation.operation_id).await;

    for i in 0..3i64 {
        ci_tick(&st, ci_now() + chrono::Duration::seconds(i + 1)).await;
        let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
        assert_eq!(
            record.state,
            mqk_db::STATE_COMPLETED_NO_TRADE,
            "repeated tick {i} must be read-only"
        );
    }

    let events_after = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_before, events_after,
        "zero new lifecycle events across repeats"
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        alerts.lock().await.is_empty(),
        "zero notifications across repeated terminal ticks"
    );
}

/// Req 9/10: restart after durable stop (a fresh `AppState`, next tick)
/// finalizes exactly once; restart after terminal commit emits zero
/// duplicate notification.
#[tokio::test]
#[ignore]
async fn ci_09_10_restart_before_and_after_finalization_is_exactly_once() {
    let Some(pool) = maybe_db("ci_09_10").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci0910");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;

    // Restart before finalization: a brand-new AppState/process, never
    // having observed the prior fixture-construction steps in memory.
    let st_restarted = ci_daemon_state(pool.clone(), &webhook_url).await;
    ci_tick(&st_restarted, ci_now()).await;
    let record_mid = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record_mid.state, mqk_db::STATE_COMPLETED_NO_TRADE);

    // Restart after terminal commit: a second brand-new AppState.
    let st_restarted_again = ci_daemon_state(pool.clone(), &webhook_url).await;
    ci_tick(&st_restarted_again, ci_now() + chrono::Duration::seconds(1)).await;

    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        alerts.lock().await.len(),
        1,
        "exactly one notification total across both restarts"
    );
}

/// Req 11/12: a newly applied evidence blocker (missing coverage anchor)
/// emits one warning; an exact replay emits zero additional warnings.
#[tokio::test]
#[ignore]
async fn ci_11_12_evidence_degraded_warning_dedup() {
    let Some(pool) = maybe_db("ci_11_12").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci1112");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    // Matching identity, but no coverage-bound event -> deterministic
    // unknown_incomplete_bar_coverage on every attempt.
    let operation = ci_stopping_operation(&pool, &plan, &adapter_id, ci_now()).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    let events_before = ci_events_count(&pool, operation.operation_id).await;
    ci_tick(&st, ci_now()).await;
    let record_1 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record_1.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert_eq!(
        record_1.state_reason_code.as_deref(),
        Some("unknown_incomplete_bar_coverage")
    );
    let events_after_1 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_1,
        events_before + 1,
        "first blocker application must durably advance truth (newly_applied)"
    );
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert_eq!(
        alerts.lock().await.len(),
        1,
        "one warning for the newly applied blocker"
    );

    ci_tick(&st, ci_now() + chrono::Duration::seconds(1)).await;
    let record_2 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record_2.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    let events_after_2 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_2, events_after_1,
        "exact-reason replay must not durably advance truth (not newly_applied)"
    );
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert_eq!(
        alerts.lock().await.len(),
        1,
        "zero additional warnings on exact replay"
    );
}

/// Req 14/15: evidence repair takes `evidence_degraded -> stopping`, not
/// direct completion; the following tick performs terminal completion.
#[tokio::test]
#[ignore]
async fn ci_14_15_evidence_repair_recovers_then_next_tick_finalizes() {
    let Some(pool) = maybe_db("ci_14_15").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci1415");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let (operation, run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;

    // Corrupt one expected bar's claim to `failed` so the classifier fails
    // closed to unknown_unresolved_dispatch_claim instead of finalizing.
    let expected: Vec<i64> = sqlx::query_scalar(
        "select bar_end_ts from sys_autonomous_daily_bar_dispatches where operation_id = $1 \
         order by bar_end_ts limit 1",
    )
    .bind(operation.operation_id)
    .fetch_all(&pool)
    .await
    .expect("query ok");
    let bar_end_ts = expected[0];
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'failed', evaluation_id = null, \
         completed_at_utc = null \
         where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation.operation_id)
    .bind(bar_end_ts)
    .execute(&pool)
    .await
    .expect("corrupt claim ok");

    let st = ci_daemon_state(pool.clone(), &webhook_url).await;
    ci_tick(&st, ci_now()).await;
    let degraded_row =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        degraded_row.state,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        "unresolved claim must degrade, not finalize"
    );

    // Repair: mark the claim completed again with a real confirmed
    // evaluation_id.
    let evaluation_id: Uuid = sqlx::query_scalar(
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
    .bind(bar_end_ts)
    .bind(evaluation_id)
    .execute(&pool)
    .await
    .expect("repair claim ok");

    ci_tick(&st, ci_now() + chrono::Duration::seconds(1)).await;
    let recovered_row =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        recovered_row.state,
        mqk_db::STATE_STOPPING,
        "repaired evidence must recover to stopping, not finalize directly"
    );

    ci_tick(&st, ci_now() + chrono::Duration::seconds(2)).await;
    let finalized_row =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .expect("fetch ok")
            .expect("row exists");
    assert_eq!(
        finalized_row.state,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        "the following tick must perform terminal completion"
    );

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let captured = alerts.lock().await;
    assert_eq!(
        captured.len(),
        2,
        "one evidence-degraded warning plus one terminal outcome notification, got {captured:?}"
    );
}

/// Req 16/17: a policy/config resolution failure on a stopped row persists
/// `unknown_assignment_identity_unavailable`; a replay of the same blocker
/// writes and notifies zero additional times.
#[tokio::test]
#[ignore]
async fn ci_16_17_policy_resolution_failure_persists_and_replay_is_silent() {
    let Some(pool) = maybe_db("ci_16_17").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci1617");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let operation = ci_stopping_operation(&pool, &plan, &adapter_id, ci_now()).await;
    // Deliberately clear strategy env vars only: config resolution itself
    // fails this tick, before evidence gathering can even begin. Adapter id
    // stays intact so the coordinator still finds our operation.
    ci_clear_strategy_env();
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    let events_before = ci_events_count(&pool, operation.operation_id).await;
    ci_tick(&st, ci_now()).await;
    let record_1 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record_1.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert_eq!(
        record_1.state_reason_code.as_deref(),
        Some("unknown_assignment_identity_unavailable")
    );
    let events_after_1 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(events_after_1, events_before + 1, "newly applied blocker");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(alerts.lock().await.len(), 1);

    ci_tick(&st, ci_now() + chrono::Duration::seconds(1)).await;
    let events_after_2 = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_after_2, events_after_1,
        "replay of the same resolution failure must not durably advance truth"
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        alerts.lock().await.len(),
        1,
        "zero additional writes/notifications on replay"
    );
}

/// Req 21: a generic administrative `completed` row is read-only -- no
/// classifier re-run, no notification.
#[tokio::test]
#[ignore]
async fn ci_21_generic_completed_remains_read_only() {
    let Some(pool) = maybe_db("ci_21").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci21");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let operation = ci_stopping_operation(&pool, &plan, &adapter_id, ci_now()).await;
    let operation = ci_transition(&pool, &operation, mqk_db::STATE_COMPLETED, ci_now()).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;
    let events_before = ci_events_count(&pool, operation.operation_id).await;

    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED);
    assert!(record.outcome.is_none());
    assert_eq!(
        record.state_version, operation.state_version,
        "generic completed must never be rewritten"
    );
    let events_after = ci_events_count(&pool, operation.operation_id).await;
    assert_eq!(
        events_before, events_after,
        "read-only: zero new lifecycle events"
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(alerts.lock().await.is_empty());
}

/// Req 22: notifier delivery "failure" (a true no-op notifier, never a real
/// network call) never alters durable terminal DB truth.
#[tokio::test]
#[ignore]
async fn ci_22_notification_noop_does_not_alter_terminal_db_truth() {
    let Some(pool) = maybe_db("ci_22").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci22");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    let st = ci_daemon_state_noop_notifier(pool.clone()).await;

    ci_tick(&st, ci_now()).await;
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
    assert!(record.finalized_at_utc.is_some());
}

/// E3.5: `tick_autonomous_daily_coordinator`'s own typed return value, called
/// directly (not through the notifying `run_durable_session_controller_tick`
/// wrapper every other test in this file uses), projects a clean E2B
/// finalization into exactly `OutcomeFinalized { outcome_reason_code, .. }` --
/// proving the bounded typed projection in isolation from notification
/// delivery.
#[tokio::test]
#[ignore]
async fn ci_typed_outcome_projection_is_exhaustive_for_a_clean_finalization() {
    let Some(pool) = maybe_db("ci_typed").await else {
        return;
    };
    let adapter_id = unique_adapter_id("citype");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, _alerts) = ci_webhook_sink().await;
    let (_operation, _run_id) = ci_clean_fixture(&pool, &plan, &adapter_id, ci_now(), false).await;
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    let outcome = ci_tick_typed(&st, ci_now()).await;
    match outcome {
        AutonomousDailyCoordinatorTickOutcome::OutcomeFinalized {
            outcome_reason_code,
            run_id,
            ..
        } => {
            assert_eq!(
                outcome_reason_code,
                mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
            );
            assert!(run_id.is_some());
        }
        other => panic!("expected OutcomeFinalized(no_trade), got {other:?}"),
    }
}

/// Req 24 (source-level, not runtime): the completed-bar production adapter
/// never calls the E2B finalizer. Enforced primarily by the new E3 guard
/// (`validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1`);
/// this test adds a cheap, redundant, non-DB, non-ignored source check so a
/// regression is caught even without running guards.
#[test]
fn ci_24_completed_bar_adapter_source_never_references_the_finalizer() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        "src/state/autonomous_completed_bar_task.rs",
        "src/state/autonomous_completed_bar_driver.rs",
    ] {
        let path = manifest_dir.join(rel);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("expected to read {}: {e}", path.display()));
        assert!(
            !content.contains("classify_and_finalize_autonomous_daily_operation"),
            "{rel} must never call the E2B finalizer"
        );
    }
}

/// Req 25: finalization remains possible through the relevant-existing-
/// operation fallback path after a resolution-failure tick -- a stopped,
/// finalization-eligible operation discovered via
/// `resolve_or_degrade_on_resolution_failure`'s `fetch_relevant_open_autonomous_daily_operation`
/// lookup is never abandoned; it is routed into the same
/// `handle_outcome_finalization` seam every ordinary tick uses.
#[tokio::test]
#[ignore]
async fn ci_25_finalization_reachable_through_resolution_failure_fallback_lookup() {
    let Some(pool) = maybe_db("ci_25").await else {
        return;
    };
    let adapter_id = unique_adapter_id("ci25");
    ci_set_env(&adapter_id);
    let plan = ci_plan();
    let (webhook_url, alerts) = ci_webhook_sink().await;
    let operation = ci_stopping_operation(&pool, &plan, &adapter_id, ci_now()).await;

    // Deliberately corrupt the current tick's own top-level config
    // resolution (no strategy env vars) so `tick_autonomous_daily_coordinator`
    // takes the `resolve_or_degrade_on_resolution_failure` fallback path
    // *before* ever reaching `create_or_recover`/`dispatch_by_state` for a
    // fresh identity this tick -- the only way it can still observe our
    // already-durable stopped operation is via the relevant-open-operation
    // lookup inside that fallback.
    ci_clear_strategy_env();
    let st = ci_daemon_state(pool.clone(), &webhook_url).await;

    ci_tick(&st, ci_now()).await;
    // The fallback path finds our stopped operation and routes it into
    // handle_outcome_finalization -- which independently re-resolves policy
    // (and fails identically, since the same env vars are still corrupted),
    // failing closed to a durable evidence blocker rather than being
    // silently dropped as `IdentityBlocked`/`CalendarBlocked` with no trace
    // of our operation at all.
    let record = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        record.state,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        "the fallback path must route into finalization rather than abandon the stopped \
         operation"
    );
    assert_eq!(
        record.state_reason_code.as_deref(),
        Some("unknown_assignment_identity_unavailable")
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(alerts.lock().await.len(), 1);
}
