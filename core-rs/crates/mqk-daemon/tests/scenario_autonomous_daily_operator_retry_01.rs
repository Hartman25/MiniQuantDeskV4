//! AUTONOMOUS-DAILY-OPERATOR-RETRY-01: proof tests for
//! `POST /api/v1/autonomous/daily-operation/retry`.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_operator_retry_01 \
//!   -- --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! `BrokerKind::Paper` (in-process paper broker) is used throughout.
//!
//! The positive scenario (T01) uses the real `AAPL`/`5m`/`alpaca` lane
//! (`config/instruments/equities.json`, `config/providers/providers.json`)
//! together with the real `intraday_scalper` strategy engine (the only
//! engine whose declared timeframe is 5 minutes) so the full production
//! `daily_data_readiness::evaluate_readiness_with_binding` path — the exact
//! function the new retry route calls — can genuinely flip from blocked to
//! `start_allowed=true` once bars are seeded through the real
//! metadata-aware ingestion function. `swing_momentum` (this repo's other
//! common test-fixture strategy) is deliberately not used here: it is a 1D
//! strategy, and 1D evaluation is unconditionally blocked by
//! `provider_timestamp_convention_unverified` for every provider except
//! `twelvedata` (see `daily_data_readiness.rs`), so it can never reach
//! `ready` and cannot prove this route's full recovery path end to end.

use std::sync::Arc;

use axum::http::{header, Method, Request, StatusCode};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window;
use mqk_daemon::state::autonomous_daily_coordinator::attempt_canonical_start;
use mqk_daemon::state::autonomous_runtime_context::resolve_autonomous_runtime_context;
use mqk_daemon::state::market_calendar::{resolve_market_session_schedule, NyseWeekdaysProvider};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, OperatorAuthMode, StrategyFleetEntry,
    SymbolStrategyAssignment,
};
use mqk_db::{
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs,
    TransitionAutonomousDailyOperationArgs, STATE_AWAITING_OPEN, STATE_MANUAL_INTERVENTION_REQUIRED,
    STATE_PREPARING_DATA, STATE_RECOVERY_RETRYING, STATE_RUNNING, STATE_START_RETRYING,
};
use tower::ServiceExt;
use uuid::Uuid;

const SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const IDS_ENV: &str = "MQK_STRATEGY_IDS";
const TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";
const SYMBOL: &str = "AAPL";
const STRATEGY_ID: &str = "intraday_scalper";
const TIMEFRAME: &str = "5m";
const PROVIDER_ID: &str = "alpaca";
const RETRY_ROUTE: &str = "/api/v1/autonomous/daily-operation/retry";

// ---------------------------------------------------------------------------
// Helpers
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

/// Repo-root-relative config paths resolve correctly regardless of the
/// process's actual working directory when `cargo test` runs.
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

/// Sets env so both `build_multi_symbol_runtime_config_from_env` (legacy
/// single-symbol fallback) and the real native-strategy bootstrap resolve
/// the *same* symbol/strategy/timeframe, and points the readiness evaluator
/// at the real repo-root config files (never a synthetic injected registry —
/// this route calls `load_readiness_context_from_env` in production).
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

fn paper_state_with_db_and_auth(
    db: sqlx::PgPool,
    adapter_id: &str,
    auth: OperatorAuthMode,
) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    // Rebuild with the requested auth mode (test-only constructor composition:
    // `new_for_test_with_db_mode_and_broker` always uses `ExplicitDevNoToken`).
    let mut st2 = AppState::new_with_db_and_operator_auth(
        st.db.clone().expect("db must be set"),
        auth,
    );
    st2.set_adapter_id_for_test(adapter_id);
    let _ = st; // drop the throwaway state used only to construct the pool clone above
    Arc::new(st2)
}

async fn active_fleet_st(st: &Arc<AppState>) {
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: STRATEGY_ID.to_string(),
    }]))
    .await;
}

/// A weekday at a fixed wall-clock time (2026-07-20 is a Monday), offset
/// `hour`:`minute` UTC. Matches the fixture date already proven applicable
/// by `scenario_autonomous_daily_session_coordinator_01.rs`. Safe for any
/// fixture that never seeds real market-data rows (their `ingested_at` is
/// unavoidably stamped with the real wall clock at insert time, which is
/// nowhere near this fixed fictional date — see [`dynamic_session_now`]).
fn weekday_at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, 0).unwrap()
}

/// The next weekday at 14:30 UTC strictly after the real wall clock, always
/// at least ~10 hours out. Required for every fixture that seeds real
/// `md_bars` rows through the normal ingestion path: `ingested_at` is
/// stamped with the real wall clock at insert time (`db_rules.md`: callers
/// supply time, but `md_bars.ingested_at` is a `default now()` column — see
/// migration 0003 — outside this route's control), so the readiness
/// evaluator's `provider_ingest_time_future` check requires the fixture
/// `now_utc` to stay comfortably ahead of real insert time, not pinned to
/// an arbitrary fictional date. Always resolves to a Mon-Fri date, matching
/// the `nyse_weekdays_heuristic` calendar's simple weekday-only coverage
/// (no holiday table), so `resolve_autonomous_daily_session_plan_from_env`
/// reliably reports `Applicable`.
fn dynamic_session_now() -> DateTime<Utc> {
    let mut candidate_date = Utc::now().date_naive() + ChronoDuration::days(1);
    loop {
        match candidate_date.weekday() {
            chrono::Weekday::Sat | chrono::Weekday::Sun => {
                candidate_date += ChronoDuration::days(1);
            }
            _ => break,
        }
    }
    Utc.from_utc_datetime(&candidate_date.and_hms_opt(14, 30, 0).unwrap())
}

/// Resolve today's REAL canonical identity for the active (non-Dormant)
/// fleet/env configuration set by [`set_active_assignment_env`] +
/// [`active_fleet_st`]. Never hand-constructs the runtime binding — derives
/// it from the same [`resolve_autonomous_runtime_context`] call the
/// coordinator and the retry route both use, so the resulting
/// `operation_id` is guaranteed to match whatever the route independently
/// recomputes.
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
    deployment_mode: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
    initial_state: &str,
    close_override: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    let create_args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: NaiveDate::parse_from_str(&plan.market_date, "%Y-%m-%d")?,
        deployment_mode: deployment_mode.to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: close_override.unwrap_or(plan.effective_operation_close_utc),
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

/// Apply one real CAS transition, panicking (test failure) unless it
/// `Applied`. Every fixture in this file reaches `manual_intervention_required`
/// (and, for R2, `running`) only through this — the real production CAS
/// primitive the coordinator itself uses — never a raw `UPDATE`.
async fn real_transition(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    expected_state: &str,
    expected_state_version: i64,
    new_state: &str,
    reason_code: Option<&str>,
    run_id: Option<Uuid>,
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
        run_id,
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

/// Seed `md_bars` via the real production metadata-aware ingest function —
/// never a raw `INSERT` — matching the accepted
/// MARKET-DATA-PROVIDER-PROVENANCE-01 provenance shape exactly (see
/// `scenario_daily_data_readiness_01.rs`'s `seed_bars_via_normal_ingestion`,
/// which this mirrors).
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

/// The exact 5m completed-bar window the production readiness evaluator
/// expects before `now_utc`, computed via the same production function
/// (`expected_intraday_end_ts_window`) the evaluator itself calls — never
/// hand-computed, so a fixture always matches by construction.
fn expected_bar_window(now_utc: DateTime<Utc>, required: usize) -> Vec<i64> {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, now_utc);
    expected_intraday_end_ts_window(&provider, &schedule, now_utc.timestamp(), 300, 0, required)
        .expect("expected intraday window must resolve for the fixed weekday fixture")
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("response body is not valid JSON");
    (status, json)
}

fn retry_req(operation_id: Uuid, expected_market_date: Option<&str>) -> Request<axum::body::Body> {
    let mut body = serde_json::json!({ "operation_id": operation_id.to_string() });
    if let Some(d) = expected_market_date {
        body["expected_market_date"] = serde_json::Value::String(d.to_string());
    }
    Request::builder()
        .method(Method::POST)
        .uri(RETRY_ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn retry_req_authed(operation_id: Uuid, token: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "operation_id": operation_id.to_string() });
    Request::builder()
        .method(Method::POST)
        .uri(RETRY_ROUTE)
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// T01 — REQUIRED POSITIVE SCENARIO (§31): full recovery lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t01_full_recovery_lifecycle_market_data_repair() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-t01-{}", unique_suffix());
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;

    // 1-3: pristine operation, data readiness genuinely fails (no bars).
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;

    // 4: coordinator durably transitions to manual_intervention_required with
    // a real recoverable (market-data) reason code.
    let manual = real_transition(
        &pool,
        operation_id,
        STATE_PREPARING_DATA,
        1,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING),
        None,
        now,
        "daily data readiness blocked: market_data_missing (test fixture)",
    )
    .await;
    assert_eq!(manual.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(
        manual.state_reason_code.as_deref(),
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING)
    );

    // 5: retry while blocker still exists -> 409 still_blocked, state unchanged.
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "still blocked: {json}");
    assert_eq!(json["truth_state"], "still_blocked", "{json}");

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.state, STATE_MANUAL_INTERVENTION_REQUIRED,
        "a still-blocked retry must not mutate state"
    );
    assert_eq!(row.state_version, manual.state_version);

    // 6-7: repair data through normal ingestion; canonical readiness now passes.
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    // 8-9: POST retry with the exact operation_id -> durable state becomes
    // preparing_data; old blocker cleared from current-state fields.
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, Some(&plan.market_date)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "recovery must succeed: {json}");
    assert_eq!(json["truth_state"], "recovered", "{json}");
    assert_eq!(json["new_state"], STATE_PREPARING_DATA, "{json}");
    assert_eq!(
        json["previous_reason_code"],
        mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING,
        "{json}"
    );
    assert_eq!(json["runtime_started"], false, "{json}");
    assert_eq!(json["arm_modified"], false, "{json}");
    assert_eq!(json["halt_changed"], false, "{json}");
    assert_eq!(json["reconcile_changed"], false, "{json}");
    assert_eq!(json["orders_submitted"], 0, "{json}");

    let recovered = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(recovered.state, STATE_PREPARING_DATA);
    assert_eq!(
        recovered.state_reason_code, None,
        "the old blocker reason must be cleared from current-state fields"
    );
    assert_eq!(recovered.state_blocker_signature, None);

    // 11: transition history preserves the original blocker event alongside
    // the recovery transition — both rows must exist, append-only.
    let events: Vec<(String, String)> = sqlx::query_as(
        "select from_state, to_state from sys_autonomous_daily_operation_events \
         where operation_id = $1 order by transition_seq asc",
    )
    .bind(operation_id)
    .fetch_all(&pool)
    .await?;
    assert!(
        events
            .iter()
            .any(|(from, to)| from == STATE_PREPARING_DATA && to == STATE_MANUAL_INTERVENTION_REQUIRED),
        "original blocker event must be preserved: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|(from, to)| from == STATE_MANUAL_INTERVENTION_REQUIRED && to == STATE_PREPARING_DATA),
        "recovery transition event must be recorded: {events:?}"
    );

    // 12-13: next coordinator tick reevaluates readiness and progresses.
    let outcome = mqk_daemon::state::autonomous_daily_coordinator::dispatch_by_state(
        &st, &pool, recovered, &plan, now,
    )
    .await?;
    assert!(
        matches!(
            outcome,
            mqk_daemon::state::autonomous_daily_coordinator::AutonomousDailyCoordinatorTickOutcome::AwaitingOpen
                | mqk_daemon::state::autonomous_daily_coordinator::AutonomousDailyCoordinatorTickOutcome::PreparingData
        ),
        "coordinator must progress toward awaiting_open with readiness now satisfied, got {outcome:?}"
    );

    // 27: idempotency — a second retry call performs no mutation.
    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let (status2, json2) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    // The coordinator dispatch above already advanced the operation past
    // `preparing_data` (to `awaiting_open`), so the route's honest, non-
    // fabricated idempotency answer here is `409 not_manual` — it never
    // claims to know this state resulted from a prior retry versus ordinary
    // lifecycle progress. Either way, no mutation.
    assert!(
        (status2 == StatusCode::OK && json2["truth_state"] == "already_recovered")
            || (status2 == StatusCode::CONFLICT && json2["truth_state"] == "not_manual"),
        "second retry must be a no-op idempotent replay: status={status2} body={json2}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state_version, before.state_version,
        "second retry must not mutate the row"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R2 — runtime history present -> refused
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r02_runtime_history_present_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r02-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;

    // Drive through the real legal chain to a `running` operation with a
    // bound `run_id`, then to `manual_intervention_required` — proving this
    // operation carries genuine runtime history, never faked via raw SQL.
    let v2 = real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_AWAITING_OPEN, None, None, now,
        "test: preopen readiness satisfied",
    )
    .await;
    let v3 = real_transition(
        &pool, operation_id, STATE_AWAITING_OPEN, v2.state_version, STATE_START_RETRYING, None,
        None, now, "test: entering start sequence",
    )
    .await;

    let run_id = Uuid::new_v4();
    let running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id,
        expected_state: STATE_START_RETRYING.to_string(),
        expected_state_version: v3.state_version,
        run_id,
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test: canonical start succeeded".to_string(),
    };
    let v4 = match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &running_args)
        .await?
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(v4.run_id, Some(run_id));

    let v5 = real_transition(
        &pool, operation_id, STATE_RUNNING, v4.state_version, STATE_RECOVERY_RETRYING, None,
        None, now, "test: runtime ended without halt",
    )
    .await;
    let manual = real_transition(
        &pool, operation_id, STATE_RECOVERY_RETRYING, v5.state_version,
        STATE_MANUAL_INTERVENTION_REQUIRED, Some("runtime_run_id_mismatch"), None, now,
        "test: runtime run id mismatch",
    )
    .await;
    assert_eq!(manual.run_id, Some(run_id), "run_id must survive into manual state");

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_recoverable", "{json}");

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, STATE_MANUAL_INTERVENTION_REQUIRED, "must be unmutated");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R3 — operation identity mismatch -> refused
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r03_stale_identity_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r03-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, _assignment_identity, _runtime_binding_identity, _real_operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    // Deliberately stale identity fields + a random operation_id — this can
    // never equal what the route independently (re)derives for today's
    // canonical configuration.
    let operation_id = Uuid::new_v4();
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        "stale-assignment-identity",
        "stale-runtime-binding-identity",
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: stale identity fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_recoverable", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R4 — session window closed -> refused
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r04_session_closed_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r04-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        Some(now - ChronoDuration::minutes(1)), // already closed
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: session-closed fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "session_closed", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R5 — wrong operation_id -> not_found
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r05_wrong_operation_id_not_found() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r05-{}", unique_suffix());
    let st = paper_state_with_db(pool.clone(), &adapter_id);

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(Uuid::new_v4(), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["truth_state"], "not_found", "{json}");
    Ok(())
}

// ---------------------------------------------------------------------------
// R6 — live deployment -> refused
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r06_live_deployment_not_authorized() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r06-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, _real_operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    let operation_id = Uuid::new_v4();
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "LIVE",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: live-mode fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["truth_state"], "not_authorized", "{json}");

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, STATE_MANUAL_INTERVENTION_REQUIRED);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R7 — CAS race: a stale (expected_state, expected_state_version) must
// never be applied. Proven directly against the canonical CAS primitive
// this route uses (`mqk_db::transition_autonomous_daily_operation`) — the
// same function/outcome the route maps to `409 conflict`.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r07_stale_cas_version_is_refused_not_applied() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r07-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    let manual = real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: race fixture blocker",
    )
    .await;
    let captured_state = manual.state.clone();
    let captured_version = manual.state_version;

    // Simulate a second writer moving the row before this route's own CAS
    // write lands.
    real_transition(
        &pool, operation_id, STATE_MANUAL_INTERVENTION_REQUIRED, captured_version,
        mqk_db::STATE_PREFLIGHT_BLOCKED, None, None, now,
        "test: concurrent writer moved the row",
    )
    .await;

    // The retry route's own transition attempt, built from the
    // pre-race (state, state_version) it would have captured on its own
    // fetch, must be refused as stale — never silently applied against a
    // row that has since moved.
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: captured_state,
        expected_state_version: captured_version,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "test: stale retry attempt".to_string(),
    };
    let outcome = mqk_db::transition_autonomous_daily_operation(&pool, &args).await?;
    assert!(
        matches!(outcome, AutonomousDailyTransitionOutcome::StaleState { .. }),
        "a stale CAS attempt must be refused, got {outcome:?}"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.state,
        mqk_db::STATE_PREFLIGHT_BLOCKED,
        "the concurrent writer's state must remain authoritative"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R9 — halt / kill-switch / reconcile authorities are never touched by a
// successful retry.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r09_successful_retry_never_touches_halt_or_arm_authority() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r09-{}", unique_suffix());
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: halt-safety fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "recovered", "{json}");

    let ig = st.integrity.read().await;
    assert!(ig.halted, "retry must never clear the halt flag");
    assert!(ig.disarmed, "retry must never clear disarmed/arm state");
    drop(ig);

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// R10 — arm-before-start proof: a recovered operation still cannot start
// without a successful typed arm. Mirrors the existing D01/D02 proof
// pattern in `scenario_autonomous_daily_session_coordinator_01.rs`.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn r10_recovered_operation_still_requires_arm_before_start() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-r10-{}", unique_suffix());
    let now = dynamic_session_now();

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    let bars = expected_bar_window(now, 5);
    seed_bars_via_normal_ingestion(&pool, &bars).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some(mqk_daemon::daily_data_readiness::REASON_MARKET_DATA_MISSING), None, now,
        "test: arm-before-start fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let recovered = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(recovered.state, STATE_PREPARING_DATA);

    // Coordinator dispatch progresses preparing_data -> awaiting_open with
    // readiness now satisfied (still integrity-disarmed throughout).
    let after_preparing = mqk_daemon::state::autonomous_daily_coordinator::dispatch_by_state(
        &st,
        &pool,
        recovered,
        &plan,
        now,
    )
    .await?;
    let awaiting_open_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        awaiting_open_row.state, STATE_AWAITING_OPEN,
        "coordinator must reach awaiting_open with readiness satisfied: {after_preparing:?}"
    );

    // Negative control: attempt_canonical_start must refuse (durable arm
    // disarmed / integrity halted) and start_execution_runtime must never
    // be reached — start_attempt_count stays exactly 0.
    let outcome = attempt_canonical_start(
        &st,
        &pool,
        awaiting_open_row,
        now,
        STATE_AWAITING_OPEN,
    )
    .await?;
    assert!(
        matches!(
            outcome,
            mqk_daemon::state::autonomous_daily_coordinator::AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "disarmed/halted must refuse the start sequence, got {outcome:?}"
    );

    let final_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        final_row.start_attempt_count, 0,
        "an arm-gate refusal must never count as (or reach) a canonical start attempt"
    );

    cleanup_bars(&pool, SYMBOL, TIMEFRAME).await;
    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unsafe / administrative / unknown reason classes -> refused
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn eligibility_unsafe_runtime_history_reason_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-elig-unsafe-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("integrity_halted"), None, now, "test: unsafe-reason fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_recoverable", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn eligibility_identity_conflict_reason_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-elig-idconf-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("operation_identity_conflict"), None, now, "test: identity-conflict fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_recoverable", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn eligibility_unknown_reason_fails_closed() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-elig-unknown-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;
    real_transition(
        &pool, operation_id, STATE_PREPARING_DATA, 1, STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("totally_unrecognized_reason_code_xyz"), None, now, "test: unknown-reason fixture",
    )
    .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_recoverable", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// not_manual — operation is not currently manual_intervention_required
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn not_manual_state_refuses_retry() -> anyhow::Result<()> {
    reset_env();
    set_active_assignment_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-notmanual-{}", unique_suffix());
    let now = weekday_at(14, 30);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    active_fleet_st(&st).await;
    // The route reads `AppState::daily_data_readiness_now()`, not a raw
    // `Utc::now()` — pin it to this test's fixed fixture instant so the
    // route's own session-window/identity/readiness evaluation agrees with
    // the fixtures built against `now` below.
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_active_identity(&st, &adapter_id, now).await;

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        "PAPER",
        &assignment_identity,
        &runtime_binding_identity,
        now,
        STATE_PREPARING_DATA,
        None,
    )
    .await?;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(operation_id, None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["truth_state"], "not_manual", "{json}");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Operator auth (§30): missing token / wrong token / valid token
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn auth_missing_token_fail_closed_blocks_retry() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-auth-missing-{}", unique_suffix());
    let st = paper_state_with_db_and_auth(
        pool,
        &adapter_id,
        OperatorAuthMode::MissingTokenFailClosed,
    );

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req(Uuid::new_v4(), None),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(json["gate"], "operator_auth_config", "{json}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn auth_wrong_token_blocks_retry() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-auth-wrong-{}", unique_suffix());
    let st = paper_state_with_db_and_auth(
        pool,
        &adapter_id,
        OperatorAuthMode::TokenRequired("correct-token".to_string()),
    );

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req_authed(Uuid::new_v4(), "wrong-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["gate"], "operator_token", "{json}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn auth_valid_token_is_evaluated_not_rejected_at_auth_layer() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("retry-auth-valid-{}", unique_suffix());
    let st = paper_state_with_db_and_auth(
        pool,
        &adapter_id,
        OperatorAuthMode::TokenRequired("correct-token".to_string()),
    );

    // A valid token against a nonexistent operation_id must reach the route
    // handler (not_found), never the auth layer's 401/503.
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        retry_req_authed(Uuid::new_v4(), "correct-token"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["truth_state"], "not_found", "{json}");
    Ok(())
}
