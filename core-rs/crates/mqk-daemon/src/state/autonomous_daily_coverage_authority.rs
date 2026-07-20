//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
//! FOUNDATION: the durable, operation-scoped `autonomous_daily_coverage_bound`
//! event -- typed model, parser, semantic equality, canonical construction,
//! and the write/re-read/replay/conflict authority contract (binding
//! contract §6a).
//!
//! This module implements exactly the durable evidence foundation the E1
//! contract authorizes: a typed payload, a pure constructor reusing only
//! [`crate::daily_data_readiness::expected_intraday_end_ts_window`] and
//! [`crate::daily_data_readiness::intraday_grid_starts`], and the exact
//! write/re-read/idempotent-replay/conflict contract over the existing
//! `sys_autonomous_session_events` store. It implements no outcome
//! classifier and no finalization behavior (E2B, not started here).

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;
use sqlx::PgPool;
use uuid::Uuid;

use super::market_calendar::{
    CalendarCoverageState, MarketCalendarProvider, MarketSessionSchedule,
};
use super::{MultiSymbolRuntimeConfig, SymbolStrategyAssignment};
use mqk_db::AutonomousDailyOperationRecord;

// ---------------------------------------------------------------------------
// Event identity
// ---------------------------------------------------------------------------

pub const EVENT_TYPE_COVERAGE_BOUND: &str = "autonomous_daily_coverage_bound";
pub const COVERAGE_SOURCE: &str = "mqk-daemon.autonomous_daily_coordinator";
pub const COVERAGE_SCHEMA_VERSION: i64 = 1;

/// Deterministic, operation-scoped event id. The store's own `(id)` primary
/// key -- not an application-level convention -- is the immutability
/// mechanism: at most one row per operation can ever exist.
pub fn coverage_bound_event_id(operation_id: Uuid) -> String {
    format!("autonomous_daily_coverage_bound:{operation_id}")
}

// ---------------------------------------------------------------------------
// Typed reason codes (adapter/coordinator shared literals)
// ---------------------------------------------------------------------------

pub const REASON_COVERAGE_AUTHORITY_NOT_BOUND: &str = "coverage_authority_not_bound";
pub const REASON_COVERAGE_AUTHORITY_UNREADABLE: &str = "coverage_authority_unreadable";
pub const REASON_COVERAGE_AUTHORITY_INVALID: &str = "coverage_authority_invalid";
pub const REASON_COVERAGE_AUTHORITY_CONFLICT: &str = "coverage_authority_conflict";
pub const REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY: &str =
    "coverage_authority_missing_after_activity";
pub const REASON_COVERAGE_POLICY_RESOLUTION_UNAVAILABLE: &str =
    "coverage_policy_resolution_unavailable";
pub const REASON_COVERAGE_POLICY_CONSTRUCTION_FAILED: &str = "coverage_policy_construction_failed";

// ---------------------------------------------------------------------------
// Typed payload (§6a). Deliberately excludes `bound_at_utc`/any caller-`now`
// field -- the bind instant is the event row's own `ts_utc` column, metadata
// only, never part of semantic equality. `#[derive(PartialEq)]` over every
// field below therefore *is* the semantic-equality comparison the stable-
// replay rule requires: no separate comparator is needed or written.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CoverageBoundDetail {
    pub schema_version: i64,
    pub operation_id: Uuid,
    pub market_date: String,
    pub deployment_mode: String,
    pub adapter_id: String,

    pub first_dispatchable_bar_end_ts: i64,
    pub final_dispatchable_bar_end_ts: i64,

    pub local_symbol: String,
    pub timeframe: String,
    pub timeframe_secs: i64,
    pub required_history_bars: i64,
    pub effective_grace_seconds: i64,

    pub session_plan_identity: String,
    pub assignment_identity: String,
    pub runtime_binding_identity: String,

    pub exchange_session_open_utc: DateTime<Utc>,
    pub exchange_session_close_utc: DateTime<Utc>,
    pub effective_operation_open_utc: DateTime<Utc>,
    pub effective_operation_close_utc: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Serialization / parsing -- fail-closed. Unknown schema version, missing,
// duplicated (collapsed to an unexpected key-count mismatch, since a raw
// JSON object with a truly duplicated key is deduplicated by the JSON
// parser itself before this code ever sees it), or type-invalid fields are
// all rejected. No free-form provider/SQL/filesystem/credential/panic text
// ever enters the payload -- every field is a bounded identity/timestamp/
// count value.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageParseError {
    Malformed,
    UnknownSchemaVersion(i64),
}

const EXPECTED_FIELDS: &[&str] = &[
    "schema_version",
    "operation_id",
    "market_date",
    "deployment_mode",
    "adapter_id",
    "first_dispatchable_bar_end_ts",
    "final_dispatchable_bar_end_ts",
    "local_symbol",
    "timeframe",
    "timeframe_secs",
    "required_history_bars",
    "effective_grace_seconds",
    "session_plan_identity",
    "assignment_identity",
    "runtime_binding_identity",
    "exchange_session_open_utc",
    "exchange_session_close_utc",
    "effective_operation_open_utc",
    "effective_operation_close_utc",
];

pub fn serialize_coverage_bound_detail(detail: &CoverageBoundDetail) -> String {
    serde_json::json!({
        "schema_version": detail.schema_version,
        "operation_id": detail.operation_id.to_string(),
        "market_date": detail.market_date,
        "deployment_mode": detail.deployment_mode,
        "adapter_id": detail.adapter_id,
        "first_dispatchable_bar_end_ts": detail.first_dispatchable_bar_end_ts,
        "final_dispatchable_bar_end_ts": detail.final_dispatchable_bar_end_ts,
        "local_symbol": detail.local_symbol,
        "timeframe": detail.timeframe,
        "timeframe_secs": detail.timeframe_secs,
        "required_history_bars": detail.required_history_bars,
        "effective_grace_seconds": detail.effective_grace_seconds,
        "session_plan_identity": detail.session_plan_identity,
        "assignment_identity": detail.assignment_identity,
        "runtime_binding_identity": detail.runtime_binding_identity,
        "exchange_session_open_utc": detail.exchange_session_open_utc.to_rfc3339(),
        "exchange_session_close_utc": detail.exchange_session_close_utc.to_rfc3339(),
        "effective_operation_open_utc": detail.effective_operation_open_utc.to_rfc3339(),
        "effective_operation_close_utc": detail.effective_operation_close_utc.to_rfc3339(),
    })
    .to_string()
}

fn parse_rfc3339(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn parse_coverage_bound_detail(raw: &str) -> Result<CoverageBoundDetail, CoverageParseError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| CoverageParseError::Malformed)?;
    let obj = value.as_object().ok_or(CoverageParseError::Malformed)?;

    // Exact key set required -- no fewer, no extra (also the practical
    // guard against a duplicated key surviving as an unexpected shape,
    // since the JSON parser itself collapses literal duplicate keys before
    // constructing this map).
    if obj.len() != EXPECTED_FIELDS.len() || !EXPECTED_FIELDS.iter().all(|k| obj.contains_key(*k)) {
        return Err(CoverageParseError::Malformed);
    }

    let schema_version = obj["schema_version"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    if schema_version != COVERAGE_SCHEMA_VERSION {
        return Err(CoverageParseError::UnknownSchemaVersion(schema_version));
    }

    let operation_id = obj["operation_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(CoverageParseError::Malformed)?;
    let market_date = obj["market_date"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let deployment_mode = obj["deployment_mode"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let adapter_id = obj["adapter_id"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let first_dispatchable_bar_end_ts = obj["first_dispatchable_bar_end_ts"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    let final_dispatchable_bar_end_ts = obj["final_dispatchable_bar_end_ts"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    let local_symbol = obj["local_symbol"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let timeframe = obj["timeframe"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let timeframe_secs = obj["timeframe_secs"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    let required_history_bars = obj["required_history_bars"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    let effective_grace_seconds = obj["effective_grace_seconds"]
        .as_i64()
        .ok_or(CoverageParseError::Malformed)?;
    let session_plan_identity = obj["session_plan_identity"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let assignment_identity = obj["assignment_identity"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let runtime_binding_identity = obj["runtime_binding_identity"]
        .as_str()
        .ok_or(CoverageParseError::Malformed)?
        .to_string();
    let exchange_session_open_utc =
        parse_rfc3339(&obj["exchange_session_open_utc"]).ok_or(CoverageParseError::Malformed)?;
    let exchange_session_close_utc =
        parse_rfc3339(&obj["exchange_session_close_utc"]).ok_or(CoverageParseError::Malformed)?;
    let effective_operation_open_utc =
        parse_rfc3339(&obj["effective_operation_open_utc"]).ok_or(CoverageParseError::Malformed)?;
    let effective_operation_close_utc = parse_rfc3339(&obj["effective_operation_close_utc"])
        .ok_or(CoverageParseError::Malformed)?;

    let detail = CoverageBoundDetail {
        schema_version,
        operation_id,
        market_date,
        deployment_mode,
        adapter_id,
        first_dispatchable_bar_end_ts,
        final_dispatchable_bar_end_ts,
        local_symbol,
        timeframe,
        timeframe_secs,
        required_history_bars,
        effective_grace_seconds,
        session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        exchange_session_open_utc,
        exchange_session_close_utc,
        effective_operation_open_utc,
        effective_operation_close_utc,
    };

    validate_semantic_invariants(&detail).map_err(|_| CoverageParseError::Malformed)?;
    Ok(detail)
}

fn validate_semantic_invariants(detail: &CoverageBoundDetail) -> Result<(), &'static str> {
    if detail.session_plan_identity.trim().is_empty() {
        return Err("session_plan_identity blank");
    }
    if detail.assignment_identity.trim().is_empty() {
        return Err("assignment_identity blank");
    }
    if detail.runtime_binding_identity.trim().is_empty() {
        return Err("runtime_binding_identity blank");
    }
    if detail.local_symbol.trim().is_empty() {
        return Err("local_symbol blank");
    }
    if detail.timeframe.trim().is_empty() {
        return Err("timeframe blank");
    }
    if detail.market_date.trim().is_empty() {
        return Err("market_date blank");
    }
    if detail.deployment_mode.trim().is_empty() {
        return Err("deployment_mode blank");
    }
    if detail.adapter_id.trim().is_empty() {
        return Err("adapter_id blank");
    }
    if detail.timeframe_secs <= 0 {
        return Err("timeframe_secs not positive");
    }
    if detail.required_history_bars <= 0 {
        return Err("required_history_bars not positive");
    }
    if detail.effective_grace_seconds < 0 {
        return Err("effective_grace_seconds negative");
    }
    if detail.first_dispatchable_bar_end_ts <= 0 || detail.final_dispatchable_bar_end_ts <= 0 {
        return Err("bar end_ts not positive");
    }
    if detail.final_dispatchable_bar_end_ts < detail.first_dispatchable_bar_end_ts {
        return Err("final bar precedes first bar");
    }
    if detail.exchange_session_open_utc >= detail.exchange_session_close_utc {
        return Err("exchange session boundaries invalid");
    }
    if detail.effective_operation_open_utc >= detail.effective_operation_close_utc {
        return Err("effective operation boundaries invalid");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Canonical, side-effect-free construction (§6a/§6 item 2).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageConstructionError {
    BlankIdentity(&'static str),
    NonPositiveTimeframeSecs,
    NonPositiveRequiredHistoryBars,
    NegativeGraceSeconds,
    InvalidSessionBoundaries,
    CalendarWindowUnresolvable,
    EmptyCalendarWindow,
}

/// Every input the canonical coverage constructor needs, already resolved
/// by the caller from durable/persisted authorities -- never re-derived by
/// this function. `local_symbol`/`timeframe`/`timeframe_secs`/
/// `required_history_bars`/`effective_grace_seconds` are the caller's
/// freshly-resolved current policy (via
/// [`resolve_current_coverage_policy_inputs`]); every other field is a
/// persisted, immutable-per-operation fact (from the durable operation row
/// or the session plan that produced it).
pub struct CoverageConstructionInputs<'a> {
    pub operation_id: Uuid,
    pub market_date_tuple: (i64, i64, i64),
    pub previous_trading_date_tuple: (i64, i64, i64),
    pub deployment_mode: &'a str,
    pub adapter_id: &'a str,
    pub exchange_session_open_utc: DateTime<Utc>,
    pub exchange_session_close_utc: DateTime<Utc>,
    pub exchange_is_early_close: bool,
    pub effective_operation_open_utc: DateTime<Utc>,
    pub effective_operation_close_utc: DateTime<Utc>,
    pub session_plan_identity: &'a str,
    pub assignment_identity: &'a str,
    pub runtime_binding_identity: &'a str,
    pub policy: &'a CoveragePolicyInputs,
}

fn format_market_date_tuple(t: (i64, i64, i64)) -> String {
    format!("{:04}-{:02}-{:02}", t.0, t.1, t.2)
}

/// Build one [`CoverageBoundDetail`] from `inputs`, reusing only
/// [`crate::daily_data_readiness::expected_intraday_end_ts_window`] and
/// [`crate::daily_data_readiness::intraday_grid_starts`] -- no second
/// calendar, timeframe, grace, or completed-bar algorithm.
pub fn construct_coverage_bound_detail(
    calendar_provider: &dyn MarketCalendarProvider,
    inputs: &CoverageConstructionInputs<'_>,
) -> Result<CoverageBoundDetail, CoverageConstructionError> {
    if inputs.session_plan_identity.trim().is_empty() {
        return Err(CoverageConstructionError::BlankIdentity(
            "session_plan_identity",
        ));
    }
    if inputs.assignment_identity.trim().is_empty() {
        return Err(CoverageConstructionError::BlankIdentity(
            "assignment_identity",
        ));
    }
    if inputs.runtime_binding_identity.trim().is_empty() {
        return Err(CoverageConstructionError::BlankIdentity(
            "runtime_binding_identity",
        ));
    }
    if inputs.policy.timeframe_secs <= 0 {
        return Err(CoverageConstructionError::NonPositiveTimeframeSecs);
    }
    if inputs.policy.required_history_bars == 0 {
        return Err(CoverageConstructionError::NonPositiveRequiredHistoryBars);
    }
    if inputs.policy.effective_grace_seconds < 0 {
        return Err(CoverageConstructionError::NegativeGraceSeconds);
    }
    if inputs.exchange_session_open_utc >= inputs.exchange_session_close_utc
        || inputs.effective_operation_open_utc >= inputs.effective_operation_close_utc
    {
        return Err(CoverageConstructionError::InvalidSessionBoundaries);
    }

    // Reconstruct the exact [`MarketSessionSchedule`] this operation was
    // bound against, directly from persisted/immutable facts -- never a
    // fresh env-driven calendar resolution, which could disagree with what
    // was true when the operation was created.
    let schedule = MarketSessionSchedule {
        market_date: inputs.market_date_tuple,
        session_open_utc: inputs.exchange_session_open_utc,
        session_close_utc: inputs.exchange_session_close_utc,
        previous_trading_date: inputs.previous_trading_date_tuple,
        is_early_close: inputs.exchange_is_early_close,
        is_trading_day: true,
        calendar_source: "autonomous_daily_operation_bound",
        coverage_state: CalendarCoverageState::Active,
    };

    let timeframe_secs = inputs.policy.timeframe_secs;
    let effective_grace_seconds = inputs.policy.effective_grace_seconds;

    // Bound 4: first intended dispatchable bar -- final element of the
    // expected window evaluated at effective_operation_open_utc.
    let window = crate::daily_data_readiness::expected_intraday_end_ts_window(
        calendar_provider,
        &schedule,
        inputs.effective_operation_open_utc.timestamp(),
        timeframe_secs,
        effective_grace_seconds,
        inputs.policy.required_history_bars,
    )
    .ok_or(CoverageConstructionError::CalendarWindowUnresolvable)?;

    let first_dispatchable_bar_end_ts = *window
        .last()
        .ok_or(CoverageConstructionError::EmptyCalendarWindow)?;

    // Bound 5/6: final intended dispatchable bar -- the last current-session
    // grid identity whose own expectation instant is strictly greater than
    // bound 4's own expectation instant and strictly less than
    // effective_operation_close_utc; first_dispatchable_bar_end_ts itself
    // when none qualifies.
    let current_grid = crate::daily_data_readiness::intraday_grid_starts(
        inputs.exchange_session_open_utc,
        inputs.exchange_session_close_utc,
        timeframe_secs,
    );
    let first_expectation_instant =
        first_dispatchable_bar_end_ts + timeframe_secs + effective_grace_seconds;
    let close_ts = inputs.effective_operation_close_utc.timestamp();

    let final_dispatchable_bar_end_ts = current_grid
        .iter()
        .copied()
        .filter(|&slot_start| {
            let expectation = slot_start + timeframe_secs + effective_grace_seconds;
            expectation > first_expectation_instant && expectation < close_ts
        })
        .max()
        .unwrap_or(first_dispatchable_bar_end_ts);

    let detail = CoverageBoundDetail {
        schema_version: COVERAGE_SCHEMA_VERSION,
        operation_id: inputs.operation_id,
        market_date: format_market_date_tuple(inputs.market_date_tuple),
        deployment_mode: inputs.deployment_mode.to_string(),
        adapter_id: inputs.adapter_id.to_string(),
        first_dispatchable_bar_end_ts,
        final_dispatchable_bar_end_ts,
        local_symbol: inputs.policy.local_symbol.clone(),
        timeframe: inputs.policy.timeframe.clone(),
        timeframe_secs,
        required_history_bars: inputs.policy.required_history_bars as i64,
        effective_grace_seconds,
        session_plan_identity: inputs.session_plan_identity.to_string(),
        assignment_identity: inputs.assignment_identity.to_string(),
        runtime_binding_identity: inputs.runtime_binding_identity.to_string(),
        exchange_session_open_utc: inputs.exchange_session_open_utc,
        exchange_session_close_utc: inputs.exchange_session_close_utc,
        effective_operation_open_utc: inputs.effective_operation_open_utc,
        effective_operation_close_utc: inputs.effective_operation_close_utc,
    };

    validate_semantic_invariants(&detail)
        .map_err(|_| CoverageConstructionError::InvalidSessionBoundaries)?;
    Ok(detail)
}

/// Build [`CoverageConstructionInputs`] from a durable operation row plus a
/// freshly-resolved policy and freshly-derived identity strings. The same
/// helper both the coordinator (immediately after `create_or_recover`) and
/// the completed-bar adapter (on every tick) call, so both sides construct
/// byte-identical payloads from the same immutable operation facts. Returns
/// `None` when the operation row predates migration `0049` and lacks the
/// exchange-calendar columns this construction requires (never fabricated).
pub fn coverage_construction_inputs_from_operation<'a>(
    operation: &'a AutonomousDailyOperationRecord,
    assignment_identity: &'a str,
    runtime_binding_identity: &'a str,
    policy: &'a CoveragePolicyInputs,
) -> Option<CoverageConstructionInputs<'a>> {
    let exchange_session_open_utc = operation.exchange_session_open_utc?;
    let exchange_session_close_utc = operation.exchange_session_close_utc?;
    let exchange_is_early_close = operation.exchange_is_early_close?;
    let previous_trading_date = operation.previous_trading_date?;

    Some(CoverageConstructionInputs {
        operation_id: operation.operation_id,
        market_date_tuple: naive_date_to_tuple(operation.market_date),
        previous_trading_date_tuple: naive_date_to_tuple(previous_trading_date),
        deployment_mode: &operation.deployment_mode,
        adapter_id: &operation.adapter_id,
        exchange_session_open_utc,
        exchange_session_close_utc,
        exchange_is_early_close,
        effective_operation_open_utc: operation.effective_operation_open_utc,
        effective_operation_close_utc: operation.effective_operation_close_utc,
        session_plan_identity: &operation.session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        policy,
    })
}

fn naive_date_to_tuple(d: NaiveDate) -> (i64, i64, i64) {
    (d.year() as i64, d.month() as i64, d.day() as i64)
}

// ---------------------------------------------------------------------------
// Current coverage-policy-inputs resolution -- the one shared place
// `timeframe_secs`/`required_history_bars`/`effective_grace_seconds`/
// `local_symbol`/`timeframe` are derived from current mutable
// assignment/binding/strategy-registry configuration. Reuses only the
// existing pure sub-calls `daily_data_readiness::evaluate_assignment`
// already performs for these two facts -- never a second, independently-
// derived algorithm.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CoveragePolicyInputs {
    pub local_symbol: String,
    pub timeframe: String,
    pub timeframe_secs: i64,
    pub required_history_bars: usize,
    pub effective_grace_seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveragePolicyResolutionError {
    NoSymbolAssignment,
    UnsupportedTimeframe,
    StrategyIdUnresolved,
    StrategyHistoryRequirementUnknown,
}

/// Resolve the current-tick coverage policy from the assignment's own
/// configured timeframe -- the same authority
/// [`crate::daily_data_readiness::evaluate_assignment`] uses for its own
/// expected-bar-window computation. A disagreement between the assignment's
/// configured timeframe and `binding.effective_runtime_timeframe_secs` is
/// `evaluate_assignment`'s own `REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH`
/// readiness blocker, reported by the driver's own readiness/binding gate --
/// this resolver must not preempt that with a second, competing check: doing
/// so would abort coverage-anchor construction before the driver ever gets a
/// chance to report its own typed `BindingBlocked`/`ReadinessBlocked`
/// outcome for the exact same fact.
pub fn resolve_current_coverage_policy_inputs(
    config: &MultiSymbolRuntimeConfig,
    binding: &EffectiveRuntimeBinding,
    strategy_registry: &PluginRegistry,
) -> Result<CoveragePolicyInputs, CoveragePolicyResolutionError> {
    let assignment: &SymbolStrategyAssignment = config
        .symbols
        .first()
        .ok_or(CoveragePolicyResolutionError::NoSymbolAssignment)?;

    let tf = mqk_md::Timeframe::parse(&assignment.timeframe)
        .map_err(|_| CoveragePolicyResolutionError::UnsupportedTimeframe)?;
    let timeframe_secs = tf.duration_secs();

    let strategy_id = binding
        .effective_runtime_strategy_id
        .as_ref()
        .ok_or(CoveragePolicyResolutionError::StrategyIdUnresolved)?;
    let required_history_bars = strategy_registry
        .lookup(strategy_id)
        .ok()
        .and_then(|meta| meta.data_requirements.clone())
        .map(|req| req.minimum_completed_bars)
        .ok_or(CoveragePolicyResolutionError::StrategyHistoryRequirementUnknown)?;

    let configured_grace = crate::daily_data_readiness::configured_grace_seconds_from_env();
    let effective_grace_seconds =
        crate::daily_data_readiness::effective_grace_seconds(configured_grace, timeframe_secs);

    Ok(CoveragePolicyInputs {
        local_symbol: assignment.symbol.trim().to_uppercase(),
        timeframe: tf.as_str().to_string(),
        timeframe_secs,
        required_history_bars,
        effective_grace_seconds,
    })
}

// ---------------------------------------------------------------------------
// Write / re-read / idempotent-replay / conflict authority contract (§6a).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
// Called once per coordinator tick, never in a hot per-row loop; boxing the
// payload would only add an allocation for no measurable benefit here.
#[allow(clippy::large_enum_variant)]
pub enum CoverageAuthorityEnsureResult {
    /// First write confirmed by re-read, or an exact-replay re-read that
    /// already matched.
    Bound(CoverageBoundDetail),
    /// The already-bound row's semantic payload disagrees with `fresh`.
    Conflict,
    /// Missing, unparseable, or unreadable after a write attempt.
    Unreadable,
}

/// Attempt to bind `fresh` as the operation's coverage authority: insert
/// (idempotent, `ON CONFLICT (id) DO NOTHING`), then perform one
/// authoritative exact-id re-read. Never reports success merely because the
/// insert call returned `Ok(())` -- only the re-read decides. A write error
/// is swallowed here (never propagated as a hard failure) because the
/// re-read alone determines committed truth, exactly as the write-failure/
/// uncertain-commit contract requires; a DB error on the re-read itself
/// propagates, since no authority claim can be made without it.
pub async fn write_and_confirm_coverage_authority(
    pool: &PgPool,
    fresh: &CoverageBoundDetail,
    ts_utc: DateTime<Utc>,
) -> anyhow::Result<CoverageAuthorityEnsureResult> {
    let id = coverage_bound_event_id(fresh.operation_id);
    let row = mqk_db::AutonomousSessionEventRow {
        id: id.clone(),
        ts_utc,
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(fresh),
        run_id: None,
        source: COVERAGE_SOURCE.to_string(),
    };
    let _ = mqk_db::persist_autonomous_session_event(pool, &row).await;

    match mqk_db::fetch_autonomous_session_event_by_id(pool, &id).await? {
        None => Ok(CoverageAuthorityEnsureResult::Unreadable),
        Some(existing_row) => match parse_coverage_bound_detail(&existing_row.detail) {
            Err(_) => Ok(CoverageAuthorityEnsureResult::Unreadable),
            Ok(existing) if existing.operation_id != fresh.operation_id => {
                Ok(CoverageAuthorityEnsureResult::Unreadable)
            }
            Ok(existing) if &existing == fresh => {
                Ok(CoverageAuthorityEnsureResult::Bound(existing))
            }
            Ok(_existing) => Ok(CoverageAuthorityEnsureResult::Conflict),
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
// Called once per adapter tick, never in a hot per-row loop; boxing the
// payload would only add an allocation for no measurable benefit here.
#[allow(clippy::large_enum_variant)]
pub enum CoverageAuthorityCheck {
    /// Bound and semantically identical to `fresh`.
    Compatible(CoverageBoundDetail),
    NotBound,
    Unreadable,
    Invalid,
    Conflict,
}

/// Read-only exact-id lookup plus parse/identity/semantic verification --
/// never writes. The completed-bar adapter's per-tick authority gate.
pub async fn check_coverage_authority(
    pool: &PgPool,
    operation_id: Uuid,
    fresh: &CoverageBoundDetail,
) -> anyhow::Result<CoverageAuthorityCheck> {
    let id = coverage_bound_event_id(operation_id);
    let Some(row) = mqk_db::fetch_autonomous_session_event_by_id(pool, &id).await? else {
        return Ok(CoverageAuthorityCheck::NotBound);
    };
    match parse_coverage_bound_detail(&row.detail) {
        Err(_) => Ok(CoverageAuthorityCheck::Unreadable),
        Ok(existing) => {
            if existing.operation_id != operation_id {
                Ok(CoverageAuthorityCheck::Invalid)
            } else if existing == *fresh {
                Ok(CoverageAuthorityCheck::Compatible(existing))
            } else {
                Ok(CoverageAuthorityCheck::Conflict)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pristine-vs-prior-activity evidence check (§6a).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PristineCheckOutcome {
    /// Every durable activity signal is absent -- safe to bind a fresh
    /// anchor after ordinary identity/session/policy verification.
    Pristine,
    /// At least one durable activity signal is present -- the anchor must
    /// never be fabricated retroactively.
    HasActivity,
}

/// Durable evidence check gating whether the coordinator may bind a missing
/// coverage anchor (§6a). Every required DB read must succeed; a read
/// failure propagates so the caller can fail closed exactly like the
/// `HasActivity` case (never optimistically treated as `Pristine`).
pub async fn check_operation_pristine(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
) -> anyhow::Result<PristineCheckOutcome> {
    if operation.run_id.is_some()
        || operation.started_at_utc.is_some()
        || operation.bars_observed != 0
        || operation.bars_dispatched != 0
        || operation.last_completed_bar_ts.is_some()
        || operation.last_dispatched_bar_ts.is_some()
    {
        return Ok(PristineCheckOutcome::HasActivity);
    }

    let claim_count =
        mqk_db::count_autonomous_daily_bar_dispatch_claims(pool, operation.operation_id).await?;
    if claim_count > 0 {
        return Ok(PristineCheckOutcome::HasActivity);
    }

    let lineage =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(pool, operation).await?;
    match lineage {
        Ok(lineage) if lineage.is_empty() => Ok(PristineCheckOutcome::Pristine),
        Ok(_) => Ok(PristineCheckOutcome::HasActivity),
        // A contradictory lineage is itself activity evidence this check
        // cannot safely wave through as pristine.
        Err(_) => Ok(PristineCheckOutcome::HasActivity),
    }
}
