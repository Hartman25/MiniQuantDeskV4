//! CONTROL-DEGRADED-LIVE-OWNER-RECOVERY-01: proof tests for the
//! `dispatch_by_state` repair to `mqk_db::STATE_CONTROLLER_DEGRADED` that
//! stops it from falsely asserting ownership loss for a runtime that never
//! stopped.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_control_degraded_live_owner_recovery_01 \
//!   -- --test-threads=1 --nocapture --ignored
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! Every `runs` row fixture is driven through the real lifecycle primitives
//! (`mqk_db::insert_run` / `arm_run` / `begin_run`) -- never a raw `UPDATE`.
//! Every operation fixture reaches `running` -> `controller_degraded` only
//! through the real production CAS (`mqk_db::transition_autonomous_daily_
//! operation`). Local runtime ownership is established via the real
//! `AppState::inject_running_loop_for_test` seam -- never a raw field
//! write -- so `AppState::locally_owned_run_id()` reports genuine
//! production truth.
//!
//! Incident this closes: before this repair, `dispatch_by_state`'s
//! `STATE_CONTROLLER_DEGRADED` arm called `reconcile_durable_run_without_
//! local_owner` unconditionally -- that helper has no `AppState`/local-
//! owner input at all, so it asserted `durable_active_run_without_local_
//! owner` even when a live local runtime still genuinely owned the exact
//! run the operation expected, and there was no path back to `running`.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_daemon::daily_data_readiness::{
    expected_intraday_end_ts_window, REASON_MARKET_DATA_MISSING,
};
use mqk_daemon::state::autonomous_completed_bar_driver::AutonomousCompletedBarDriverMode;
use mqk_daemon::state::autonomous_completed_bar_task::select_driver_mode_for_state;
use mqk_daemon::state::autonomous_daily_coordinator::{
    dispatch_by_state, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::autonomous_runtime_context::resolve_autonomous_runtime_context;
use mqk_daemon::state::market_calendar::{resolve_market_session_schedule, NyseWeekdaysProvider};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, StrategyFleetEntry,
    SymbolStrategyAssignment,
};
use mqk_db::{AutonomousDailyTransitionOutcome, TransitionAutonomousDailyOperationArgs};
use uuid::Uuid;

const SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const IDS_ENV: &str = "MQK_STRATEGY_IDS";
const TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";
const SYMBOL: &str = "AAPL";
const STRATEGY_ID: &str = "intraday_scalper";
const TIMEFRAME: &str = "5m";
const PROVIDER_ID: &str = "alpaca";

// ---------------------------------------------------------------------------
// Helpers (mirrors scenario_autonomous_data_blocker_auto_recovery_01.rs and
// scenario_autonomous_daily_controller_degraded_recovery_01.rs)
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn reset_env() {
    std::env::remove_var(SYMBOL_ENV);
    std::env::remove_var(IDS_ENV);
    std::env::remove_var(TIMEFRAME_ENV);
    std::env::remove_var("MQK_DATA_READINESS_GRACE_SECS");
    std::env::remove_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS");
    std::env::remove_var("MQK_INSTRUMENT_REGISTRY_PATH");
    std::env::remove_var("MQK_PROVIDER_REGISTRY_PATH");
}

fn set_active_assignment_env() {
    std::env::set_var(SYMBOL_ENV, SYMBOL);
    std::env::set_var(IDS_ENV, STRATEGY_ID);
    std::env::set_var(TIMEFRAME_ENV, TIMEFRAME);
    std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
    std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    std::env::set_var(
        "MQK_INSTRUMENT_REGISTRY_PATH",
        repo_root()
            .join("config/instruments/equities.json")
            .to_string_lossy()
            .to_string(),
    );
    std::env::set_var(
        "MQK_PROVIDER_REGISTRY_PATH",
        repo_root()
            .join("config/providers/providers.json")
            .to_string_lossy()
            .to_string(),
    );
}

fn paper_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

async fn active_fleet_st(st: &Arc<AppState>) {
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: STRATEGY_ID.to_string(),
    }]))
    .await;
}

/// The next date at 14:30 UTC strictly after the real wall clock for which
/// the canonical session-plan resolver
/// (`resolve_autonomous_daily_session_plan_from_env`) itself reports
/// `Applicable` -- dynamic rather than a fixed fictional date so the
/// readiness evaluator's own session/calendar math always sees a genuinely
/// open session. A plain weekday check is not sufficient: a weekday can
/// still be an exchange holiday (e.g. Independence Day, Thanksgiving,
/// Christmas), which the canonical resolver would reject as
/// `NotApplicable`. Bounded to a short forward search using the exact same
/// authority production dispatch relies on, never an ad hoc weekday-only
/// assumption.
fn dynamic_session_now() -> DateTime<Utc> {
    let timing = AutonomousDailyPlanTiming::production_default();
    let mut candidate_date = Utc::now().date_naive() + ChronoDuration::days(1);
    for _ in 0..30 {
        let candidate_now = Utc.from_utc_datetime(&candidate_date.and_hms_opt(14, 30, 0).unwrap());
        if matches!(
            resolve_autonomous_daily_session_plan_from_env(candidate_now, &timing),
            AutonomousDailySessionPlanResolution::Applicable(_)
        ) {
            return candidate_now;
        }
        candidate_date += ChronoDuration::days(1);
    }
    panic!(
        "dynamic_session_now: no Applicable trading session found within 30 days starting {}",
        Utc::now().date_naive() + ChronoDuration::days(1)
    )
}

async fn resolve_active_identity(
    st: &Arc<AppState>,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
) -> (AutonomousDailySessionPlan, String, String, Uuid) {
    let timing = AutonomousDailyPlanTiming::production_default();
    let plan = match resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => {
            panic!("expected an applicable session plan for the test fixture date, got {other:?}")
        }
    };
    let config = MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: SYMBOL.to_string(),
            strategy_id: STRATEGY_ID.to_string(),
            timeframe: TIMEFRAME.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    let assignment_identity = derive_assignment_identity(&config);
    let runtime_context = resolve_autonomous_runtime_context(st)
        .await
        .expect("runtime context must resolve for the active fleet fixture");
    let runtime_binding_identity =
        derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let operation_id = derive_autonomous_daily_operation_id(
        &plan,
        "PAPER",
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );
    (
        plan,
        assignment_identity,
        runtime_binding_identity,
        operation_id,
    )
}

#[allow(clippy::too_many_arguments)]
async fn seed_operation_row(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    operation_id: Uuid,
    adapter_id: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<()> {
    let create_args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: NaiveDate::parse_from_str(&plan.market_date, "%Y-%m-%d")?,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: NaiveDate::parse_from_str(&plan.previous_trading_date, "%Y-%m-%d")?,
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "test fixture seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await?;
    Ok(())
}

/// Drive a freshly-seeded (`awaiting_open`) operation to `controller_degraded`
/// bound to `run_id` with the given `reason_code`, via the same legal edges
/// production uses (`awaiting_open -> start_retrying -> running ->
/// controller_degraded`). Never a raw `UPDATE` -- every hop is a real CAS
/// transition.
async fn seed_controller_degraded_operation(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    run_id: Uuid,
    reason_code: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<mqk_db::AutonomousDailyOperationRecord> {
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await?
        .expect("seeded row must exist");
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_START_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "test setup: -> start_retrying".to_string(),
    };
    let row = match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_RUNNING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> running".to_string(),
    };
    let row = match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_CONTROLLER_DEGRADED.to_string(),
        reason_code: Some(reason_code.to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: format!("test setup: -> controller_degraded ({reason_code})"),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => Ok(r),
        other => panic!("expected Applied, got {other:?}"),
    }
}

fn new_run(run_id: Uuid, now_utc: DateTime<Utc>) -> mqk_db::NewRun {
    mqk_db::NewRun {
        run_id,
        engine_id: "mqk-daemon".to_string(),
        mode: "PAPER".to_string(),
        started_at_utc: now_utc,
        git_hash: "TEST".to_string(),
        config_hash: "TEST".to_string(),
        config_json: serde_json::json!({}),
        host_fingerprint: "test-host".to_string(),
    }
}

async fn cleanup_operation(pool: &sqlx::PgPool, operation_id: Uuid) {
    let _ =
        sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id = $1")
            .bind(operation_id)
            .execute(pool)
            .await;
    let _ = sqlx::query("delete from sys_autonomous_daily_operations where operation_id = $1")
        .bind(operation_id)
        .execute(pool)
        .await;
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn cleanup_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await;
}

async fn seed_bars_via_normal_ingestion(pool: &sqlx::PgPool, expected_ts: &[i64]) {
    cleanup_bars(pool, SYMBOL, TIMEFRAME).await;
    let bars: Vec<mqk_db::md::ProviderBar> = expected_ts
        .iter()
        .map(|&end_ts| mqk_db::md::ProviderBar {
            symbol: SYMBOL.to_string(),
            timeframe: TIMEFRAME.to_string(),
            end_ts,
            open: "100".to_string(),
            high: "101".to_string(),
            low: "99".to_string(),
            close: "100.5".to_string(),
            volume: 1_000_000,
            is_complete: true,
        })
        .collect();
    let metadata = mqk_db::md::MdBarProviderMetadata {
        provider_id: PROVIDER_ID.to_string(),
        provider_source: Some(PROVIDER_ID.to_string()),
        provider_symbol: Some(SYMBOL.to_string()),
        ingest_mode: Some("provider_sync".to_string()),
        provider_bar_id: None,
        provider_updated_at_utc: None,
    };
    mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: PROVIDER_ID.to_string(),
            timeframe: TIMEFRAME.to_string(),
            ingest_id: Uuid::new_v4(),
            bars,
        },
        metadata,
    )
    .await
    .expect("normal metadata-aware ingestion must succeed");
}

fn expected_bar_window(now_utc: DateTime<Utc>, required: usize) -> Vec<i64> {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, now_utc);
    expected_intraday_end_ts_window(&provider, &schedule, now_utc.timestamp(), 300, 0, required)
        .expect("expected intraday window must resolve for the fixed weekday fixture")
}

/// Full fixture: a resolvable active-fleet env, a `controller_degraded`
/// operation bound to a genuinely `ARMED`/`RUNNING` `run_id` with
/// `reason_code = market_data_missing` (a readiness-classified Control
/// blocker -- the same class `interior_gap` belongs to). Bars are NOT
/// seeded (blocker genuinely still true) -- callers that need the cleared
/// case seed bars themselves via `seed_bars_via_normal_ingestion`.
async fn build_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
) -> anyhow::Result<(
    Arc<AppState>,
    AutonomousDailySessionPlan,
    Uuid,
    Uuid,
    DateTime<Utc>,
)> {
    reset_env();
    set_active_assignment_env();
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), adapter_id);
    active_fleet_st(&st).await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;

    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: now,
        },
    )
    .await?;

    cleanup_bars(pool, SYMBOL, TIMEFRAME).await;

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, adapter_id, now).await;
    seed_operation_row(
        pool,
        &plan,
        operation_id,
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(pool, &new_run(run_id, now)).await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    seed_controller_degraded_operation(pool, operation_id, run_id, REASON_MARKET_DATA_MISSING, now)
        .await?;
    Ok((st, plan, operation_id, run_id, now))
}

// ---------------------------------------------------------------------------
// A1 + A2: matching live local owner + blocker still genuinely active ->
// never falsely orphaned, never a premature recovery to running.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a1_a2_matching_local_owner_with_active_blocker_stays_degraded_never_orphaned(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a1a2-{}", unique_suffix());
    let (st, plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    // The live local runtime never stopped -- it still genuinely owns
    // run_id.
    st.inject_running_loop_for_test(run_id).await;
    assert_eq!(st.locally_owned_run_id().await, Some(run_id));

    // No bars seeded: the original readiness blocker is still genuinely
    // true.
    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let before_version = operation.state_version;
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;

    assert!(
        !matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                ..
            }
        ),
        "a live local owner must never be falsely reported as an ownerless durable run; \
         got {outcome:?}"
    );
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "controller_degraded",
                newly_applied: false,
            }
        ),
        "a still-active blocker with a matching live owner must remain controller_degraded \
         without a premature recovery; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_CONTROLLER_DEGRADED);
    assert_eq!(
        after.state_version, before_version,
        "an unrepaired blocker must not mutate durable state at all"
    );
    assert_eq!(after.run_id, Some(run_id));

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A3 + A7 + A8: matching live local owner + blocker genuinely cleared ->
// recovers to running with the same run_id, blocker fields cleared, and
// completed-bar dispatch eligibility restored.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a3_a7_a8_matching_local_owner_with_cleared_blocker_recovers_to_running(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a3-{}", unique_suffix());
    let (st, plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    st.inject_running_loop_for_test(run_id).await;

    // Repair data through the real production ingestion path -- canonical
    // readiness now genuinely passes.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = dispatch_by_state(&st, &pool, before.clone(), &plan, now).await?;

    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ControllerDegradedRecovered { run_id },
        "a genuinely cleared blocker with an unchanged live local owner must recover"
    );

    let recovered = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(recovered.state, mqk_db::STATE_RUNNING);
    assert_eq!(
        recovered.run_id,
        Some(run_id),
        "recovery must never create/duplicate a run -- the same run_id must be preserved"
    );
    assert_eq!(
        recovered.state_reason_code, None,
        "A7: the stale blocker reason must be cleared through the running-transition authority"
    );
    assert_eq!(
        recovered.state_blocker_signature, None,
        "A7: the stale blocker signature must be cleared through the running-transition authority"
    );
    assert_eq!(
        recovered.state_version,
        before.state_version + 1,
        "exactly one real durable CAS transition must have occurred"
    );

    let run_count: i64 = sqlx::query_scalar("select count(*) from runs where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        run_count, 1,
        "A3: exactly one run row must exist -- no duplicate runtime was created"
    );

    // A8: completed-bar dispatch eligibility is restored only now that the
    // operation is genuinely running again -- controller_degraded itself
    // never carries a driver mode.
    assert_eq!(
        select_driver_mode_for_state(mqk_db::STATE_CONTROLLER_DEGRADED),
        None,
        "controller_degraded must never be completed-bar-dispatch-eligible"
    );
    assert_eq!(
        select_driver_mode_for_state(&recovered.state),
        Some(AutonomousCompletedBarDriverMode::RunningDispatch),
        "A8: recovery to running must restore completed-bar dispatch eligibility"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A4: no local owner at all + durable ARMED/RUNNING -> existing fail-closed
// behavior is unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a4_no_local_owner_durable_active_run_stays_fail_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a4-{}", unique_suffix());
    let (st, plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    // Deliberately no `inject_running_loop_for_test` -- no local runtime
    // owns anything in this AppState.
    assert_eq!(st.locally_owned_run_id().await, None);

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                ..
            }
        ),
        "a genuinely running run with no local owner at all must keep failing closed exactly \
         as before this patch; got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A5: a local runtime owns a DIFFERENT run_id than this operation expects ->
// fails closed as runtime_run_id_mismatch, never recovers.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a5_mismatched_local_owner_run_id_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a5-{}", unique_suffix());
    let (st, plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    // Repair readiness too -- proves the mismatch guard fires BEFORE any
    // readiness revalidation is even attempted.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let other_run_id = Uuid::new_v4();
    st.inject_running_loop_for_test(other_run_id).await;
    assert_eq!(st.locally_owned_run_id().await, Some(other_run_id));

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = dispatch_by_state(&st, &pool, before.clone(), &plan, now).await?;

    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_run_id_mismatch",
                ..
            }
        ),
        "a local owner for a different run_id must fail closed rather than assume either \
         side is stale; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_ne!(
        after.state,
        mqk_db::STATE_RUNNING,
        "a run_id mismatch must never recover to running"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    cleanup_run(&pool, other_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A6: a concurrent transition wins the CAS first -- the coordinator's own
// attempt against the now-stale (state, state_version) it read must fail
// closed, never apply a favorable transition against stale truth.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a6_recovery_cas_race_never_applies_against_stale_state() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a6-{}", unique_suffix());
    let (st, plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    st.inject_running_loop_for_test(run_id).await;

    // The coordinator will read this stale snapshot -- readiness is
    // genuinely repaired, so without the CAS guard it would recover.
    let stale_snapshot = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    // A concurrent actor (e.g. an operator forcing manual intervention)
    // wins the CAS first, against the exact same (state, state_version)
    // the coordinator is about to attempt.
    let concurrent_args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: stale_snapshot.state.clone(),
        expected_state_version: stale_snapshot.state_version,
        new_state: mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        reason_code: Some("operator_forced_manual".to_string()),
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: Some(run_id),
        bounded_detail: "test: concurrent operator-forced transition wins first".to_string(),
    };
    let winner =
        match mqk_db::transition_autonomous_daily_operation(&pool, &concurrent_args).await? {
            AutonomousDailyTransitionOutcome::Applied(r) => r,
            other => panic!("expected Applied, got {other:?}"),
        };

    // The coordinator's attempt, still holding the now-stale snapshot, must
    // fail rather than silently overwrite the concurrent winner.
    let result = dispatch_by_state(&st, &pool, stale_snapshot, &plan, now).await;
    assert!(
        result.is_err(),
        "a stale CAS attempt must never silently apply a favorable transition; got {result:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state, winner.state,
        "the concurrent winner's state must remain authoritative"
    );
    assert_eq!(
        after.state_version, winner.state_version,
        "the stale attempt must not have produced any further mutation"
    );
    assert_ne!(
        after.state,
        mqk_db::STATE_RUNNING,
        "the stale attempt must never have recovered to running"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A9: a NonData-classified blocker (`provider_disabled`) must never
// auto-recover, even with a matching live owner and an otherwise-green
// readiness evaluation. FAILS against the pre-repair edc04de1 behavior,
// which accepted any reason string that merely parsed as a known
// `DailyDataReadinessReason` rather than requiring the closed
// `DataRepairable` classification.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a9_nondata_reason_provider_disabled_never_recovers_even_with_green_readiness(
) -> anyhow::Result<()> {
    use mqk_daemon::daily_data_readiness::REASON_PROVIDER_DISABLED;

    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a9-{}", unique_suffix());
    reset_env();
    set_active_assignment_env();
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: now,
        },
    )
    .await?;
    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(&pool, &new_run(run_id, now)).await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    seed_controller_degraded_operation(&pool, operation_id, run_id, REASON_PROVIDER_DISABLED, now)
        .await?;

    st.inject_running_loop_for_test(run_id).await;
    assert_eq!(st.locally_owned_run_id().await, Some(run_id));

    // Readiness is otherwise green -- repaired through the real production
    // ingestion path -- so only the NonData classification gate can be
    // responsible for staying degraded.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let before_version = operation.state_version;
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;

    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "controller_degraded",
                newly_applied: false,
            }
        ),
        "a NonData reason (provider_disabled) must never auto-recover even with green \
         readiness and a matching live owner; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_CONTROLLER_DEGRADED);
    assert_eq!(
        after.state_version, before_version,
        "a NonData blocker must never mutate durable state via this recovery path"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A10: a matching live owner whose durable run row is ARMED (not exactly
// RUNNING) must never auto-recover, even with an otherwise-green readiness
// evaluation. FAILS against the pre-repair edc04de1 behavior, which accepted
// `RunStatus::Armed` alongside `RunStatus::Running`.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a10_matching_owner_durable_armed_with_green_readiness_never_recovers() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a10-{}", unique_suffix());
    reset_env();
    set_active_assignment_env();
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: now,
        },
    )
    .await?;
    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(&pool, &new_run(run_id, now)).await?;
    mqk_db::arm_run(&pool, run_id).await?;
    // Deliberately never `begin_run` -- the durable run row stays ARMED, not
    // RUNNING.
    seed_controller_degraded_operation(
        &pool,
        operation_id,
        run_id,
        REASON_MARKET_DATA_MISSING,
        now,
    )
    .await?;

    // Readiness is otherwise green.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    st.inject_running_loop_for_test(run_id).await;
    assert_eq!(st.locally_owned_run_id().await, Some(run_id));

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;

    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "controller_degraded_local_owner_run_not_active",
                ..
            }
        ),
        "a durable run row that is ARMED (not exactly RUNNING) must never recover to running \
         even with green readiness and a matching live owner; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_ne!(
        after.state,
        mqk_db::STATE_RUNNING,
        "an ARMED (not RUNNING) durable run row must never recover to running"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// A11 (PATCH-A-RUN-STATUS-TOCTOU-CLOSE): a safety halt that lands on the
// bound run before the atomic recovery commit must block recovery -- the
// operation must never become `running` against a run that is no longer
// durably `RUNNING`, even when every upstream gate
// `attempt_controller_degraded_recovery` checks before reaching the atomic
// seam (matching local owner, `DataRepairable` blocker, genuinely green
// readiness) has already passed. Exercises the real production
// `mqk_db::recover_controller_degraded_operation_with_run_authority` seam
// directly -- not a reimplementation -- so this proves the seam's own
// run-row lock, not merely the coordinator's separate, unlocked pre-check
// (which this scenario deliberately never reaches). FAILS against the
// pre-repair behavior, which proved `RUNNING` once via a plain `fetch_run`
// and never re-checked it at the commit point, letting a concurrent halt
// land in between with no effect on the outcome.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn a11_run_halted_before_atomic_commit_recovery_must_refuse() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-live-a11-{}", unique_suffix());
    let (st, _plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    st.inject_running_loop_for_test(run_id).await;
    assert_eq!(st.locally_owned_run_id().await, Some(run_id));

    // Readiness genuinely repaired -- every upstream gate the coordinator
    // checks before entering the atomic seam has already passed for this
    // fixture.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");

    // The safety halt -- the exact production `halt_run` an execution-loop
    // safety trip calls -- wins the run-row race, landing before this
    // tick's atomic recovery commit.
    mqk_db::halt_run(&pool, run_id, now).await?;

    let outcome = mqk_db::recover_controller_degraded_operation_with_run_authority(
        &pool,
        &mqk_db::RecoverControllerDegradedArgs {
            operation_id,
            expected_state_version: operation.state_version,
            expected_run_id: run_id,
            occurred_at_utc: now,
            bounded_detail: "test: race proof -- safety halt wins before atomic commit".to_string(),
        },
    )
    .await?;

    assert!(
        matches!(
            &outcome,
            mqk_db::ControllerDegradedRecoveryOutcome::RunNotRunning { actual_status }
                if actual_status == "HALTED"
        ),
        "a run halted before the atomic commit point must refuse recovery; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "the operation must never become running when its bound run lost the race to a halt"
    );
    assert_eq!(
        after.state_version, operation.state_version,
        "a refused recovery must not mutate the operation row at all"
    );
    assert_eq!(after.run_id, Some(run_id));

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
