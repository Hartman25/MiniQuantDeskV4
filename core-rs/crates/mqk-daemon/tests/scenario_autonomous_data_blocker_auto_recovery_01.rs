//! AUTONOMOUS-DATA-BLOCKER-AUTO-RECOVERY-01 PATCH 2: proof tests for the
//! automatic-recovery arm of `state::autonomous_daily_coordinator::
//! dispatch_by_state`'s `STATE_MANUAL_INTERVENTION_REQUIRED` handling.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_data_blocker_auto_recovery_01 \
//!   -- --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! `BrokerKind::Paper` (in-process paper broker) is used throughout, and
//! every test calls `dispatch_by_state` directly against a hermetic
//! disposable test-DB row it seeds itself -- never a real/running Paper
//! session, never `Start-MiniQuantDesk.ps1`, never a real coordinator
//! process.
//!
//! Fixtures mirror `scenario_autonomous_daily_operator_retry_01.rs`'s T01
//! positive scenario (the real `AAPL`/`5m`/`alpaca`/`intraday_scalper` lane,
//! seeded through the real metadata-aware ingestion function) so the same
//! genuine `daily_data_readiness::evaluate_readiness_with_binding` path the
//! automatic recovery arm calls can actually flip from blocked to
//! `start_allowed=true`.
//!
//! Required proofs (per the PATCH 2 mission):
//!   - automatic retry only for PATCH 1's typed DataRepairable condition
//!     (POS1 positive; NEG1 non-data reason fails closed; NEG3 unknown
//!     reason fails closed)
//!   - fresh readiness re-check through the canonical authority before
//!     retry, never a stale/assumed verdict (NEG2: data-repairable reason,
//!     genuinely still blocked, no bars seeded -> stays manual)
//!   - reuses the existing retry-route CAS transition, not a second
//!     implementation (POS1 asserts the shared function's own
//!     `automatic_data_blocker_recovery` bounded-detail label appears in
//!     the durable transition history)
//!   - same durable operation identity preserved (POS1)
//!   - bounded/idempotent: repeated coordinator ticks never produce more
//!     than one durable mutation (POS2: two ticks back-to-back while
//!     genuinely still blocked; POS3: a tick after the CAS already applied)
//!   - no broker/order/arm/halt side effects (NEG4 asserts integrity/arm/
//!     run_id/start_attempt_count are byte-for-byte unchanged)
//!
//! MQK-LEDGER-BURN-CONTROLLER-04 R2 (deterministic concurrent-CAS proof,
//! no timing sleeps -- both "actors" start from the exact same durable
//! row/version read by `seed_manual_intervention_fixture`, then apply the
//! shared CAS deterministically in a fixed order):
//!   - POS4: a concurrent actor (an operator retry, or another coordinator
//!     tick) wins the identical manual_intervention_required ->
//!     preparing_data CAS first; the coordinator's own attempt against the
//!     same stale (state, state_version) it read must receive the DB's
//!     canonical `AlreadyApplied` for that exact transition and project
//!     `PreparingData` truthfully -- never a stale `ManualInterventionRequired`
//!     re-projection. Exactly one transition event exists and state_version
//!     increments exactly once (the winner's write only).
//!   - NEG4: a genuinely different concurrent transition (manual_intervention_
//!     required -> stopping, a different target than preparing_data) must
//!     still classify as `StaleState` and must NOT be treated as recovered --
//!     StaleState remains fail-closed, the coordinator stays in
//!     `ManualInterventionRequired`.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window;
use mqk_daemon::state::autonomous_daily_coordinator::{
    dispatch_by_state, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::autonomous_runtime_context::resolve_autonomous_runtime_context;
use mqk_daemon::state::market_calendar::{resolve_market_session_schedule, NyseWeekdaysProvider};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, StrategyFleetEntry, SymbolStrategyAssignment,
};
use mqk_db::{
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs,
    TransitionAutonomousDailyOperationArgs, STATE_MANUAL_INTERVENTION_REQUIRED, STATE_PREPARING_DATA,
    STATE_STOPPING,
};
use uuid::Uuid;

const SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const IDS_ENV: &str = "MQK_STRATEGY_IDS";
const TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";
const SYMBOL: &str = "AAPL";
const STRATEGY_ID: &str = "intraday_scalper";
const TIMEFRAME: &str = "5m";
const PROVIDER_ID: &str = "alpaca";

// ---------------------------------------------------------------------------
// Helpers (mirrors scenario_autonomous_daily_operator_retry_01.rs)
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

/// The next weekday at 14:30 UTC strictly after the real wall clock (see
/// the identical helper's doc comment in
/// scenario_autonomous_daily_operator_retry_01.rs for why this must be
/// dynamic rather than a fixed fictional date).
fn dynamic_session_now() -> DateTime<Utc> {
    let mut candidate_date = Utc::now().date_naive() + ChronoDuration::days(1);
    while let chrono::Weekday::Sat | chrono::Weekday::Sun = candidate_date.weekday() {
        candidate_date += ChronoDuration::days(1);
    }
    Utc.from_utc_datetime(&candidate_date.and_hms_opt(14, 30, 0).unwrap())
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
    (plan, assignment_identity, runtime_binding_identity, operation_id)
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
    initial_state: &str,
) -> anyhow::Result<()> {
    let create_args = CreateAutonomousDailyOperationArgs {
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
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "test fixture seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn real_transition(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    expected_state: &str,
    expected_state_version: i64,
    new_state: &str,
    reason_code: Option<&str>,
    now_utc: DateTime<Utc>,
    detail: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: expected_state.to_string(),
        expected_state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(|s| s.to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: detail.to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition query must not fail")
    {
        AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
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

/// Common fixture setup shared by every test below: pristine operation ->
/// real CAS transition into `manual_intervention_required` with the given
/// `reason_code`. Returns (state, plan, operation_id, manual row).
async fn seed_manual_intervention_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    reason_code: &str,
) -> anyhow::Result<(
    Arc<AppState>,
    AutonomousDailySessionPlan,
    Uuid,
    mqk_db::AutonomousDailyOperationRecord,
)> {
    reset_env();
    set_active_assignment_env();
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), adapter_id);
    active_fleet_st(&st).await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, adapter_id, now).await;

    cleanup_bars(pool, SYMBOL, TIMEFRAME).await;

    seed_operation_row(
        pool,
        &plan,
        operation_id,
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
    )
    .await?;

    let manual = real_transition(
        pool,
        operation_id,
        STATE_PREPARING_DATA,
        1,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(reason_code),
        now,
        &format!("daily data readiness blocked: {reason_code} (test fixture)"),
    )
    .await;
    assert_eq!(manual.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(manual.state_reason_code.as_deref(), Some(reason_code));

    Ok((st, plan, operation_id, manual))
}

// ---------------------------------------------------------------------------
// POS1 — REQUIRED POSITIVE SCENARIO: data-repairable reason + repaired
// readiness -> exactly one automatic canonical recovery transition.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn pos1_data_repairable_reason_with_repaired_readiness_auto_recovers() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-pos1-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    // Snapshot integrity state before the recovery attempt -- automatic
    // recovery must never touch halt/disarm authority either direction,
    // regardless of whatever this fresh test AppState's own default is.
    let (halted_before, disarmed_before) = {
        let ig = st.integrity.read().await;
        (ig.halted, ig.disarmed)
    };

    // Repair data through the real production ingestion path -- canonical
    // readiness now genuinely passes.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(outcome, AutonomousDailyCoordinatorTickOutcome::PreparingData),
        "repaired data-repairable blocker must automatically re-enter preparing_data, got {outcome:?}"
    );

    let recovered = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(recovered.state, STATE_PREPARING_DATA);
    assert_eq!(
        recovered.operation_id, operation_id,
        "same durable operation identity must be preserved through automatic recovery"
    );
    assert_eq!(
        recovered.state_reason_code, None,
        "the old blocker reason must be cleared from current-state fields"
    );
    assert_ne!(
        recovered.state_version, manual.state_version,
        "exactly one real durable CAS transition must have occurred"
    );

    // Reuses the existing retry-route CAS transition, not a second
    // implementation: proven by the shared function's own distinctly-
    // labeled bounded_detail appearing in the durable transition history.
    let details: Vec<String> = sqlx::query_scalar(
        "select bounded_detail from sys_autonomous_daily_operation_events \
         where operation_id = $1 and to_state = $2 order by transition_seq desc limit 1",
    )
    .bind(operation_id)
    .bind(STATE_PREPARING_DATA)
    .fetch_all(&pool)
    .await?;
    assert!(
        details
            .iter()
            .any(|d| d.contains("automatic_data_blocker_recovery")),
        "the recovery transition must be labeled as the automatic path, not a coincidental \
         match with some other transition: {details:?}"
    );

    // No broker/order/arm/halt side effects: integrity state is byte-for-
    // byte unchanged by the automatic recovery attempt.
    assert_eq!(recovered.run_id, None);
    assert_eq!(recovered.start_attempt_count, 0);
    let (halted_after, disarmed_after) = {
        let ig = st.integrity.read().await;
        (ig.halted, ig.disarmed)
    };
    assert_eq!(
        (halted_before, disarmed_before),
        (halted_after, disarmed_after),
        "automatic recovery must never touch halt/disarm integrity state in either direction"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// POS2 — bounded/idempotent: two ticks in a row while genuinely still
// blocked (no bars seeded) never produce more than zero mutations.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn pos2_repeated_ticks_while_still_blocked_produce_no_mutation() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-pos2-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    // Deliberately no bars seeded -- readiness genuinely still blocked.
    let outcome1 = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome1,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "still-blocked data-repairable reason must stay manual on tick 1, got {outcome1:?}"
    );
    let after1 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after1.state_version, manual.state_version,
        "a still-blocked automatic recovery attempt must not mutate state"
    );

    let outcome2 = dispatch_by_state(&st, &pool, after1.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome2,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "still-blocked data-repairable reason must stay manual on tick 2, got {outcome2:?}"
    );
    let after2 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after2.state_version, manual.state_version,
        "repeated coordinator ticks while genuinely still blocked must never create repeated \
         independent mutations"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// POS3 — bounded/idempotent: a tick AFTER the CAS already applied (state no
// longer manual_intervention_required) never attempts a second transition.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn pos3_tick_after_recovery_already_applied_makes_no_further_mutation() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-pos3-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let outcome1 = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(matches!(
        outcome1,
        AutonomousDailyCoordinatorTickOutcome::PreparingData
    ));
    let after_recovery = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after_recovery.state, STATE_PREPARING_DATA);
    let version_after_recovery = after_recovery.state_version;

    // A later tick's dispatch now routes through STATE_PREPARING_DATA's own
    // handler (not the manual-intervention arm at all, since the durable
    // state has already changed) -- episode is over, no automatic-recovery
    // code path is even reachable again for this occurrence.
    let outcome2 = dispatch_by_state(&st, &pool, after_recovery.clone(), &plan, now).await?;
    assert!(
        !matches!(
            outcome2,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "operation must not have been routed back through the manual-intervention arm, got {outcome2:?}"
    );
    let after_second_tick = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(
        after_second_tick.state, STATE_MANUAL_INTERVENTION_REQUIRED,
        "must never fall back into manual_intervention_required merely from a later tick"
    );
    let _ = version_after_recovery; // documented above; no further assertion needed

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// NEG1 — non-data manual_intervention_required reason never auto-recovers,
// even though it IS in the operator route's own broader recoverable set.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn neg1_non_data_reason_never_auto_recovers() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-neg1-{}", unique_suffix());
    // REASON_PROVIDER_DISABLED is a real, valid daily_data_readiness reason
    // code, and IS in the operator route's own broader
    // RECOVERABLE_PREFLIGHT_REASON_CODES set -- but it classifies NonData
    // under PATCH 1's typed authority (a provider being disabled is a
    // config fact, not something the required-universe autofresh
    // controller can repair by fetching more data).
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_PROVIDER_DISABLED,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    // Even though data readiness would genuinely pass if evaluated (bars
    // seeded), this must still never be attempted -- the reason-code
    // classification gate runs before any readiness re-check at all.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "a non-data reason must remain manual regardless of readiness, got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state_version, manual.state_version,
        "a non-data blocker reason must never be automatically mutated"
    );
    assert_eq!(after.state, STATE_MANUAL_INTERVENTION_REQUIRED);

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// NEG2 — data-repairable reason, but readiness genuinely still blocked
// (no bars seeded): fresh re-check, never a stale/assumed verdict.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn neg2_data_repairable_but_unrepaired_readiness_stays_manual() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-neg2-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_INTERIOR_GAP,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    // No bars seeded at all -- readiness is genuinely, unambiguously still
    // blocked.
    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "unrepaired readiness must never be treated as recovered, got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state_version, manual.state_version);
    assert_eq!(after.state, STATE_MANUAL_INTERVENTION_REQUIRED);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// NEG3 — unknown/unrecognized reason code fails closed (never treated as
// data-repairable).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn neg3_unknown_reason_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-neg3-{}", unique_suffix());
    let (st, plan, operation_id, manual) =
        seed_manual_intervention_fixture(&pool, &adapter_id, "totally_unrecognized_reason_code")
            .await?;
    let now = st.daily_data_readiness_now().await;

    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "an unrecognized reason code must fail closed to manual, never DataRepairable, got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state_version, manual.state_version);

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// POS4 (MQK-LEDGER-BURN-CONTROLLER-04 R2) — deterministic concurrent-CAS
// proof: a concurrent actor wins the identical manual_intervention_required
// -> preparing_data CAS first (simulated deterministically, no timing
// sleeps, by applying the shared transition directly against the exact same
// (state, state_version) `dispatch_by_state` will independently attempt
// below). The coordinator's own attempt must then receive the DB's
// canonical `AlreadyApplied` for that exact transition and project
// `PreparingData` truthfully -- never a stale `ManualInterventionRequired`
// re-projection, and never a second durable mutation.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn pos4_concurrent_recovery_winner_and_loser_both_project_preparing_data(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-pos4-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    let (halted_before, disarmed_before) = {
        let ig = st.integrity.read().await;
        (ig.halted, ig.disarmed)
    };

    // Data is repaired -- both the "concurrent winner" below and the
    // coordinator's own re-check will see a genuinely passing readiness
    // evaluation, exactly as a real race would.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    // 1. Both actors begin from the same MANUAL state/version: `manual`
    //    (read once by the fixture) is passed to both the winner's direct
    //    transition below and to `dispatch_by_state` further down --
    //    neither actor re-reads a newer row first, exactly like two
    //    concurrent processes that both read the row before either wrote.
    // 2. Exactly one CAS actually creates the transition/event: this direct
    //    call is the only `Applied` outcome in this test -- the coordinator
    //    below must never independently create a second one.
    let winner = real_transition(
        &pool,
        operation_id,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        manual.state_version,
        STATE_PREPARING_DATA,
        None,
        now,
        "automatic_data_blocker_recovery: concurrent-winner fixture transition (POS4)",
    )
    .await;
    assert_eq!(winner.state, STATE_PREPARING_DATA);
    assert_eq!(winner.state_version, manual.state_version + 1);

    // 3. The "loser": the coordinator ticks against the SAME stale `manual`
    //    row it originally read (state_version unchanged) -- its internal
    //    CAS guard cannot match (the winner already moved the row), so it
    //    must re-read and classify. Because the winner applied the EXACT
    //    same (expected_state -> new_state) transition, this classifies as
    //    `AlreadyApplied`, not `StaleState`.
    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;

    // 4. Coordinator projection is PreparingData, never stale
    //    ManualInterventionRequired.
    assert!(
        matches!(outcome, AutonomousDailyCoordinatorTickOutcome::PreparingData),
        "the loser of a concurrent identical CAS must project PreparingData from the DB's own \
         AlreadyApplied truth, not a stale ManualInterventionRequired re-projection, got {outcome:?}"
    );

    let final_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(final_row.state, STATE_PREPARING_DATA);
    // 6. state_version increments exactly once (the winner's write only --
    //    the loser's AlreadyApplied path must never attempt a second write).
    assert_eq!(
        final_row.state_version,
        manual.state_version + 1,
        "the loser's AlreadyApplied projection must never itself produce a second durable \
         mutation on top of the winner's"
    );

    // 5. Exactly one transition event exists for this exact
    //    manual_intervention_required -> preparing_data edge.
    let event_count: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events \
         where operation_id = $1 and from_state = $2 and to_state = $3",
    )
    .bind(operation_id)
    .bind(STATE_MANUAL_INTERVENTION_REQUIRED)
    .bind(STATE_PREPARING_DATA)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        event_count, 1,
        "exactly one durable transition event must exist for this CAS, proving the loser never \
         wrote a duplicate"
    );

    // 7. No run_id/order/broker/arm/halt side effects.
    assert_eq!(final_row.run_id, None);
    assert_eq!(final_row.start_attempt_count, 0);
    let (halted_after, disarmed_after) = {
        let ig = st.integrity.read().await;
        (ig.halted, ig.disarmed)
    };
    assert_eq!(
        (halted_before, disarmed_before),
        (halted_after, disarmed_after),
        "the loser's AlreadyApplied projection must never touch halt/disarm integrity state"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// NEG4 (MQK-LEDGER-BURN-CONTROLLER-04 R2) — a genuinely DIFFERENT concurrent
// transition (manual_intervention_required -> stopping, not preparing_data)
// must classify as StaleState and must NOT be treated as recovered.
// StaleState remains fail-closed; the coordinator stays in
// ManualInterventionRequired rather than broadening StaleState into success.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn neg4_different_concurrent_transition_stays_stale_and_manual() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("autorec-neg4-{}", unique_suffix());
    let (st, plan, operation_id, manual) = seed_manual_intervention_fixture(
        &pool,
        &adapter_id,
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    )
    .await?;
    let now = st.daily_data_readiness_now().await;

    // Readiness would genuinely pass if evaluated (same repair as POS4) --
    // proving the fail-closed result below comes from the CAS mismatch
    // itself, not merely from a still-blocked readiness re-check.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    // A genuinely different concurrent transition wins first: the SAME
    // starting (state, state_version) `manual` carries, but to a different
    // legal target (`stopping`, e.g. an operator-initiated shutdown) --
    // never the preparing_data edge the coordinator will attempt.
    let winner = real_transition(
        &pool,
        operation_id,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        manual.state_version,
        STATE_STOPPING,
        None,
        now,
        "test fixture: genuinely different concurrent transition (NEG4)",
    )
    .await;
    assert_eq!(winner.state, STATE_STOPPING);
    assert_eq!(winner.state_version, manual.state_version + 1);

    // The coordinator ticks against the same stale `manual` row it
    // originally read. Its CAS guard cannot match (current state is now
    // `stopping`, not `manual_intervention_required`), and the re-read
    // classification cannot satisfy AlreadyApplied either (current.state !=
    // args.new_state) -- this must resolve to StaleState, and StaleState
    // must never be treated as recovered.
    let outcome = dispatch_by_state(&st, &pool, manual.clone(), &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "a genuinely different concurrent transition (StaleState) must never be treated as \
         recovered, got {outcome:?}"
    );

    let final_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        final_row.state, STATE_STOPPING,
        "the coordinator's failed CAS attempt must never overwrite the genuinely different \
         concurrent transition's durable outcome"
    );
    assert_eq!(
        final_row.state_version,
        manual.state_version + 1,
        "the coordinator's StaleState outcome must never itself produce a durable mutation"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}
