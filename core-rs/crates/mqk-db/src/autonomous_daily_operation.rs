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
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR: D2
/// extends five edges the Phase B graph did not need, all of the same
/// shape (`<pre-running state> -> stopping`): `awaiting_preopen`,
/// `preparing_data`, `awaiting_open`, `preflight_blocked`, and
/// `start_retrying` may each transition directly to `stopping`. This closes
/// a genuine gap the original graph left open: before D2, no pre-running
/// state could reach `stopping` at all (only `running` /
/// `recovery_retrying` / `controller_degraded` / `evidence_degraded`
/// could), so a coordinator that first observes an applicable operation
/// already past its `effective_operation_close_utc` -- having never
/// started a runtime -- had no legal, honest state to record. D2 records
/// this truthfully as `stopping` with `stopped_at_utc` set and detail `no
/// autonomous runtime existed at session close` (per the binding contract's
/// D2.17), never by inventing a `running` step that never happened.
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
            STATE_PREPARING_DATA
                | STATE_CALENDAR_UNAVAILABLE
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | STATE_STOPPING
        ),
        Some(STATE_PREPARING_DATA) => matches!(
            new_state,
            STATE_AWAITING_OPEN
                | STATE_PREFLIGHT_BLOCKED
                | STATE_CALENDAR_UNAVAILABLE
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | STATE_STOPPING
        ),
        Some(STATE_AWAITING_OPEN) => matches!(
            new_state,
            STATE_PREFLIGHT_BLOCKED
                | STATE_START_RETRYING
                | STATE_CALENDAR_UNAVAILABLE
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | STATE_STOPPING
        ),
        Some(STATE_PREFLIGHT_BLOCKED) => matches!(
            new_state,
            STATE_START_RETRYING | STATE_MANUAL_INTERVENTION_REQUIRED | STATE_STOPPING
        ),
        Some(STATE_START_RETRYING) => matches!(
            new_state,
            STATE_RUNNING
                | STATE_PREFLIGHT_BLOCKED
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | STATE_STOPPING
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
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01:
    /// bounded D1 typed blocker signature (`"sha256:<64 hex>"`) for
    /// `state_reason_code`, present only when the current state was reached
    /// via a blocked/manual transition. `None` for a legacy row created
    /// before migration `0052`, for a row never blocked, and cleared by any
    /// non-blocked forward transition. Dedup authority for manual/blocked
    /// truth is `(state, state_reason_code, state_blocker_signature)`
    /// together -- never `state_reason_code` alone.
    pub state_blocker_signature: Option<String>,
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
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: count of canonical
    /// `stop_execution_runtime` call attempts. `None` only for a legacy row
    /// created before migration `0051` -- never fabricated as zero. Every
    /// row created through this store's create API after `0051` supplies an
    /// explicit `0`.
    pub stop_attempt_count: Option<i64>,
    pub last_stop_attempt_utc: Option<DateTime<Utc>>,
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
    previous_trading_date,
    stop_attempt_count, last_stop_attempt_utc,
    state_blocker_signature
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
        state_blocker_signature: r.try_get("state_blocker_signature")?,
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
        stop_attempt_count: r.try_get("stop_attempt_count")?,
        last_stop_attempt_utc: r.try_get("last_stop_attempt_utc")?,
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
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: explicit `0` for every new
    /// row -- no SQL `DEFAULT` fabricates it. Legacy rows created before
    /// migration `0051` remain `None`/null, never backfilled.
    pub stop_attempt_count: i64,
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
                    $39,$40,$41,$42,
                    $43,$44,
                    $45
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
            .bind(args.stop_attempt_count)
            .bind(None::<DateTime<Utc>>) // last_stop_attempt_utc
            .bind(None::<String>) // state_blocker_signature
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
                    state_blocker_signature: None,
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
                    stop_attempt_count: Some(args.stop_attempt_count),
                    last_stop_attempt_utc: None,
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
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01:
    /// the D1 typed blocker signature paired with `reason_code`, or `None`
    /// for a non-blocked transition. Always overwrites the row's
    /// `state_blocker_signature` (never `coalesce`d) so a non-blocked
    /// forward transition truthfully clears a stale signature from a prior
    /// blocked state.
    pub blocker_signature: Option<String>,
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
    if let Some(sig) = &args.blocker_signature {
        if sig.len() > 128 {
            anyhow::bail!(
                "transition_autonomous_daily_operation: blocker_signature exceeds 128 chars"
            );
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
            state_blocker_signature = $3,
            state_version = $4,
            run_id = coalesce($5, run_id),
            updated_at_utc = $6
        where operation_id = $7 and state = $8 and state_version = $9
        returning {OPERATION_COLUMNS}
        "#
    ))
    .bind(&args.new_state)
    .bind(&args.reason_code)
    .bind(&args.blocker_signature)
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
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01
// Atomic running transition
// ---------------------------------------------------------------------------

/// Arguments for [`transition_autonomous_daily_operation_to_running`].
#[derive(Debug, Clone)]
pub struct TransitionAutonomousDailyOperationToRunningArgs {
    pub operation_id: Uuid,
    pub expected_state: String,
    pub expected_state_version: i64,
    /// The exact new run_id bound atomically with the running transition —
    /// never `coalesce`d, always overwrites (a running transition always
    /// carries the run the canonical start call just confirmed).
    pub run_id: Uuid,
    pub started_at_utc: DateTime<Utc>,
    pub occurred_at_utc: DateTime<Utc>,
    pub bounded_detail: String,
}

fn validate_transition_to_running_args(
    args: &TransitionAutonomousDailyOperationToRunningArgs,
) -> Result<()> {
    if !is_known_operation_state(&args.expected_state) {
        anyhow::bail!(
            "transition_autonomous_daily_operation_to_running: unknown expected_state '{}'",
            args.expected_state
        );
    }
    if args.expected_state_version < 1 {
        anyhow::bail!(
            "transition_autonomous_daily_operation_to_running: expected_state_version must be >= 1"
        );
    }
    if args.bounded_detail.len() > 4000 {
        anyhow::bail!(
            "transition_autonomous_daily_operation_to_running: bounded_detail exceeds 4000 chars"
        );
    }
    Ok(())
}

/// Atomically transition one operation directly into `running`, binding the
/// exact new `run_id` and `started_at_utc` in the same CAS update as the
/// state/version/retry-clear/event write.
///
/// Closes the D2 split-sequence defect: the coordinator previously called
/// [`transition_autonomous_daily_operation`] (state -> running, run_id) and
/// then separately called [`record_running_started`] (started_at_utc) as two
/// independent writes — a crash between them could leave a `running`
/// operation with no `started_at_utc`, or (in principle) diverge the two
/// writes' outcomes. This function performs, in one transaction:
///
/// 1. CAS-guard on `(operation_id, expected_state, expected_state_version)`,
///    exactly like [`transition_autonomous_daily_operation`].
/// 2. `is_legal_operation_transition(Some(expected_state), STATE_RUNNING)`
///    check before touching SQL — only `start_retrying`,
///    `recovery_retrying`, `controller_degraded`, and `evidence_degraded`
///    are legal sources into `running` today.
/// 3. On match: set `state = running`, clear `state_reason_code` and
///    `state_blocker_signature`, increment `state_version` by exactly one,
///    set `run_id` to the caller-supplied value (never `coalesce`d), advance
///    `started_at_utc` only if it was previously null (idempotent, never
///    rewinds), clear `next_retry_utc` and `last_error`, and insert exactly
///    one matching transition event — all in the same transaction.
/// 4. On no match: re-read and classify `NotFound` / `AlreadyApplied` /
///    `StaleState`, exactly like [`transition_autonomous_daily_operation`].
pub async fn transition_autonomous_daily_operation_to_running(
    pool: &PgPool,
    args: &TransitionAutonomousDailyOperationToRunningArgs,
) -> Result<AutonomousDailyTransitionOutcome> {
    validate_transition_to_running_args(args)?;

    if !is_legal_operation_transition(Some(&args.expected_state), STATE_RUNNING) {
        return Ok(AutonomousDailyTransitionOutcome::IllegalTransition);
    }

    let mut tx = pool
        .begin()
        .await
        .context("transition_autonomous_daily_operation_to_running: begin transaction failed")?;

    let new_version = args.expected_state_version + 1;

    let updated_row = sqlx::query(&format!(
        r#"
        update sys_autonomous_daily_operations
        set state = $1,
            state_reason_code = null,
            state_blocker_signature = null,
            state_version = $2,
            run_id = $3,
            started_at_utc = coalesce(started_at_utc, $4),
            next_retry_utc = null,
            last_error = null,
            updated_at_utc = $5
        where operation_id = $6 and state = $7 and state_version = $8
        returning {OPERATION_COLUMNS}
        "#
    ))
    .bind(STATE_RUNNING)
    .bind(new_version)
    .bind(args.run_id)
    .bind(args.started_at_utc)
    .bind(args.occurred_at_utc)
    .bind(args.operation_id)
    .bind(&args.expected_state)
    .bind(args.expected_state_version)
    .fetch_optional(&mut *tx)
    .await
    .context("transition_autonomous_daily_operation_to_running: CAS update failed")?;

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
        .bind(STATE_RUNNING)
        .bind(None::<String>)
        .bind(args.occurred_at_utc)
        .bind(Some(args.run_id))
        .bind(&args.bounded_detail)
        .execute(&mut *tx)
        .await
        .context("transition_autonomous_daily_operation_to_running: insert event failed")?;

        tx.commit()
            .await
            .context("transition_autonomous_daily_operation_to_running: commit (applied) failed")?;

        return Ok(AutonomousDailyTransitionOutcome::Applied(record));
    }

    let current_row = sqlx::query(&format!(
        "select {OPERATION_COLUMNS} from sys_autonomous_daily_operations where operation_id = $1"
    ))
    .bind(args.operation_id)
    .fetch_optional(&mut *tx)
    .await
    .context("transition_autonomous_daily_operation_to_running: current-row re-read failed")?;

    let outcome = match current_row {
        None => AutonomousDailyTransitionOutcome::NotFound,
        Some(row) => {
            let current = row_to_operation_record(row)?;
            if current.state == STATE_RUNNING
                && current.state_version == new_version
                && current.run_id == Some(args.run_id)
            {
                let event_row = sqlx::query(
                    "select from_state, to_state from sys_autonomous_daily_operation_events \
                     where operation_id = $1 and transition_seq = $2",
                )
                .bind(args.operation_id)
                .bind(new_version)
                .fetch_optional(&mut *tx)
                .await
                .context(
                    "transition_autonomous_daily_operation_to_running: replay event lookup failed",
                )?;

                let is_exact_replay = event_row
                    .map(|r| {
                        let from_state: String = r.try_get("from_state").unwrap_or_default();
                        let to_state: String = r.try_get("to_state").unwrap_or_default();
                        from_state == args.expected_state && to_state == STATE_RUNNING
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

    tx.rollback().await.context(
        "transition_autonomous_daily_operation_to_running: rollback (no-op read) failed",
    )?;

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

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER
// Durable provider-poll and bar-observation evidence
// ---------------------------------------------------------------------------
//
// These recorders update bookkeeping counters/evidence columns on an
// existing `sys_autonomous_daily_operations` row only. They never touch
// `state_version` and never insert a `sys_autonomous_daily_operation_events`
// row -- evidence accumulation is not itself a lifecycle transition. All
// timestamps are caller-supplied; nothing here reads the wall clock.

fn validate_data_refresh_state(data_refresh_state: &str) -> Result<()> {
    if data_refresh_state.trim().is_empty() {
        anyhow::bail!("data_refresh_state must not be empty");
    }
    if data_refresh_state.len() > 64 {
        anyhow::bail!("data_refresh_state exceeds 64 chars");
    }
    Ok(())
}

/// Outcome of a provider-poll evidence recorder. `NotFound` is authoritative
/// "no such operation" -- never silently a no-op.
#[derive(Debug, Clone)]
pub enum RecordProviderPollOutcome {
    Recorded {
        provider_poll_attempt_count: i64,
        provider_poll_success_count: i64,
        provider_poll_failure_count: i64,
    },
    NotFound,
}

fn provider_poll_counts_from_row(
    r: sqlx::postgres::PgRow,
) -> Result<RecordProviderPollOutcome, sqlx::Error> {
    Ok(RecordProviderPollOutcome::Recorded {
        provider_poll_attempt_count: r.try_get("provider_poll_attempt_count")?,
        provider_poll_success_count: r.try_get("provider_poll_success_count")?,
        provider_poll_failure_count: r.try_get("provider_poll_failure_count")?,
    })
}

/// Record that a provider-poll attempt began: increments
/// `provider_poll_attempt_count` by exactly one, and advances
/// `last_provider_poll_utc`/`data_refresh_state`.
pub async fn record_provider_poll_started(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
    data_refresh_state: &str,
) -> Result<RecordProviderPollOutcome> {
    validate_data_refresh_state(data_refresh_state)?;
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set provider_poll_attempt_count = provider_poll_attempt_count + 1,
            last_provider_poll_utc = $2,
            data_refresh_state = $3,
            updated_at_utc = $2
        where operation_id = $1
        returning provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count
        "#,
    )
    .bind(operation_id)
    .bind(occurred_at_utc)
    .bind(data_refresh_state)
    .fetch_optional(pool)
    .await
    .context("record_provider_poll_started failed")?;

    match row {
        Some(r) => provider_poll_counts_from_row(r).map_err(Into::into),
        None => Ok(RecordProviderPollOutcome::NotFound),
    }
}

/// Record that a provider-poll attempt succeeded: increments
/// `provider_poll_success_count` by exactly one, and advances
/// `last_provider_poll_utc`/`data_refresh_state`.
pub async fn record_provider_poll_succeeded(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
    data_refresh_state: &str,
) -> Result<RecordProviderPollOutcome> {
    validate_data_refresh_state(data_refresh_state)?;
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set provider_poll_success_count = provider_poll_success_count + 1,
            last_provider_poll_utc = $2,
            data_refresh_state = $3,
            updated_at_utc = $2
        where operation_id = $1
        returning provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count
        "#,
    )
    .bind(operation_id)
    .bind(occurred_at_utc)
    .bind(data_refresh_state)
    .fetch_optional(pool)
    .await
    .context("record_provider_poll_succeeded failed")?;

    match row {
        Some(r) => provider_poll_counts_from_row(r).map_err(Into::into),
        None => Ok(RecordProviderPollOutcome::NotFound),
    }
}

/// Record that a provider-poll attempt failed: increments
/// `provider_poll_failure_count` by exactly one, advances
/// `last_provider_poll_utc`/`data_refresh_state`, and records a bounded
/// `last_error`.
pub async fn record_provider_poll_failed(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
    data_refresh_state: &str,
    last_error: &str,
) -> Result<RecordProviderPollOutcome> {
    validate_data_refresh_state(data_refresh_state)?;
    if last_error.len() > 4000 {
        anyhow::bail!("record_provider_poll_failed: last_error exceeds 4000 chars");
    }
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set provider_poll_failure_count = provider_poll_failure_count + 1,
            last_provider_poll_utc = $2,
            data_refresh_state = $3,
            last_error = $4,
            updated_at_utc = $2
        where operation_id = $1
        returning provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count
        "#,
    )
    .bind(operation_id)
    .bind(occurred_at_utc)
    .bind(data_refresh_state)
    .bind(last_error)
    .fetch_optional(pool)
    .await
    .context("record_provider_poll_failed failed")?;

    match row {
        Some(r) => provider_poll_counts_from_row(r).map_err(Into::into),
        None => Ok(RecordProviderPollOutcome::NotFound),
    }
}

/// Outcome of [`record_completed_bar_observed`].
#[derive(Debug, Clone)]
pub enum RecordCompletedBarObservedOutcome {
    /// `bar_end_ts` is newer than the previously recorded value (or none was
    /// recorded yet): `bars_observed` incremented by exactly one,
    /// `last_completed_bar_ts` advanced to `bar_end_ts`.
    Recorded { bars_observed: i64 },
    /// `bar_end_ts` exactly matches the already-recorded
    /// `last_completed_bar_ts` — idempotent replay, no counter change.
    AlreadyObserved { bars_observed: i64 },
    /// `bar_end_ts` is older than the already-recorded
    /// `last_completed_bar_ts` — refused as a no-op; evidence never rewinds.
    StaleBarIgnored {
        current_last_completed_bar_ts: Option<i64>,
    },
    /// No row exists for `operation_id`.
    NotFound,
}

/// Record that a genuinely new completed bar was observed for one
/// operation. Idempotent on an exact replay of the same `bar_end_ts`;
/// refuses (as a no-op) an older `bar_end_ts` than what is already
/// recorded. Does not touch `state_version` and inserts no transition
/// event.
pub async fn record_completed_bar_observed(
    pool: &PgPool,
    operation_id: Uuid,
    bar_end_ts: i64,
    occurred_at_utc: DateTime<Utc>,
) -> Result<RecordCompletedBarObservedOutcome> {
    if bar_end_ts <= 0 {
        anyhow::bail!("record_completed_bar_observed: bar_end_ts must be positive");
    }

    let advanced = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set bars_observed = bars_observed + 1,
            last_completed_bar_ts = $2,
            updated_at_utc = $3
        where operation_id = $1
          and (last_completed_bar_ts is null or last_completed_bar_ts < $2)
        returning bars_observed
        "#,
    )
    .bind(operation_id)
    .bind(bar_end_ts)
    .bind(occurred_at_utc)
    .fetch_optional(pool)
    .await
    .context("record_completed_bar_observed: guarded advance failed")?;

    if let Some(r) = advanced {
        return Ok(RecordCompletedBarObservedOutcome::Recorded {
            bars_observed: r.try_get("bars_observed")?,
        });
    }

    let current = sqlx::query(
        "select bars_observed, last_completed_bar_ts from sys_autonomous_daily_operations \
         where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_optional(pool)
    .await
    .context("record_completed_bar_observed: current-row read failed")?;

    Ok(match current {
        None => RecordCompletedBarObservedOutcome::NotFound,
        Some(r) => {
            let bars_observed: i64 = r.try_get("bars_observed")?;
            let last_completed_bar_ts: Option<i64> = r.try_get("last_completed_bar_ts")?;
            if last_completed_bar_ts == Some(bar_end_ts) {
                RecordCompletedBarObservedOutcome::AlreadyObserved { bars_observed }
            } else {
                RecordCompletedBarObservedOutcome::StaleBarIgnored {
                    current_last_completed_bar_ts: last_completed_bar_ts,
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER
// Durable bar-identity-keyed dispatch claims (migration 0050)
// ---------------------------------------------------------------------------
//
// Audit conclusion: no existing seam (`strategy_signal_evaluations`'
// `evaluation_id`, keyed on an arbitrary tick counter; `oms_outbox`'s
// `decision_id`, keyed on wall-clock `now_micros`) provides durable
// idempotency keyed on bar identity (local_symbol, timeframe, bar_end_ts).
// `sys_autonomous_daily_bar_dispatches` closes that gap: claim before
// dispatch, complete only after confirmed success, and any non-`completed`
// outcome permanently blocks automatic redispatch of that exact bar
// identity (Phase D owns manual recovery).

pub const DISPATCH_STATUS_CLAIMED: &str = "claimed";
pub const DISPATCH_STATUS_COMPLETED: &str = "completed";
pub const DISPATCH_STATUS_UNCERTAIN: &str = "uncertain";
pub const DISPATCH_STATUS_FAILED: &str = "failed";

/// One row from `sys_autonomous_daily_bar_dispatches`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutonomousDailyBarDispatchRecord {
    pub operation_id: Uuid,
    pub local_symbol: String,
    pub timeframe: String,
    pub bar_end_ts: i64,
    pub status: String,
    pub claimed_at_utc: DateTime<Utc>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub evaluation_id: Option<Uuid>,
    pub last_error: Option<String>,
}

const BAR_DISPATCH_COLUMNS: &str = r#"
    operation_id, local_symbol, timeframe, bar_end_ts, status,
    claimed_at_utc, completed_at_utc, evaluation_id, last_error
"#;

fn row_to_bar_dispatch_record(
    r: sqlx::postgres::PgRow,
) -> Result<AutonomousDailyBarDispatchRecord, sqlx::Error> {
    Ok(AutonomousDailyBarDispatchRecord {
        operation_id: r.try_get("operation_id")?,
        local_symbol: r.try_get("local_symbol")?,
        timeframe: r.try_get("timeframe")?,
        bar_end_ts: r.try_get("bar_end_ts")?,
        status: r.try_get("status")?,
        claimed_at_utc: r.try_get("claimed_at_utc")?,
        completed_at_utc: r.try_get("completed_at_utc")?,
        evaluation_id: r.try_get("evaluation_id")?,
        last_error: r.try_get("last_error")?,
    })
}

fn validate_bar_identity(local_symbol: &str, timeframe: &str, bar_end_ts: i64) -> Result<()> {
    if local_symbol.trim().is_empty() {
        anyhow::bail!("local_symbol must not be empty");
    }
    if timeframe.trim().is_empty() {
        anyhow::bail!("timeframe must not be empty");
    }
    if bar_end_ts <= 0 {
        anyhow::bail!("bar_end_ts must be positive");
    }
    Ok(())
}

/// Outcome of [`claim_autonomous_daily_bar_dispatch`].
#[derive(Debug, Clone)]
pub enum BarDispatchClaimOutcome {
    /// A new claim row was inserted — the caller may proceed to dispatch.
    Claimed,
    /// The bar identity was already durably dispatched to completion.
    AlreadyCompleted { evaluation_id: Option<Uuid> },
    /// A prior claim exists but is not provably complete. Never dispatch
    /// automatically. `status` reflects the (possibly just-reclassified)
    /// current status.
    Unresolved { status: String },
}

/// Race-safe claim of one bar identity within one operation.
///
/// `INSERT ... ON CONFLICT DO NOTHING` gives exactly-once claim creation.
/// When the row already exists: a `completed` row yields `AlreadyCompleted`
/// unchanged; a `claimed` row is reclassified to `uncertain` in the same
/// transaction (a second claim attempt against a still-`claimed` row is
/// exactly the observable signature of a prior attempt whose outcome was
/// never confirmed — including across a daemon restart, since no other
/// signal distinguishes "still in flight" from "the process that claimed it
/// is gone") and returned as `Unresolved`; an already-`uncertain` or
/// `failed` row is returned as `Unresolved` unchanged.
pub async fn claim_autonomous_daily_bar_dispatch(
    pool: &PgPool,
    operation_id: Uuid,
    local_symbol: &str,
    timeframe: &str,
    bar_end_ts: i64,
    claimed_at_utc: DateTime<Utc>,
) -> Result<BarDispatchClaimOutcome> {
    validate_bar_identity(local_symbol, timeframe, bar_end_ts)?;

    let mut tx = pool
        .begin()
        .await
        .context("claim_autonomous_daily_bar_dispatch: begin transaction failed")?;

    let inserted = sqlx::query(&format!(
        r#"
        insert into sys_autonomous_daily_bar_dispatches ({BAR_DISPATCH_COLUMNS})
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (operation_id, local_symbol, timeframe, bar_end_ts) do nothing
        "#
    ))
    .bind(operation_id)
    .bind(local_symbol)
    .bind(timeframe)
    .bind(bar_end_ts)
    .bind(DISPATCH_STATUS_CLAIMED)
    .bind(claimed_at_utc)
    .bind(None::<DateTime<Utc>>)
    .bind(None::<Uuid>)
    .bind(None::<String>)
    .execute(&mut *tx)
    .await
    .context("claim_autonomous_daily_bar_dispatch: insert failed")?;

    if inserted.rows_affected() == 1 {
        tx.commit()
            .await
            .context("claim_autonomous_daily_bar_dispatch: commit (claimed) failed")?;
        return Ok(BarDispatchClaimOutcome::Claimed);
    }

    let existing = sqlx::query(&format!(
        "select {BAR_DISPATCH_COLUMNS} from sys_autonomous_daily_bar_dispatches \
         where operation_id = $1 and local_symbol = $2 and timeframe = $3 and bar_end_ts = $4 \
         for update"
    ))
    .bind(operation_id)
    .bind(local_symbol)
    .bind(timeframe)
    .bind(bar_end_ts)
    .fetch_one(&mut *tx)
    .await
    .context("claim_autonomous_daily_bar_dispatch: existing-row read failed")?;

    let record = row_to_bar_dispatch_record(existing)?;

    let outcome = if record.status == DISPATCH_STATUS_COMPLETED {
        BarDispatchClaimOutcome::AlreadyCompleted {
            evaluation_id: record.evaluation_id,
        }
    } else if record.status == DISPATCH_STATUS_CLAIMED {
        sqlx::query(
            "update sys_autonomous_daily_bar_dispatches set status = $5 \
             where operation_id = $1 and local_symbol = $2 and timeframe = $3 and bar_end_ts = $4",
        )
        .bind(operation_id)
        .bind(local_symbol)
        .bind(timeframe)
        .bind(bar_end_ts)
        .bind(DISPATCH_STATUS_UNCERTAIN)
        .execute(&mut *tx)
        .await
        .context("claim_autonomous_daily_bar_dispatch: reclassify to uncertain failed")?;
        BarDispatchClaimOutcome::Unresolved {
            status: DISPATCH_STATUS_UNCERTAIN.to_string(),
        }
    } else {
        BarDispatchClaimOutcome::Unresolved {
            status: record.status,
        }
    };

    tx.commit()
        .await
        .context("claim_autonomous_daily_bar_dispatch: commit (existing) failed")?;

    Ok(outcome)
}

/// Mark a previously-`claimed` bar identity as durably completed, and
/// advance the parent operation's `bars_dispatched`/`last_dispatched_bar_ts`
/// evidence in the same transaction. Returns `true` iff the claim row was
/// actually transitioned by this call (guarded on `status = 'claimed'`, so a
/// second call is a safe no-op rather than a double-increment). The
/// operation-row advance mirrors [`record_completed_bar_observed`]'s
/// idempotent, never-rewinds guard.
pub async fn complete_autonomous_daily_bar_dispatch(
    pool: &PgPool,
    operation_id: Uuid,
    local_symbol: &str,
    timeframe: &str,
    bar_end_ts: i64,
    completed_at_utc: DateTime<Utc>,
    evaluation_id: Option<Uuid>,
) -> Result<bool> {
    validate_bar_identity(local_symbol, timeframe, bar_end_ts)?;

    let mut tx = pool
        .begin()
        .await
        .context("complete_autonomous_daily_bar_dispatch: begin transaction failed")?;

    let updated = sqlx::query(
        "update sys_autonomous_daily_bar_dispatches \
         set status = $5, completed_at_utc = $6, evaluation_id = $7 \
         where operation_id = $1 and local_symbol = $2 and timeframe = $3 and bar_end_ts = $4 \
           and status = $8",
    )
    .bind(operation_id)
    .bind(local_symbol)
    .bind(timeframe)
    .bind(bar_end_ts)
    .bind(DISPATCH_STATUS_COMPLETED)
    .bind(completed_at_utc)
    .bind(evaluation_id)
    .bind(DISPATCH_STATUS_CLAIMED)
    .execute(&mut *tx)
    .await
    .context("complete_autonomous_daily_bar_dispatch: update claim failed")?;

    let claim_updated = updated.rows_affected() == 1;

    if claim_updated {
        sqlx::query(
            r#"
            update sys_autonomous_daily_operations
            set bars_dispatched = bars_dispatched + 1,
                last_dispatched_bar_ts = $2,
                updated_at_utc = $3
            where operation_id = $1
              and (last_dispatched_bar_ts is null or last_dispatched_bar_ts < $2)
            "#,
        )
        .bind(operation_id)
        .bind(bar_end_ts)
        .bind(completed_at_utc)
        .execute(&mut *tx)
        .await
        .context("complete_autonomous_daily_bar_dispatch: operation evidence advance failed")?;
    }

    tx.commit()
        .await
        .context("complete_autonomous_daily_bar_dispatch: commit failed")?;

    Ok(claim_updated)
}

/// Mark a previously-`claimed` bar identity as a known, non-crash dispatch
/// failure (`failed`) — distinct from `uncertain` (reserved for the
/// cannot-tell-what-happened case a second claim attempt detects in
/// [`claim_autonomous_daily_bar_dispatch`]). Both statuses block automatic
/// redispatch identically; the distinction is diagnostic only. Returns
/// `true` iff the row was actually transitioned (guarded on
/// `status = 'claimed'`).
pub async fn fail_autonomous_daily_bar_dispatch(
    pool: &PgPool,
    operation_id: Uuid,
    local_symbol: &str,
    timeframe: &str,
    bar_end_ts: i64,
    last_error: &str,
) -> Result<bool> {
    validate_bar_identity(local_symbol, timeframe, bar_end_ts)?;
    if last_error.len() > 4000 {
        anyhow::bail!("fail_autonomous_daily_bar_dispatch: last_error exceeds 4000 chars");
    }
    let updated = sqlx::query(
        "update sys_autonomous_daily_bar_dispatches \
         set status = $5, last_error = $6 \
         where operation_id = $1 and local_symbol = $2 and timeframe = $3 and bar_end_ts = $4 \
           and status = $7",
    )
    .bind(operation_id)
    .bind(local_symbol)
    .bind(timeframe)
    .bind(bar_end_ts)
    .bind(DISPATCH_STATUS_FAILED)
    .bind(last_error)
    .bind(DISPATCH_STATUS_CLAIMED)
    .execute(pool)
    .await
    .context("fail_autonomous_daily_bar_dispatch: update failed")?;
    Ok(updated.rows_affected() == 1)
}

/// Fetch one bar-dispatch claim row, if any. `Ok(None)` is authoritative "no
/// claim has ever been made for this bar identity" — never a synthesized
/// row. Read-only.
pub async fn fetch_autonomous_daily_bar_dispatch(
    pool: &PgPool,
    operation_id: Uuid,
    local_symbol: &str,
    timeframe: &str,
    bar_end_ts: i64,
) -> Result<Option<AutonomousDailyBarDispatchRecord>> {
    let row = sqlx::query(&format!(
        "select {BAR_DISPATCH_COLUMNS} from sys_autonomous_daily_bar_dispatches \
         where operation_id = $1 and local_symbol = $2 and timeframe = $3 and bar_end_ts = $4"
    ))
    .bind(operation_id)
    .bind(local_symbol)
    .bind(timeframe)
    .bind(bar_end_ts)
    .fetch_optional(pool)
    .await
    .context("fetch_autonomous_daily_bar_dispatch failed")?;
    row.map(row_to_bar_dispatch_record)
        .transpose()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR
// Start/running/stop attempt evidence recorders
// ---------------------------------------------------------------------------
//
// Counter-only updates over an existing sys_autonomous_daily_operations row.
// None of these touch state_version and none inserts a
// sys_autonomous_daily_operation_events row -- attempt/evidence bookkeeping
// is not itself a lifecycle transition (mirrors the provider-poll/bar
// recorders above). All timestamps are caller-supplied; nothing here reads
// the wall clock. `NotFound` is authoritative "no such operation" -- never a
// silent no-op.

/// Outcome of [`record_start_attempt`].
#[derive(Debug, Clone)]
pub enum RecordStartAttemptOutcome {
    Recorded { start_attempt_count: i64 },
    NotFound,
}

/// Record one canonical `start_execution_runtime` call attempt: increments
/// `start_attempt_count` by exactly one, advances `last_start_attempt_utc`,
/// clears any stale `last_error`, and sets `next_retry_utc` to the
/// caller-supplied value (`None` clears it).
pub async fn record_start_attempt(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
    next_retry_utc: Option<DateTime<Utc>>,
) -> Result<RecordStartAttemptOutcome> {
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set start_attempt_count = start_attempt_count + 1,
            last_start_attempt_utc = $2,
            next_retry_utc = $3,
            last_error = null,
            updated_at_utc = $2
        where operation_id = $1
        returning start_attempt_count
        "#,
    )
    .bind(operation_id)
    .bind(occurred_at_utc)
    .bind(next_retry_utc)
    .fetch_optional(pool)
    .await
    .context("record_start_attempt failed")?;

    Ok(match row {
        Some(r) => RecordStartAttemptOutcome::Recorded {
            start_attempt_count: r.try_get("start_attempt_count")?,
        },
        None => RecordStartAttemptOutcome::NotFound,
    })
}

/// Outcome of [`record_running_started`].
#[derive(Debug, Clone)]
pub enum RecordRunningStartedOutcome {
    Recorded { started_at_utc: DateTime<Utc> },
    NotFound,
}

/// Record the durable operation's successful start: sets `started_at_utc`
/// (idempotent -- `coalesce` never rewinds an already-recorded value),
/// clears `next_retry_utc`, and clears any stale `last_error`. Does not set
/// `run_id` -- callers bind `run_id` atomically as part of the CAS
/// transition to `running` via [`transition_autonomous_daily_operation`]'s
/// own `run_id` argument, in the same tick, so `run_id` and `state` are
/// never observed out of sync.
pub async fn record_running_started(
    pool: &PgPool,
    operation_id: Uuid,
    started_at_utc: DateTime<Utc>,
) -> Result<RecordRunningStartedOutcome> {
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set started_at_utc = coalesce(started_at_utc, $2),
            next_retry_utc = null,
            last_error = null,
            updated_at_utc = $2
        where operation_id = $1
        returning started_at_utc
        "#,
    )
    .bind(operation_id)
    .bind(started_at_utc)
    .fetch_optional(pool)
    .await
    .context("record_running_started failed")?;

    Ok(match row {
        Some(r) => RecordRunningStartedOutcome::Recorded {
            started_at_utc: r.try_get("started_at_utc")?,
        },
        None => RecordRunningStartedOutcome::NotFound,
    })
}

/// Outcome of [`record_retry_timing`] / [`clear_retry_timing`].
#[derive(Debug, Clone)]
pub enum RecordRetryTimingOutcome {
    Recorded,
    NotFound,
}

/// Record transient-retry timing/error without a state transition: sets
/// `next_retry_utc` (`None` clears it) and `last_error` (bounded to 4000
/// chars; `None` clears it).
pub async fn record_retry_timing(
    pool: &PgPool,
    operation_id: Uuid,
    next_retry_utc: Option<DateTime<Utc>>,
    last_error: Option<&str>,
    occurred_at_utc: DateTime<Utc>,
) -> Result<RecordRetryTimingOutcome> {
    if let Some(err) = last_error {
        if err.len() > 4000 {
            anyhow::bail!("record_retry_timing: last_error exceeds 4000 chars");
        }
    }
    let result = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set next_retry_utc = $2,
            last_error = $3,
            updated_at_utc = $4
        where operation_id = $1
        "#,
    )
    .bind(operation_id)
    .bind(next_retry_utc)
    .bind(last_error)
    .bind(occurred_at_utc)
    .execute(pool)
    .await
    .context("record_retry_timing failed")?;

    Ok(if result.rows_affected() == 1 {
        RecordRetryTimingOutcome::Recorded
    } else {
        RecordRetryTimingOutcome::NotFound
    })
}

/// Clear retry timing/error without a state transition. Equivalent to
/// [`record_retry_timing`] with both fields `None`.
pub async fn clear_retry_timing(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
) -> Result<RecordRetryTimingOutcome> {
    record_retry_timing(pool, operation_id, None, None, occurred_at_utc).await
}

/// Outcome of [`record_stop_attempt`].
#[derive(Debug, Clone)]
pub enum RecordStopAttemptOutcome {
    Recorded { stop_attempt_count: i64 },
    NotFound,
}

/// Record one canonical `stop_execution_runtime` call attempt: increments
/// `stop_attempt_count` by exactly one from `coalesce(stop_attempt_count,
/// 0)` (a legacy-null row starts counting from zero on its first recorded
/// attempt, never fabricating prior history) and advances
/// `last_stop_attempt_utc`.
pub async fn record_stop_attempt(
    pool: &PgPool,
    operation_id: Uuid,
    occurred_at_utc: DateTime<Utc>,
) -> Result<RecordStopAttemptOutcome> {
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set stop_attempt_count = coalesce(stop_attempt_count, 0) + 1,
            last_stop_attempt_utc = $2,
            updated_at_utc = $2
        where operation_id = $1
        returning stop_attempt_count
        "#,
    )
    .bind(operation_id)
    .bind(occurred_at_utc)
    .fetch_optional(pool)
    .await
    .context("record_stop_attempt failed")?;

    Ok(match row {
        Some(r) => RecordStopAttemptOutcome::Recorded {
            stop_attempt_count: r.try_get("stop_attempt_count")?,
        },
        None => RecordStopAttemptOutcome::NotFound,
    })
}

/// Outcome of [`record_stopped_at`].
#[derive(Debug, Clone)]
pub enum RecordStoppedAtOutcome {
    Recorded { stopped_at_utc: DateTime<Utc> },
    NotFound,
}

/// Record a successful canonical stop: sets `stopped_at_utc` (idempotent --
/// `coalesce` never rewinds an already-recorded value). The operation's
/// `state` transition to `stopping`/`stop_retrying`/`completed*` remains a
/// separate CAS call via [`transition_autonomous_daily_operation`] -- this
/// recorder only advances the evidence timestamp.
pub async fn record_stopped_at(
    pool: &PgPool,
    operation_id: Uuid,
    stopped_at_utc: DateTime<Utc>,
) -> Result<RecordStoppedAtOutcome> {
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set stopped_at_utc = coalesce(stopped_at_utc, $2),
            updated_at_utc = $2
        where operation_id = $1
        returning stopped_at_utc
        "#,
    )
    .bind(operation_id)
    .bind(stopped_at_utc)
    .fetch_optional(pool)
    .await
    .context("record_stopped_at failed")?;

    Ok(match row {
        Some(r) => RecordStoppedAtOutcome::Recorded {
            stopped_at_utc: r.try_get("stopped_at_utc")?,
        },
        None => RecordStoppedAtOutcome::NotFound,
    })
}

/// Outcome of [`record_autonomous_runtime_stopped`].
#[derive(Debug, Clone)]
pub enum RecordAutonomousRuntimeStoppedOutcome {
    Recorded { stopped_at_utc: DateTime<Utc> },
    NotFound,
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01:
/// restart-safe canonical stop completion. Atomically:
///
/// - sets `stopped_at_utc` only when currently null (`coalesce`, never
///   rewinds an already-recorded value — mirrors [`record_stopped_at`]);
/// - clears `next_retry_utc`, `last_error`, and `state_blocker_signature`,
///   so a resolved stop leaves no stale retry/blocker evidence behind;
/// - advances `updated_at_utc`.
///
/// The operation's `state` transition to `stopping`/`stop_retrying`/
/// `completed*` remains a separate CAS call via
/// [`transition_autonomous_daily_operation`] — this recorder only advances
/// evidence. Idempotent by construction: if a stop succeeds but this write
/// fails, the next tick observing no local runtime plus a terminal durable
/// run can safely call this again with no risk of rewinding
/// `stopped_at_utc` or issuing a second canonical stop call (callers decide
/// the latter; this function performs no stop call itself).
pub async fn record_autonomous_runtime_stopped(
    pool: &PgPool,
    operation_id: Uuid,
    stopped_at_utc: DateTime<Utc>,
) -> Result<RecordAutonomousRuntimeStoppedOutcome> {
    let row = sqlx::query(
        r#"
        update sys_autonomous_daily_operations
        set stopped_at_utc = coalesce(stopped_at_utc, $2),
            next_retry_utc = null,
            last_error = null,
            state_blocker_signature = null,
            updated_at_utc = $2
        where operation_id = $1
        returning stopped_at_utc
        "#,
    )
    .bind(operation_id)
    .bind(stopped_at_utc)
    .fetch_optional(pool)
    .await
    .context("record_autonomous_runtime_stopped failed")?;

    Ok(match row {
        Some(r) => RecordAutonomousRuntimeStoppedOutcome::Recorded {
            stopped_at_utc: r.try_get("stopped_at_utc")?,
        },
        None => RecordAutonomousRuntimeStoppedOutcome::NotFound,
    })
}
