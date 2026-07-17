//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-DURABLE-COORDINATION: durable daily
//! autonomous-operation coordination store.
//!
//! `sys_autonomous_daily_operations` is the mutable current-state row for
//! exactly one applicable autonomous PAPER daily operation per
//! `(market_date, deployment_mode, adapter_id)` daily slot (migration
//! `0048_autonomous_daily_operations.sql`). `sys_autonomous_daily_operation_events`
//! is its append-only transition history, one row per `state_version` bump.
//!
//! Foundation only: this module is not called from `session_controller.rs`,
//! `autonomous_bar_ticker.rs`, the market-data scheduler,
//! `start_execution_runtime`, any HTTP route, or any GUI component in this
//! patch.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Typed operation states
// ---------------------------------------------------------------------------

pub const STATE_AWAITING_PREOPEN: &str = "awaiting_preopen";
pub const STATE_PREPARING_DATA: &str = "preparing_data";
pub const STATE_AWAITING_OPEN: &str = "awaiting_open";
pub const STATE_PREFLIGHT_BLOCKED: &str = "preflight_blocked";
pub const STATE_START_RETRYING: &str = "start_retrying";
pub const STATE_RUNNING: &str = "running";
pub const STATE_RECOVERY_RETRYING: &str = "recovery_retrying";
pub const STATE_STOPPING: &str = "stopping";
pub const STATE_STOP_RETRYING: &str = "stop_retrying";
pub const STATE_COMPLETED: &str = "completed";
pub const STATE_COMPLETED_NO_TRADE: &str = "completed_no_trade";
pub const STATE_COMPLETED_WITH_ACTIVITY: &str = "completed_with_activity";
pub const STATE_MANUAL_INTERVENTION_REQUIRED: &str = "manual_intervention_required";
pub const STATE_CONTROLLER_DEGRADED: &str = "controller_degraded";
pub const STATE_CALENDAR_UNAVAILABLE: &str = "calendar_unavailable";
pub const STATE_EVIDENCE_DEGRADED: &str = "evidence_degraded";

/// `"none"` is the literal `from_state` for the very first transition of a
/// new operation. It is never a valid current `state` value.
pub const TRANSITION_FROM_NONE: &str = "none";

pub const ALL_OPERATION_STATES: [&str; 16] = [
    STATE_AWAITING_PREOPEN,
    STATE_PREPARING_DATA,
    STATE_AWAITING_OPEN,
    STATE_PREFLIGHT_BLOCKED,
    STATE_START_RETRYING,
    STATE_RUNNING,
    STATE_RECOVERY_RETRYING,
    STATE_STOPPING,
    STATE_STOP_RETRYING,
    STATE_COMPLETED,
    STATE_COMPLETED_NO_TRADE,
    STATE_COMPLETED_WITH_ACTIVITY,
    STATE_MANUAL_INTERVENTION_REQUIRED,
    STATE_CONTROLLER_DEGRADED,
    STATE_CALENDAR_UNAVAILABLE,
    STATE_EVIDENCE_DEGRADED,
];

/// `true` iff `state` is one of the 16 canonical operation states.
pub fn is_known_operation_state(state: &str) -> bool {
    ALL_OPERATION_STATES.contains(&state)
}

/// `true` for the three completed-* states. Terminal: no legal transition
/// leaves any of them (see [`is_legal_operation_transition`]).
pub fn is_terminal_operation_state(state: &str) -> bool {
    matches!(
        state,
        STATE_COMPLETED | STATE_COMPLETED_NO_TRADE | STATE_COMPLETED_WITH_ACTIVITY
    )
}

/// Pure mirror of the legal transition graph. `previous == None` represents
/// the literal `"none"` initial-transition marker — legal only into one of
/// the pipeline's true entry states (never directly into `running`,
/// `manual_intervention_required`, or any completed-* state: those must be
/// reached through a real recorded transition, never fabricated as an
/// operation's first state).
///
/// This is a Phase B foundation graph: it proves every representative path
/// the bundle contract requires and structurally forbids the four
/// explicitly-named illegal edges (`completed* -> running`,
/// `completed* -> start_retrying`, `running -> awaiting_preopen`,
/// `stopping -> preparing_data`). Phase C-E may extend this graph with new
/// edges between these same 16 states without a migration; this patch does
/// not attempt to anticipate every future edge.
pub fn is_legal_operation_transition(previous: Option<&str>, new_state: &str) -> bool {
    if !is_known_operation_state(new_state) {
        return false;
    }
    match previous {
        None => matches!(
            new_state,
            STATE_AWAITING_PREOPEN
                | STATE_PREPARING_DATA
                | STATE_AWAITING_OPEN
                | STATE_CALENDAR_UNAVAILABLE
        ),
        Some(STATE_AWAITING_PREOPEN) => matches!(
            new_state,
            STATE_PREPARING_DATA | STATE_CALENDAR_UNAVAILABLE | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_PREPARING_DATA) => matches!(
            new_state,
            STATE_AWAITING_OPEN
                | STATE_PREFLIGHT_BLOCKED
                | STATE_CALENDAR_UNAVAILABLE
                | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_AWAITING_OPEN) => matches!(
            new_state,
            STATE_PREFLIGHT_BLOCKED
                | STATE_START_RETRYING
                | STATE_CALENDAR_UNAVAILABLE
                | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_PREFLIGHT_BLOCKED) => matches!(
            new_state,
            STATE_START_RETRYING | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_START_RETRYING) => matches!(
            new_state,
            STATE_RUNNING | STATE_PREFLIGHT_BLOCKED | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_RUNNING) => matches!(
            new_state,
            STATE_STOPPING
                | STATE_RECOVERY_RETRYING
                | STATE_CONTROLLER_DEGRADED
                | STATE_EVIDENCE_DEGRADED
        ),
        Some(STATE_RECOVERY_RETRYING) => matches!(
            new_state,
            STATE_RUNNING | STATE_STOPPING | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_CONTROLLER_DEGRADED) => matches!(
            new_state,
            STATE_RUNNING | STATE_STOPPING | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_EVIDENCE_DEGRADED) => matches!(
            new_state,
            STATE_RUNNING | STATE_STOPPING | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_STOPPING) => matches!(
            new_state,
            STATE_COMPLETED
                | STATE_COMPLETED_NO_TRADE
                | STATE_COMPLETED_WITH_ACTIVITY
                | STATE_STOP_RETRYING
                | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_STOP_RETRYING) => matches!(
            new_state,
            STATE_COMPLETED
                | STATE_COMPLETED_NO_TRADE
                | STATE_COMPLETED_WITH_ACTIVITY
                | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        Some(STATE_MANUAL_INTERVENTION_REQUIRED) => matches!(
            new_state,
            STATE_AWAITING_PREOPEN
                | STATE_PREPARING_DATA
                | STATE_PREFLIGHT_BLOCKED
                | STATE_START_RETRYING
                | STATE_STOPPING
        ),
        Some(STATE_CALENDAR_UNAVAILABLE) => matches!(
            new_state,
            STATE_AWAITING_PREOPEN | STATE_MANUAL_INTERVENTION_REQUIRED
        ),
        // Terminal: no legal transition out of any completed-* state.
        Some(STATE_COMPLETED)
        | Some(STATE_COMPLETED_NO_TRADE)
        | Some(STATE_COMPLETED_WITH_ACTIVITY) => false,
        Some(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One row from `sys_autonomous_daily_operations`.
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-BOUNDARY-MODEL-REPAIR-01:
/// `effective_operation_open_utc`/`effective_operation_close_utc` map the
/// pre-existing, physically-unrenamed `session_open_utc`/`session_close_utc`
/// columns (migration 0048) -- the effective autonomous-operation window.
/// `exchange_session_open_utc`/`exchange_session_close_utc`/
/// `exchange_is_early_close`/`previous_trading_date` are the separate,
/// nullable exchange-calendar-truth columns added by migration 0049; `None`
/// only for a legacy row created before that migration, never fabricated
/// from the effective operation fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AutonomousDailyOperationRecord {
    pub operation_id: Uuid,
    pub market_date: NaiveDate,
    pub deployment_mode: String,
    pub adapter_id: String,
    pub session_plan_identity: String,
    pub assignment_identity: String,
    pub runtime_binding_identity: String,
    pub calendar_source: String,
    pub calendar_coverage_state: String,
    pub schedule_source: String,
    pub effective_operation_open_utc: DateTime<Utc>,
    pub effective_operation_close_utc: DateTime<Utc>,
    pub exchange_session_open_utc: Option<DateTime<Utc>>,
    pub exchange_session_close_utc: Option<DateTime<Utc>>,
    pub exchange_is_early_close: Option<bool>,
    pub previous_trading_date: Option<NaiveDate>,
    pub preopen_start_utc: DateTime<Utc>,
    pub postclose_finalize_utc: DateTime<Utc>,
    pub state: String,
    pub state_reason_code: Option<String>,
    pub state_version: i64,
    pub run_id: Option<Uuid>,
    pub start_attempt_count: i64,
    pub last_start_attempt_utc: Option<DateTime<Utc>>,
    pub next_retry_utc: Option<DateTime<Utc>>,
    pub data_refresh_state: String,
    pub last_provider_poll_utc: Option<DateTime<Utc>>,
    pub provider_poll_attempt_count: i64,
    pub provider_poll_success_count: i64,
    pub provider_poll_failure_count: i64,
    pub last_completed_bar_ts: Option<i64>,
    pub last_dispatched_bar_ts: Option<i64>,
    pub bars_observed: i64,
    pub bars_dispatched: i64,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub stopped_at_utc: Option<DateTime<Utc>>,
    pub finalized_at_utc: Option<DateTime<Utc>>,
    pub outcome: Option<String>,
    pub no_trade_reason: Option<String>,
    pub last_error: Option<String>,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
}

/// One row from `sys_autonomous_daily_operation_events`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutonomousDailyOperationEventRecord {
    pub operation_id: Uuid,
    pub transition_seq: i64,
    pub from_state: String,
    pub to_state: String,
    pub reason_code: Option<String>,
    pub occurred_at_utc: DateTime<Utc>,
    pub run_id: Option<Uuid>,
    pub bounded_detail: String,
}

const OPERATION_COLUMNS: &str = r#"
    operation_id, market_date, deployment_mode, adapter_id,
    session_plan_identity, assignment_identity, runtime_binding_identity,
    calendar_source, calendar_coverage_state, schedule_source,
    session_open_utc, session_close_utc, preopen_start_utc, postclose_finalize_utc,
    state, state_reason_code, state_version,
    run_id, start_attempt_count, last_start_attempt_utc, next_retry_utc,
    data_refresh_state, last_provider_poll_utc,
    provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count,
    last_completed_bar_ts, last_dispatched_bar_ts, bars_observed, bars_dispatched,
    started_at_utc, stopped_at_utc, finalized_at_utc,
    outcome, no_trade_reason, last_error,
    created_at_utc, updated_at_utc,
    exchange_session_open_utc, exchange_session_close_utc, exchange_is_early_close,
    previous_trading_date
"#;

const EVENT_COLUMNS: &str = r#"
    operation_id, transition_seq, from_state, to_state, reason_code,
    occurred_at_utc, run_id, bounded_detail
"#;

fn row_to_operation_record(
    r: sqlx::postgres::PgRow,
) -> Result<AutonomousDailyOperationRecord, sqlx::Error> {
    Ok(AutonomousDailyOperationRecord {
        operation_id: r.try_get("operation_id")?,
        market_date: r.try_get("market_date")?,
        deployment_mode: r.try_get("deployment_mode")?,
        adapter_id: r.try_get("adapter_id")?,
        session_plan_identity: r.try_get("session_plan_identity")?,
        assignment_identity: r.try_get("assignment_identity")?,
        runtime_binding_identity: r.try_get("runtime_binding_identity")?,
        calendar_source: r.try_get("calendar_source")?,
        calendar_coverage_state: r.try_get("calendar_coverage_state")?,
        schedule_source: r.try_get("schedule_source")?,
        effective_operation_open_utc: r.try_get("session_open_utc")?,
        effective_operation_close_utc: r.try_get("session_close_utc")?,
        exchange_session_open_utc: r.try_get("exchange_session_open_utc")?,
        exchange_session_close_utc: r.try_get("exchange_session_close_utc")?,
        exchange_is_early_close: r.try_get("exchange_is_early_close")?,
        previous_trading_date: r.try_get("previous_trading_date")?,
        preopen_start_utc: r.try_get("preopen_start_utc")?,
        postclose_finalize_utc: r.try_get("postclose_finalize_utc")?,
        state: r.try_get("state")?,
        state_reason_code: r.try_get("state_reason_code")?,
        state_version: r.try_get("state_version")?,
        run_id: r.try_get("run_id")?,
        start_attempt_count: r.try_get("start_attempt_count")?,
        last_start_attempt_utc: r.try_get("last_start_attempt_utc")?,
        next_retry_utc: r.try_get("next_retry_utc")?,
        data_refresh_state: r.try_get("data_refresh_state")?,
        last_provider_poll_utc: r.try_get("last_provider_poll_utc")?,
        provider_poll_attempt_count: r.try_get("provider_poll_attempt_count")?,
        provider_poll_success_count: r.try_get("provider_poll_success_count")?,
        provider_poll_failure_count: r.try_get("provider_poll_failure_count")?,
        last_completed_bar_ts: r.try_get("last_completed_bar_ts")?,
        last_dispatched_bar_ts: r.try_get("last_dispatched_bar_ts")?,
        bars_observed: r.try_get("bars_observed")?,
        bars_dispatched: r.try_get("bars_dispatched")?,
        started_at_utc: r.try_get("started_at_utc")?,
        stopped_at_utc: r.try_get("stopped_at_utc")?,
        finalized_at_utc: r.try_get("finalized_at_utc")?,
        outcome: r.try_get("outcome")?,
        no_trade_reason: r.try_get("no_trade_reason")?,
        last_error: r.try_get("last_error")?,
        created_at_utc: r.try_get("created_at_utc")?,
        updated_at_utc: r.try_get("updated_at_utc")?,
    })
}

fn row_to_event_record(
    r: sqlx::postgres::PgRow,
) -> Result<AutonomousDailyOperationEventRecord, sqlx::Error> {
    Ok(AutonomousDailyOperationEventRecord {
        operation_id: r.try_get("operation_id")?,
        transition_seq: r.try_get("transition_seq")?,
        from_state: r.try_get("from_state")?,
        to_state: r.try_get("to_state")?,
        reason_code: r.try_get("reason_code")?,
        occurred_at_utc: r.try_get("occurred_at_utc")?,
        run_id: r.try_get("run_id")?,
        bounded_detail: r.try_get("bounded_detail")?,
    })
}

// ---------------------------------------------------------------------------
// Create-or-recover
// ---------------------------------------------------------------------------

/// Arguments for [`create_or_recover_autonomous_daily_operation`]. Every
/// value is caller-supplied; the store adds no SQL default for any not-null
/// column. Bookkeeping counters that are definitionally zero for a brand new
/// operation (`start_attempt_count`, `provider_poll_*_count`,
/// `bars_observed`, `bars_dispatched`) are fixed to `0` by the store itself
/// as an honest Rust-level value bound into the INSERT — never a
/// database-level `DEFAULT`.
#[derive(Debug, Clone)]
pub struct CreateAutonomousDailyOperationArgs {
    pub operation_id: Uuid,
    pub market_date: NaiveDate,
    pub deployment_mode: String,
    pub adapter_id: String,
    pub session_plan_identity: String,
    pub assignment_identity: String,
    pub runtime_binding_identity: String,
    pub calendar_source: String,
    pub calendar_coverage_state: String,
    pub schedule_source: String,
    pub effective_operation_open_utc: DateTime<Utc>,
    pub effective_operation_close_utc: DateTime<Utc>,
    /// Authoritative exchange-calendar session open. Every caller through
    /// this API must supply real exchange truth -- never a copy of
    /// `effective_operation_open_utc` (that would fabricate calendar truth
    /// for a fixed-window-override operation).
    pub exchange_session_open_utc: DateTime<Utc>,
    pub exchange_session_close_utc: DateTime<Utc>,
    pub exchange_is_early_close: bool,
    pub previous_trading_date: NaiveDate,
    pub preopen_start_utc: DateTime<Utc>,
    pub postclose_finalize_utc: DateTime<Utc>,
    pub initial_state: String,
    pub data_refresh_state: String,
    /// Used as `created_at_utc`, `updated_at_utc`, and the initial
    /// transition event's `occurred_at_utc` — one caller-supplied instant
    /// for the entire creation, never three independently-derived clocks.
    pub occurred_at_utc: DateTime<Utc>,
    /// Bounded free text for the initial `none -> initial_state` event.
    pub bounded_detail: String,
}

/// Outcome of [`create_or_recover_autonomous_daily_operation`].
#[derive(Debug, Clone)]
pub enum CreateOrRecoverAutonomousDailyOperationOutcome {
    /// A new row (and its initial transition event) was inserted.
    Created(AutonomousDailyOperationRecord),
    /// The daily slot already held a row whose immutable identity fields
    /// (`session_plan_identity`, `assignment_identity`,
    /// `runtime_binding_identity`) exactly match the freshly computed
    /// values — the ordinary same-day daemon-restart case. No row was
    /// mutated, no event was inserted.
    Recovered(AutonomousDailyOperationRecord),
    /// The daily slot already held a row, but at least one immutable
    /// identity field differs from the freshly computed value (e.g. an
    /// operator changed strategy/symbol/timeframe config and restarted
    /// mid-day). No second row was created; the existing row's identity
    /// fields were not rewritten; no event was inserted.
    IdentityConflict {
        existing_operation_id: Uuid,
        expected_operation_id: Uuid,
        differing_fields: Vec<String>,
    },
}

fn identity_slot_lock_key(
    market_date: NaiveDate,
    deployment_mode: &str,
    adapter_id: &str,
) -> String {
    format!(
        "mqk-autonomous-daily-operation-slot.v1|{}|{}|{}",
        market_date,
        deployment_mode.trim(),
        adapter_id.trim()
    )
}

fn validate_create_args(args: &CreateAutonomousDailyOperationArgs) -> Result<()> {
    if args.deployment_mode.trim().is_empty() {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: deployment_mode must not be empty"
        );
    }
    if args.adapter_id.trim().is_empty() {
        anyhow::bail!("create_or_recover_autonomous_daily_operation: adapter_id must not be empty");
    }
    if args.session_plan_identity.trim().is_empty() {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: session_plan_identity must not be empty"
        );
    }
    if args.assignment_identity.trim().is_empty() {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: assignment_identity must not be empty"
        );
    }
    if args.runtime_binding_identity.trim().is_empty() {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: runtime_binding_identity must not be empty"
        );
    }
    if args.exchange_session_open_utc >= args.exchange_session_close_utc {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: exchange_session_open_utc must precede exchange_session_close_utc"
        );
    }
    if args.effective_operation_open_utc >= args.effective_operation_close_utc {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: effective_operation_open_utc must precede effective_operation_close_utc"
        );
    }
    if args.preopen_start_utc > args.effective_operation_open_utc {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: preopen_start_utc must not be after effective_operation_open_utc"
        );
    }
    if args.postclose_finalize_utc < args.effective_operation_close_utc {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: postclose_finalize_utc must not be before effective_operation_close_utc"
        );
    }
    if args.previous_trading_date >= args.market_date {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: previous_trading_date must precede market_date"
        );
    }
    if !is_known_operation_state(&args.initial_state) {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: unknown initial_state '{}'",
            args.initial_state
        );
    }
    if !is_legal_operation_transition(None, &args.initial_state) {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: '{}' is not a legal initial state",
            args.initial_state
        );
    }
    if args.data_refresh_state.trim().is_empty() {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: data_refresh_state must not be empty"
        );
    }
    if args.bounded_detail.len() > 4000 {
        anyhow::bail!(
            "create_or_recover_autonomous_daily_operation: bounded_detail exceeds 4000 chars"
        );
    }
    Ok(())
}

/// Race-safe create-or-recover for one daily slot
/// (`market_date`, `deployment_mode`, `adapter_id`).
///
/// Serializes concurrent attempts for the *same* slot with a
/// transaction-scoped `pg_advisory_xact_lock` (mirrors
/// `mqk_db::insert_strategy_promotion_transition_serialized`), so two
/// concurrent identical create calls always produce exactly one `Created`
/// and one `Recovered`, one operation row, and one initial event — never two
/// rows or two initial events.
pub async fn create_or_recover_autonomous_daily_operation(
    pool: &PgPool,
    args: &CreateAutonomousDailyOperationArgs,
) -> Result<CreateOrRecoverAutonomousDailyOperationOutcome> {
    validate_create_args(args)?;

    let mut tx = pool
        .begin()
        .await
        .context("create_or_recover_autonomous_daily_operation: begin transaction failed")?;

    let lock_key =
        identity_slot_lock_key(args.market_date, &args.deployment_mode, &args.adapter_id);
    sqlx::query("select pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(&lock_key)
        .execute(&mut *tx)
        .await
        .context("create_or_recover_autonomous_daily_operation: advisory lock failed")?;

    let existing_row = sqlx::query(&format!(
        "select {OPERATION_COLUMNS} from sys_autonomous_daily_operations \
         where market_date = $1 and deployment_mode = $2 and adapter_id = $3"
    ))
    .bind(args.market_date)
    .bind(&args.deployment_mode)
    .bind(&args.adapter_id)
    .fetch_optional(&mut *tx)
    .await
    .context("create_or_recover_autonomous_daily_operation: slot read failed")?;

    match existing_row {
        None => {
            sqlx::query(&format!(
                r#"
                insert into sys_autonomous_daily_operations ({OPERATION_COLUMNS})
                values (
                    $1,$2,$3,$4,
                    $5,$6,$7,
                    $8,$9,$10,
                    $11,$12,$13,$14,
                    $15,$16,$17,
                    $18,$19,$20,$21,
                    $22,$23,
                    $24,$25,$26,
                    $27,$28,$29,$30,
                    $31,$32,$33,
                    $34,$35,$36,
                    $37,$38,
                    $39,$40,$41,$42
                )
                "#
            ))
            .bind(args.operation_id)
            .bind(args.market_date)
            .bind(&args.deployment_mode)
            .bind(&args.adapter_id)
            .bind(&args.session_plan_identity)
            .bind(&args.assignment_identity)
            .bind(&args.runtime_binding_identity)
            .bind(&args.calendar_source)
            .bind(&args.calendar_coverage_state)
            .bind(&args.schedule_source)
            .bind(args.effective_operation_open_utc)
            .bind(args.effective_operation_close_utc)
            .bind(args.preopen_start_utc)
            .bind(args.postclose_finalize_utc)
            .bind(&args.initial_state)
            .bind(None::<String>) // state_reason_code
            .bind(1i64) // state_version
            .bind(None::<Uuid>) // run_id
            .bind(0i64) // start_attempt_count
            .bind(None::<DateTime<Utc>>) // last_start_attempt_utc
            .bind(None::<DateTime<Utc>>) // next_retry_utc
            .bind(&args.data_refresh_state)
            .bind(None::<DateTime<Utc>>) // last_provider_poll_utc
            .bind(0i64) // provider_poll_attempt_count
            .bind(0i64) // provider_poll_success_count
            .bind(0i64) // provider_poll_failure_count
            .bind(None::<i64>) // last_completed_bar_ts
            .bind(None::<i64>) // last_dispatched_bar_ts
            .bind(0i64) // bars_observed
            .bind(0i64) // bars_dispatched
            .bind(None::<DateTime<Utc>>) // started_at_utc
            .bind(None::<DateTime<Utc>>) // stopped_at_utc
            .bind(None::<DateTime<Utc>>) // finalized_at_utc
            .bind(None::<String>) // outcome
            .bind(None::<String>) // no_trade_reason
            .bind(None::<String>) // last_error
            .bind(args.occurred_at_utc) // created_at_utc
            .bind(args.occurred_at_utc) // updated_at_utc
            .bind(args.exchange_session_open_utc)
            .bind(args.exchange_session_close_utc)
            .bind(args.exchange_is_early_close)
            .bind(args.previous_trading_date)
            .execute(&mut *tx)
            .await
            .context("create_or_recover_autonomous_daily_operation: insert operation row failed")?;

            sqlx::query(&format!(
                r#"
                insert into sys_autonomous_daily_operation_events ({EVENT_COLUMNS})
                values ($1,$2,$3,$4,$5,$6,$7,$8)
                "#
            ))
            .bind(args.operation_id)
            .bind(1i64)
            .bind(TRANSITION_FROM_NONE)
            .bind(&args.initial_state)
            .bind(None::<String>)
            .bind(args.occurred_at_utc)
            .bind(None::<Uuid>)
            .bind(&args.bounded_detail)
            .execute(&mut *tx)
            .await
            .context("create_or_recover_autonomous_daily_operation: insert initial event failed")?;

            tx.commit()
                .await
                .context("create_or_recover_autonomous_daily_operation: commit (created) failed")?;

            Ok(CreateOrRecoverAutonomousDailyOperationOutcome::Created(
                AutonomousDailyOperationRecord {
                    operation_id: args.operation_id,
                    market_date: args.market_date,
                    deployment_mode: args.deployment_mode.clone(),
                    adapter_id: args.adapter_id.clone(),
                    session_plan_identity: args.session_plan_identity.clone(),
                    assignment_identity: args.assignment_identity.clone(),
                    runtime_binding_identity: args.runtime_binding_identity.clone(),
                    calendar_source: args.calendar_source.clone(),
                    calendar_coverage_state: args.calendar_coverage_state.clone(),
                    schedule_source: args.schedule_source.clone(),
                    effective_operation_open_utc: args.effective_operation_open_utc,
                    effective_operation_close_utc: args.effective_operation_close_utc,
                    exchange_session_open_utc: Some(args.exchange_session_open_utc),
                    exchange_session_close_utc: Some(args.exchange_session_close_utc),
                    exchange_is_early_close: Some(args.exchange_is_early_close),
                    previous_trading_date: Some(args.previous_trading_date),
                    preopen_start_utc: args.preopen_start_utc,
                    postclose_finalize_utc: args.postclose_finalize_utc,
                    state: args.initial_state.clone(),
                    state_reason_code: None,
                    state_version: 1,
                    run_id: None,
                    start_attempt_count: 0,
                    last_start_attempt_utc: None,
                    next_retry_utc: None,
                    data_refresh_state: args.data_refresh_state.clone(),
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
                    created_at_utc: args.occurred_at_utc,
                    updated_at_utc: args.occurred_at_utc,
                },
            ))
        }
        Some(row) => {
            let existing = row_to_operation_record(row)?;
            let mut differing_fields = Vec::new();
            if existing.session_plan_identity != args.session_plan_identity {
                differing_fields.push("session_plan_identity".to_string());
            }
            if existing.assignment_identity != args.assignment_identity {
                differing_fields.push("assignment_identity".to_string());
            }
            if existing.runtime_binding_identity != args.runtime_binding_identity {
                differing_fields.push("runtime_binding_identity".to_string());
            }

            if differing_fields.is_empty() {
                tx.commit().await.context(
                    "create_or_recover_autonomous_daily_operation: commit (recovered) failed",
                )?;
                Ok(CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(
                    existing,
                ))
            } else {
                let existing_operation_id = existing.operation_id;
                tx.rollback().await.context(
                    "create_or_recover_autonomous_daily_operation: rollback (identity conflict) failed",
                )?;
                Ok(
                    CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
                        existing_operation_id,
                        expected_operation_id: args.operation_id,
                        differing_fields,
                    },
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CAS transition
// ---------------------------------------------------------------------------

/// Arguments for [`transition_autonomous_daily_operation`].
#[derive(Debug, Clone)]
pub struct TransitionAutonomousDailyOperationArgs {
    pub operation_id: Uuid,
    pub expected_state: String,
    pub expected_state_version: i64,
    pub new_state: String,
    pub reason_code: Option<String>,
    pub occurred_at_utc: DateTime<Utc>,
    pub run_id: Option<Uuid>,
    pub bounded_detail: String,
}

/// Outcome of [`transition_autonomous_daily_operation`].
#[derive(Debug, Clone)]
pub enum AutonomousDailyTransitionOutcome {
    /// The transition was applied: `state_version` incremented by exactly
    /// one, and a matching event row was inserted at
    /// `transition_seq == new state_version`, atomically.
    Applied(AutonomousDailyOperationRecord),
    /// The exact same transition (`expected_state -> new_state` at the
    /// resulting `transition_seq`) was already applied — an idempotent
    /// retry. No second event was inserted, no double-increment occurred.
    AlreadyApplied(AutonomousDailyOperationRecord),
    /// `operation_id` exists, but its actual current `(state, state_version)`
    /// does not match `(expected_state, expected_state_version)` and this is
    /// not an exact replay of an already-applied transition. Nothing was
    /// written.
    StaleState {
        actual_state: String,
        actual_state_version: i64,
    },
    /// No row exists for `operation_id`.
    NotFound,
    /// `expected_state -> new_state` is not a legal edge in
    /// [`is_legal_operation_transition`]. Checked before any DB write.
    IllegalTransition,
}

fn validate_transition_args(args: &TransitionAutonomousDailyOperationArgs) -> Result<()> {
    if !is_known_operation_state(&args.expected_state) {
        anyhow::bail!(
            "transition_autonomous_daily_operation: unknown expected_state '{}'",
            args.expected_state
        );
    }
    if !is_known_operation_state(&args.new_state) {
        anyhow::bail!(
            "transition_autonomous_daily_operation: unknown new_state '{}'",
            args.new_state
        );
    }
    if args.expected_state_version < 1 {
        anyhow::bail!("transition_autonomous_daily_operation: expected_state_version must be >= 1");
    }
    if args.bounded_detail.len() > 4000 {
        anyhow::bail!("transition_autonomous_daily_operation: bounded_detail exceeds 4000 chars");
    }
    if let Some(reason) = &args.reason_code {
        if reason.len() > 128 {
            anyhow::bail!("transition_autonomous_daily_operation: reason_code exceeds 128 chars");
        }
    }
    Ok(())
}

/// Atomically validate-and-apply one CAS state transition.
///
/// 1. Validates the transition is legal (pure, no DB) before touching the
///    database — an illegal transition never reaches SQL.
/// 2. Guards the `UPDATE` by `operation_id`, `expected_state`, and
///    `expected_state_version` together (Postgres row-level locking makes
///    this atomic against concurrent transitions on the same row without a
///    separate advisory lock).
/// 3. On a match, increments `state_version` by exactly one and inserts the
///    matching event row using the new version as `transition_seq`, in the
///    same transaction.
/// 4. On no match, re-reads the actual current row (read-only) to
///    distinguish `NotFound`, `AlreadyApplied` (an exact retry of a
///    transition that already succeeded), and `StaleState` (anything else —
///    wrong expected state, or a version behind current for a different
///    reason).
pub async fn transition_autonomous_daily_operation(
    pool: &PgPool,
    args: &TransitionAutonomousDailyOperationArgs,
) -> Result<AutonomousDailyTransitionOutcome> {
    validate_transition_args(args)?;

    if !is_legal_operation_transition(Some(&args.expected_state), &args.new_state) {
        return Ok(AutonomousDailyTransitionOutcome::IllegalTransition);
    }

    let mut tx = pool
        .begin()
        .await
        .context("transition_autonomous_daily_operation: begin transaction failed")?;

    let new_version = args.expected_state_version + 1;

    let updated_row = sqlx::query(&format!(
        r#"
        update sys_autonomous_daily_operations
        set state = $1,
            state_reason_code = $2,
            state_version = $3,
            run_id = coalesce($4, run_id),
            updated_at_utc = $5
        where operation_id = $6 and state = $7 and state_version = $8
        returning {OPERATION_COLUMNS}
        "#
    ))
    .bind(&args.new_state)
    .bind(&args.reason_code)
    .bind(new_version)
    .bind(args.run_id)
    .bind(args.occurred_at_utc)
    .bind(args.operation_id)
    .bind(&args.expected_state)
    .bind(args.expected_state_version)
    .fetch_optional(&mut *tx)
    .await
    .context("transition_autonomous_daily_operation: CAS update failed")?;

    if let Some(row) = updated_row {
        let record = row_to_operation_record(row)?;

        sqlx::query(&format!(
            r#"
            insert into sys_autonomous_daily_operation_events ({EVENT_COLUMNS})
            values ($1,$2,$3,$4,$5,$6,$7,$8)
            "#
        ))
        .bind(args.operation_id)
        .bind(new_version)
        .bind(&args.expected_state)
        .bind(&args.new_state)
        .bind(&args.reason_code)
        .bind(args.occurred_at_utc)
        .bind(args.run_id)
        .bind(&args.bounded_detail)
        .execute(&mut *tx)
        .await
        .context("transition_autonomous_daily_operation: insert event failed")?;

        tx.commit()
            .await
            .context("transition_autonomous_daily_operation: commit (applied) failed")?;

        return Ok(AutonomousDailyTransitionOutcome::Applied(record));
    }

    // No row matched the CAS guard: re-read the actual current row,
    // read-only, to classify NotFound / AlreadyApplied / StaleState.
    let current_row = sqlx::query(&format!(
        "select {OPERATION_COLUMNS} from sys_autonomous_daily_operations where operation_id = $1"
    ))
    .bind(args.operation_id)
    .fetch_optional(&mut *tx)
    .await
    .context("transition_autonomous_daily_operation: current-row re-read failed")?;

    let outcome = match current_row {
        None => AutonomousDailyTransitionOutcome::NotFound,
        Some(row) => {
            let current = row_to_operation_record(row)?;
            if current.state == args.new_state && current.state_version == new_version {
                let event_row = sqlx::query(
                    "select from_state, to_state from sys_autonomous_daily_operation_events \
                     where operation_id = $1 and transition_seq = $2",
                )
                .bind(args.operation_id)
                .bind(new_version)
                .fetch_optional(&mut *tx)
                .await
                .context("transition_autonomous_daily_operation: replay event lookup failed")?;

                let is_exact_replay = event_row
                    .map(|r| {
                        let from_state: String = r.try_get("from_state").unwrap_or_default();
                        let to_state: String = r.try_get("to_state").unwrap_or_default();
                        from_state == args.expected_state && to_state == args.new_state
                    })
                    .unwrap_or(false);

                if is_exact_replay {
                    AutonomousDailyTransitionOutcome::AlreadyApplied(current)
                } else {
                    AutonomousDailyTransitionOutcome::StaleState {
                        actual_state: current.state,
                        actual_state_version: current.state_version,
                    }
                }
            } else {
                AutonomousDailyTransitionOutcome::StaleState {
                    actual_state: current.state,
                    actual_state_version: current.state_version,
                }
            }
        }
    };

    tx.rollback()
        .await
        .context("transition_autonomous_daily_operation: rollback (no-op read) failed")?;

    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Reads (bounded, read-only)
// ---------------------------------------------------------------------------

/// Fetch one operation by its exact `operation_id`. `Ok(None)` is
/// authoritative "no such operation", never a synthesized row.
pub async fn fetch_autonomous_daily_operation_by_id(
    pool: &PgPool,
    operation_id: Uuid,
) -> Result<Option<AutonomousDailyOperationRecord>> {
    let row = sqlx::query(&format!(
        "select {OPERATION_COLUMNS} from sys_autonomous_daily_operations where operation_id = $1"
    ))
    .bind(operation_id)
    .fetch_optional(pool)
    .await
    .context("fetch_autonomous_daily_operation_by_id failed")?;
    row.map(row_to_operation_record)
        .transpose()
        .map_err(Into::into)
}

/// Fetch the operation occupying one daily slot
/// (`market_date`, `deployment_mode`, `adapter_id`). `Ok(None)` is
/// authoritative "no operation yet for this slot".
pub async fn fetch_autonomous_daily_operation_for_slot(
    pool: &PgPool,
    market_date: NaiveDate,
    deployment_mode: &str,
    adapter_id: &str,
) -> Result<Option<AutonomousDailyOperationRecord>> {
    let row = sqlx::query(&format!(
        "select {OPERATION_COLUMNS} from sys_autonomous_daily_operations \
         where market_date = $1 and deployment_mode = $2 and adapter_id = $3"
    ))
    .bind(market_date)
    .bind(deployment_mode)
    .bind(adapter_id)
    .fetch_optional(pool)
    .await
    .context("fetch_autonomous_daily_operation_for_slot failed")?;
    row.map(row_to_operation_record)
        .transpose()
        .map_err(Into::into)
}

/// Clamp a caller-supplied list limit to `[1, 100]`. `<= 0` clamps to `1`.
fn bounded_limit(limit: i64) -> i64 {
    limit.clamp(1, 100)
}

/// List recent operations across every daily slot, most recent first
/// (`market_date desc, created_at_utc desc, operation_id desc`). An empty
/// `Vec` is authoritative "no operation has ever been recorded", not
/// "unavailable". `limit` is clamped to `[1, 100]`; callers wanting the
/// documented default should pass `20`.
pub async fn list_recent_autonomous_daily_operations(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AutonomousDailyOperationRecord>> {
    let rows = sqlx::query(&format!(
        r#"
        select {OPERATION_COLUMNS}
        from sys_autonomous_daily_operations
        order by market_date desc, created_at_utc desc, operation_id desc
        limit $1
        "#
    ))
    .bind(bounded_limit(limit))
    .fetch_all(pool)
    .await
    .context("list_recent_autonomous_daily_operations failed")?;

    rows.into_iter()
        .map(row_to_operation_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// List one operation's transition history, ascending by `transition_seq`.
/// An empty `Vec` would indicate a data-integrity defect (every operation
/// always has at least its initial event) — never treated as "history
/// unavailable". `limit` is clamped to `[1, 100]`; callers wanting the
/// documented default should pass `20`.
pub async fn list_autonomous_daily_operation_events(
    pool: &PgPool,
    operation_id: Uuid,
    limit: i64,
) -> Result<Vec<AutonomousDailyOperationEventRecord>> {
    let rows = sqlx::query(&format!(
        r#"
        select {EVENT_COLUMNS}
        from sys_autonomous_daily_operation_events
        where operation_id = $1
        order by transition_seq asc
        limit $2
        "#
    ))
    .bind(operation_id)
    .bind(bounded_limit(limit))
    .fetch_all(pool)
    .await
    .context("list_autonomous_daily_operation_events failed")?;

    rows.into_iter()
        .map(row_to_event_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}
