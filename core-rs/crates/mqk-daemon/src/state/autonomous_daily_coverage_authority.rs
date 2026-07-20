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
use serde::Deserialize;
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
// Serialization / parsing -- fail-closed (REPAIR 2, E2A closure). The prior
// approach decoded into a `serde_json::Value` object map, whose keys are a
// `BTreeMap`/`Map` that collapses any literal duplicate JSON key to its last
// occurrence before this code ever saw the shape -- a duplicate `operation_id`
// or `schema_version` field silently vanished rather than being rejected.
// This parser instead decodes directly into a typed wire struct
// (`CoverageBoundDetailWire`) via `#[derive(Deserialize)]`, which serde's
// derive macro itself refuses to populate twice: the generated
// `Visitor::visit_map` tracks each field in an `Option<T>` and returns
// `Error::duplicate_field` the second time any field key is seen, before the
// value is ever converted. `#[serde(deny_unknown_fields)]` rejects any key
// outside the closed set, and every field is non-`Option`, so a missing key
// is also a hard deserialization error -- no fewer, no extra, no duplicate,
// exactly as the exact-key-set contract requires. No free-form provider/SQL/
// filesystem/credential/panic text ever enters the payload -- every field is
// a bounded identity/timestamp/count value.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageParseError {
    Malformed,
    UnknownSchemaVersion(i64),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageBoundDetailWire {
    schema_version: i64,
    operation_id: String,
    market_date: String,
    deployment_mode: String,
    adapter_id: String,
    first_dispatchable_bar_end_ts: i64,
    final_dispatchable_bar_end_ts: i64,
    local_symbol: String,
    timeframe: String,
    timeframe_secs: i64,
    required_history_bars: i64,
    effective_grace_seconds: i64,
    session_plan_identity: String,
    assignment_identity: String,
    runtime_binding_identity: String,
    exchange_session_open_utc: String,
    exchange_session_close_utc: String,
    effective_operation_open_utc: String,
    effective_operation_close_utc: String,
}

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

fn parse_rfc3339_str(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn parse_coverage_bound_detail(raw: &str) -> Result<CoverageBoundDetail, CoverageParseError> {
    // `deny_unknown_fields` rejects any extra key; every field below is a
    // required (non-`Option`) member, so a missing key is also a hard
    // deserialization error; serde's own derived `visit_map` rejects a
    // literal duplicate key the moment it is seen a second time, before this
    // function ever runs. Any of these -- along with plain malformed JSON or
    // a wrong-typed value -- collapses to `Malformed`.
    let wire: CoverageBoundDetailWire =
        serde_json::from_str(raw).map_err(|_| CoverageParseError::Malformed)?;

    if wire.schema_version != COVERAGE_SCHEMA_VERSION {
        return Err(CoverageParseError::UnknownSchemaVersion(
            wire.schema_version,
        ));
    }

    let operation_id =
        Uuid::parse_str(&wire.operation_id).map_err(|_| CoverageParseError::Malformed)?;
    let exchange_session_open_utc =
        parse_rfc3339_str(&wire.exchange_session_open_utc).ok_or(CoverageParseError::Malformed)?;
    let exchange_session_close_utc =
        parse_rfc3339_str(&wire.exchange_session_close_utc).ok_or(CoverageParseError::Malformed)?;
    let effective_operation_open_utc = parse_rfc3339_str(&wire.effective_operation_open_utc)
        .ok_or(CoverageParseError::Malformed)?;
    let effective_operation_close_utc = parse_rfc3339_str(&wire.effective_operation_close_utc)
        .ok_or(CoverageParseError::Malformed)?;

    let detail = CoverageBoundDetail {
        schema_version: wire.schema_version,
        operation_id,
        market_date: wire.market_date,
        deployment_mode: wire.deployment_mode,
        adapter_id: wire.adapter_id,
        first_dispatchable_bar_end_ts: wire.first_dispatchable_bar_end_ts,
        final_dispatchable_bar_end_ts: wire.final_dispatchable_bar_end_ts,
        local_symbol: wire.local_symbol,
        timeframe: wire.timeframe,
        timeframe_secs: wire.timeframe_secs,
        required_history_bars: wire.required_history_bars,
        effective_grace_seconds: wire.effective_grace_seconds,
        session_plan_identity: wire.session_plan_identity,
        assignment_identity: wire.assignment_identity,
        runtime_binding_identity: wire.runtime_binding_identity,
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
// Complete durable event-envelope authority (E2A closure REPAIR 1). A row is
// not authoritative merely because its deterministic id and JSON detail
// payload happen to match -- every other envelope column the store itself
// carries (`event_type`, `source`, `run_id`, `resume_source`) must also
// match the exact shape this event type is defined to have. This is the one
// shared validator every authority path below (`write_and_confirm_coverage_
// authority`'s re-read, `check_coverage_authority_envelope`'s adapter/
// coordinator read path) is required to use -- never a second,
// independently-derived envelope check.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageAuthorityEnvelopeError {
    WrongId,
    WrongEventType,
    RunIdNotNull,
    ResumeSourceNotNull,
    WrongSource,
}

/// Validate every envelope column of a `sys_autonomous_session_events` row
/// against the exact shape the `autonomous_daily_coverage_bound` event type
/// is defined to have for `operation_id` (binding contract §6a): `id ==
/// autonomous_daily_coverage_bound:{operation_id}`, `event_type ==
/// autonomous_daily_coverage_bound`, `run_id IS NULL` (operation-scoped, not
/// run-scoped), `resume_source IS NULL`, `source ==
/// mqk-daemon.autonomous_daily_coordinator`. Never inspects `detail` --
/// payload parsing and semantic comparison are separate, later stages.
pub fn validate_coverage_authority_envelope(
    operation_id: Uuid,
    row: &mqk_db::AutonomousSessionEventRow,
) -> Result<(), CoverageAuthorityEnvelopeError> {
    if row.id != coverage_bound_event_id(operation_id) {
        return Err(CoverageAuthorityEnvelopeError::WrongId);
    }
    if row.event_type != EVENT_TYPE_COVERAGE_BOUND {
        return Err(CoverageAuthorityEnvelopeError::WrongEventType);
    }
    if row.run_id.is_some() {
        return Err(CoverageAuthorityEnvelopeError::RunIdNotNull);
    }
    if row.resume_source.is_some() {
        return Err(CoverageAuthorityEnvelopeError::ResumeSourceNotNull);
    }
    if row.source != COVERAGE_SOURCE {
        return Err(CoverageAuthorityEnvelopeError::WrongSource);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage A / Stage B split (E2A closure REPAIR 3). Stage A -- authority
// presence and envelope -- requires no current assignment, runtime binding,
// strategy registry, grace configuration, provider registry, or instrument
// registry: it is exactly "does a correctly-shaped authority event exist for
// this operation, and what does it durably say." Stage B -- current-policy
// comparison -- runs only after Stage A succeeds, using the one loaded typed
// authority value Stage A already parsed; it is never re-read or re-parsed
// through a second algorithm.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
// Called once per adapter/coordinator tick, never in a hot per-row loop;
// boxing the payload would only add an allocation for no measurable benefit
// here.
#[allow(clippy::large_enum_variant)]
pub enum CoverageAuthorityEnvelopeCheck {
    /// Row exists, envelope is exactly correct, detail parsed without error,
    /// and the parsed payload's own `operation_id` matches. This is the only
    /// variant carrying a usable authority value.
    Present(CoverageBoundDetail),
    /// No exact-id row exists.
    NotBound,
    /// A row exists with a correct envelope, but its `detail` failed to
    /// parse (malformed JSON, duplicate/unknown/missing/wrong-typed field,
    /// or an unknown schema version).
    Unreadable,
    /// A row exists but its envelope (id/event_type/source/run_id/
    /// resume_source) does not match the exact shape this event type
    /// requires, or its parsed payload's own `operation_id` disagrees with
    /// the operation it is stored under.
    Invalid,
}

/// Stage A: fetch the exact-id row for `operation_id`, validate its complete
/// envelope, parse its `detail` with the duplicate-safe typed parser, and
/// verify the parsed payload's own `operation_id` agrees with the row it was
/// found under. Read-only -- never writes. Requires no policy resolution of
/// any kind; a caller with only `operation_id` in hand can run this stage.
pub async fn check_coverage_authority_envelope(
    pool: &PgPool,
    operation_id: Uuid,
) -> anyhow::Result<CoverageAuthorityEnvelopeCheck> {
    let id = coverage_bound_event_id(operation_id);
    let Some(row) = mqk_db::fetch_autonomous_session_event_by_id(pool, &id).await? else {
        return Ok(CoverageAuthorityEnvelopeCheck::NotBound);
    };
    if validate_coverage_authority_envelope(operation_id, &row).is_err() {
        return Ok(CoverageAuthorityEnvelopeCheck::Invalid);
    }
    match parse_coverage_bound_detail(&row.detail) {
        Err(_) => Ok(CoverageAuthorityEnvelopeCheck::Unreadable),
        Ok(existing) if existing.operation_id != operation_id => {
            Ok(CoverageAuthorityEnvelopeCheck::Invalid)
        }
        Ok(existing) => Ok(CoverageAuthorityEnvelopeCheck::Present(existing)),
    }
}

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
    /// Missing, envelope-invalid, unparseable, or unreadable after a write
    /// attempt.
    Unreadable,
}

/// Attempt to bind `fresh` as the operation's coverage authority: insert
/// (idempotent, `ON CONFLICT (id) DO NOTHING`), then perform one
/// authoritative exact-id re-read through the same Stage A envelope
/// validator every other authority path uses. Never reports success merely
/// because the insert call returned `Ok(())` -- only the re-read decides,
/// and that re-read must pass complete envelope validation, duplicate-safe
/// parsing, and operation-id verification before semantic equality is even
/// considered. A write error is swallowed here (never propagated as a hard
/// failure) because the re-read alone determines committed truth, exactly as
/// the write-failure/uncertain-commit contract requires; a DB error on the
/// re-read itself propagates, since no authority claim can be made without
/// it. `ON CONFLICT (id) DO NOTHING` means the original row already present
/// under this id is never overwritten by this or any later call.
pub async fn write_and_confirm_coverage_authority(
    pool: &PgPool,
    fresh: &CoverageBoundDetail,
    ts_utc: DateTime<Utc>,
) -> anyhow::Result<CoverageAuthorityEnsureResult> {
    let row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(fresh.operation_id),
        ts_utc,
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(fresh),
        run_id: None,
        source: COVERAGE_SOURCE.to_string(),
    };
    let _ = mqk_db::persist_autonomous_session_event(pool, &row).await;

    match check_coverage_authority_envelope(pool, fresh.operation_id).await? {
        CoverageAuthorityEnvelopeCheck::NotBound
        | CoverageAuthorityEnvelopeCheck::Unreadable
        | CoverageAuthorityEnvelopeCheck::Invalid => Ok(CoverageAuthorityEnsureResult::Unreadable),
        CoverageAuthorityEnvelopeCheck::Present(existing) if &existing == fresh => {
            Ok(CoverageAuthorityEnsureResult::Bound(existing))
        }
        CoverageAuthorityEnvelopeCheck::Present(_) => Ok(CoverageAuthorityEnsureResult::Conflict),
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

/// Stage B, composed with Stage A for callers that already have a `fresh`
/// payload in hand: read-only exact-id lookup plus envelope validation,
/// duplicate-safe parse, and identity verification (Stage A), then -- only
/// once Stage A succeeds -- semantic comparison against `fresh` (Stage B).
/// Never writes. The completed-bar adapter's and coordinator's per-tick
/// authority gate.
pub async fn check_coverage_authority(
    pool: &PgPool,
    operation_id: Uuid,
    fresh: &CoverageBoundDetail,
) -> anyhow::Result<CoverageAuthorityCheck> {
    match check_coverage_authority_envelope(pool, operation_id).await? {
        CoverageAuthorityEnvelopeCheck::NotBound => Ok(CoverageAuthorityCheck::NotBound),
        CoverageAuthorityEnvelopeCheck::Unreadable => Ok(CoverageAuthorityCheck::Unreadable),
        CoverageAuthorityEnvelopeCheck::Invalid => Ok(CoverageAuthorityCheck::Invalid),
        CoverageAuthorityEnvelopeCheck::Present(existing) => {
            if existing == *fresh {
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
