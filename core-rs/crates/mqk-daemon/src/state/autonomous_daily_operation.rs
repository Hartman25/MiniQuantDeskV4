//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-DURABLE-COORDINATION: canonical
//! daily session-plan resolver, deterministic identity derivation, and a
//! pure shared read-model projection over the `mqk-db`
//! `sys_autonomous_daily_operations` / `sys_autonomous_daily_operation_events`
//! store.
//!
//! Foundation only. Nothing in this module is called from
//! `session_controller.rs`'s runtime tick, `autonomous_bar_ticker.rs`, the
//! market-data scheduler, any HTTP route, or any GUI component in this
//! patch. No operation identity helper here reads env directly for
//! strategy-binding purposes, calls `bootstrap_with_effective_binding()`, or
//! creates a preview binding — every identity helper accepts an
//! already-resolved value supplied by its caller.
//!
//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01: the
//! production session-plan resolver
//! ([`resolve_autonomous_daily_session_plan_from_env`]) obtains its
//! authoritative exchange-calendar provider through the same shared
//! canonical context every Bundle 2 readiness surface uses
//! (`crate::daily_data_readiness::load_readiness_context_from_env`) — it
//! never constructs a calendar provider of its own. `lifecycle.rs`'s
//! `start_execution_runtime` now also consults this module's
//! [`FixedWindowOverrideConfig`] (via
//! [`resolve_fixed_window_override_config_from_env`]) as a narrow
//! shared-calendar blocker input: an invalid override configuration refuses
//! the start before any readiness evaluation, run creation, or
//! provider/broker call — it is never silently treated as absent.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::market_calendar::{
    self, CalendarCoverageState, MarketCalendarProvider, MarketSessionState,
};
use super::session_controller::SessionWindow;
use super::{MultiSymbolConfigSource, MultiSymbolRuntimeConfig};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;

// ---------------------------------------------------------------------------
// B.1 — Canonical session-plan types
// ---------------------------------------------------------------------------

/// Stable, bounded reason codes for a non-applicable or blocked session-plan
/// resolution. Never a free-form string at the API/DB boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousDailyPlanReason {
    Weekend,
    ExchangeHoliday,
    CalendarUnavailable,
    CalendarOutOfRange,
    CalendarInvalid,
    FixedWindowOverrideInvalid,
    SessionPlanInvalid,
}

impl AutonomousDailyPlanReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Weekend => "weekend",
            Self::ExchangeHoliday => "exchange_holiday",
            Self::CalendarUnavailable => "calendar_unavailable",
            Self::CalendarOutOfRange => "calendar_out_of_range",
            Self::CalendarInvalid => "calendar_invalid",
            Self::FixedWindowOverrideInvalid => "fixed_window_override_invalid",
            Self::SessionPlanInvalid => "session_plan_invalid",
        }
    }
}

/// Which authority produced an [`AutonomousDailySessionPlan`]'s
/// `session_open_utc`/`session_close_utc` boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousDailyScheduleSource {
    /// The authoritative exchange-calendar heuristic
    /// (`market_calendar::NyseWeekdaysProvider`) — no operator override
    /// configured.
    NyseWeekdaysHeuristic,
    /// A valid operator-configured fixed UTC window
    /// (`MQK_SESSION_START_HH_MM`/`MQK_SESSION_STOP_HH_MM`), applied only
    /// after authoritative trading-day applicability was already
    /// established.
    FixedWindowOverride,
}

impl AutonomousDailyScheduleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NyseWeekdaysHeuristic => "nyse_weekdays_heuristic",
            Self::FixedWindowOverride => "fixed_window_override",
        }
    }
}

/// Immutable, injected timing configuration for preopen/postclose offsets.
/// The resolver never reads the wall clock and never hardcodes these
/// offsets internally — tests must inject their own values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutonomousDailyPlanTiming {
    pub preopen_lead: Duration,
    pub postclose_finalize_delay: Duration,
}

impl AutonomousDailyPlanTiming {
    /// Production defaults: 30-minute preopen lead, 15-minute postclose
    /// finalization delay. Callers (and all tests) should treat this as one
    /// convenience constructor, not an implicit default threaded in behind
    /// their back.
    pub fn production_default() -> Self {
        Self {
            preopen_lead: Duration::minutes(30),
            postclose_finalize_delay: Duration::minutes(15),
        }
    }
}

/// One immutable session-plan snapshot, composed strictly over the shared
/// Bundle 2 calendar authority (`market_calendar::resolve_market_session_schedule`)
/// plus, optionally, a validated fixed-UTC-window override layered on top of
/// established trading-day applicability. Never a fourth independent
/// market-date/open/close calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousDailySessionPlan {
    pub market_date: String,
    pub session_open_utc: DateTime<Utc>,
    pub session_close_utc: DateTime<Utc>,
    pub calendar_source: String,
    pub calendar_coverage_state: String,
    pub schedule_source: AutonomousDailyScheduleSource,
    pub preopen_start_utc: DateTime<Utc>,
    pub postclose_finalize_utc: DateTime<Utc>,
    pub session_plan_identity: String,
}

/// Result of resolving one autonomous-daily session plan for a given instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousDailySessionPlanResolution {
    /// A real, applicable trading-day operation.
    Applicable(AutonomousDailySessionPlan),
    /// The date is not an applicable trading day at all (weekend or
    /// exchange holiday) — never treated as a blocked/degraded operation,
    /// just "no operation today".
    NotApplicable {
        market_date: String,
        reason_code: AutonomousDailyPlanReason,
    },
    /// Applicability could not be established, or a configured override is
    /// invalid. `market_date` is `None` only when the underlying calendar
    /// classification itself failed before a market date could be derived.
    Blocked {
        market_date: Option<String>,
        reason_code: AutonomousDailyPlanReason,
        detail: String,
    },
}

// ---------------------------------------------------------------------------
// B.2 — Fixed-window override: three-way absent/valid/invalid config truth
// ---------------------------------------------------------------------------

/// Three-way truth for the optional fixed-UTC-window override
/// (`MQK_SESSION_START_HH_MM`/`MQK_SESSION_STOP_HH_MM`), distinguishing
/// "operator did not configure an override" from "operator configured an
/// override, but it is invalid" — the existing
/// `session_controller::session_window_from_env` conflates both into a bare
/// `None`, silently falling back to the authoritative schedule either way.
/// This resolver must not hide a configuration defect behind that same
/// silent fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedWindowOverrideConfig {
    /// Neither env var is set.
    Absent,
    /// Both env vars are set and parse to a valid `start < stop` window.
    Valid(SessionWindow),
    /// At least one env var is set, but the configuration is defective:
    /// only one of the two variables is present, a time is malformed, or
    /// `start >= stop`.
    Invalid { detail: String },
}

/// Read [`super::session_controller::SESSION_START_HH_MM_ENV`] /
/// `SESSION_STOP_HH_MM_ENV` directly (not through
/// `session_controller::session_window_from_env`, which cannot distinguish
/// absent from invalid) and classify the three-way override truth.
pub fn resolve_fixed_window_override_config_from_env() -> FixedWindowOverrideConfig {
    let start = std::env::var(super::session_controller::SESSION_START_HH_MM_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    let stop = std::env::var(super::session_controller::SESSION_STOP_HH_MM_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());

    match (start, stop) {
        (None, None) => FixedWindowOverrideConfig::Absent,
        (Some(_), None) => FixedWindowOverrideConfig::Invalid {
            detail: format!(
                "{} is set but {} is not; both are required",
                super::session_controller::SESSION_START_HH_MM_ENV,
                super::session_controller::SESSION_STOP_HH_MM_ENV
            ),
        },
        (None, Some(_)) => FixedWindowOverrideConfig::Invalid {
            detail: format!(
                "{} is set but {} is not; both are required",
                super::session_controller::SESSION_STOP_HH_MM_ENV,
                super::session_controller::SESSION_START_HH_MM_ENV
            ),
        },
        (Some(start), Some(stop)) => match SessionWindow::parse(&start, &stop) {
            Some(window) => FixedWindowOverrideConfig::Valid(window),
            None => FixedWindowOverrideConfig::Invalid {
                detail: format!(
                    "malformed override or start >= stop (start='{start}', stop='{stop}')"
                ),
            },
        },
    }
}

fn format_market_date(date: (i64, i64, i64)) -> String {
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

fn coverage_state_str(state: CalendarCoverageState) -> &'static str {
    match state {
        CalendarCoverageState::Active => "active",
        CalendarCoverageState::Stale => "stale",
        CalendarCoverageState::Invalid => "invalid",
        CalendarCoverageState::OutOfRange => "out_of_range",
        CalendarCoverageState::Unknown => "unknown",
    }
}

// ---------------------------------------------------------------------------
// B.2 — Session-plan resolution
// ---------------------------------------------------------------------------

/// Resolve one [`AutonomousDailySessionPlanResolution`] for `now_utc`,
/// against an already-selected authoritative exchange calendar `provider`
/// and an already-classified `override_config` — fully injectable, no env
/// reads, no wall-clock reads. Production callers should use
/// [`resolve_autonomous_daily_session_plan_from_env`] instead.
///
/// Required ordering (binding contract §13 Correction 1): resolve
/// authoritative exchange market-date/calendar truth -> require `Active`
/// coverage -> require trading day -> optionally apply valid fixed UTC
/// boundaries -> derive preopen/postclose values -> derive
/// `session_plan_identity`. This composes exclusively over
/// `market_calendar::resolve_market_session_schedule` — no second
/// ET/UTC conversion, holiday table, or weekday calculation.
pub fn resolve_autonomous_daily_session_plan(
    provider: &dyn MarketCalendarProvider,
    now_utc: DateTime<Utc>,
    timing: &AutonomousDailyPlanTiming,
    override_config: &FixedWindowOverrideConfig,
) -> AutonomousDailySessionPlanResolution {
    let schedule = market_calendar::resolve_market_session_schedule(provider, now_utc);
    let market_date_str = format_market_date(schedule.market_date);

    match schedule.coverage_state {
        CalendarCoverageState::OutOfRange => {
            return AutonomousDailySessionPlanResolution::Blocked {
                market_date: Some(market_date_str),
                reason_code: AutonomousDailyPlanReason::CalendarOutOfRange,
                detail: "market_date falls outside the declared calendar coverage window"
                    .to_string(),
            };
        }
        CalendarCoverageState::Invalid => {
            return AutonomousDailySessionPlanResolution::Blocked {
                market_date: Some(market_date_str),
                reason_code: AutonomousDailyPlanReason::CalendarInvalid,
                detail: "calendar source reported structurally invalid data".to_string(),
            };
        }
        CalendarCoverageState::Stale | CalendarCoverageState::Unknown => {
            return AutonomousDailySessionPlanResolution::Blocked {
                market_date: Some(market_date_str),
                reason_code: AutonomousDailyPlanReason::CalendarUnavailable,
                detail: "calendar provider could not establish authoritative session truth"
                    .to_string(),
            };
        }
        CalendarCoverageState::Active => {}
    }

    if !schedule.is_trading_day {
        let truth = provider.session_for(now_utc);
        let reason_code = if truth.state == MarketSessionState::Holiday {
            AutonomousDailyPlanReason::ExchangeHoliday
        } else {
            AutonomousDailyPlanReason::Weekend
        };
        return AutonomousDailySessionPlanResolution::NotApplicable {
            market_date: market_date_str,
            reason_code,
        };
    }

    let (session_open_utc, session_close_utc, schedule_source) = match override_config {
        FixedWindowOverrideConfig::Invalid { detail } => {
            return AutonomousDailySessionPlanResolution::Blocked {
                market_date: Some(market_date_str),
                reason_code: AutonomousDailyPlanReason::FixedWindowOverrideInvalid,
                detail: detail.clone(),
            };
        }
        FixedWindowOverrideConfig::Absent => (
            schedule.session_open_utc,
            schedule.session_close_utc,
            AutonomousDailyScheduleSource::NyseWeekdaysHeuristic,
        ),
        FixedWindowOverrideConfig::Valid(window) => {
            let midnight_ts = market_calendar::midnight_utc_ts_for_date(schedule.market_date);
            let open_ts =
                midnight_ts + (window.start_hh as i64) * 3600 + (window.start_mm as i64) * 60;
            let close_ts =
                midnight_ts + (window.stop_hh as i64) * 3600 + (window.stop_mm as i64) * 60;
            match (
                DateTime::<Utc>::from_timestamp(open_ts, 0),
                DateTime::<Utc>::from_timestamp(close_ts, 0),
            ) {
                (Some(open), Some(close)) if open < close => (
                    open,
                    close,
                    AutonomousDailyScheduleSource::FixedWindowOverride,
                ),
                _ => {
                    return AutonomousDailySessionPlanResolution::Blocked {
                        market_date: Some(market_date_str),
                        reason_code: AutonomousDailyPlanReason::FixedWindowOverrideInvalid,
                        detail: "fixed window override produced an invalid open/close instant"
                            .to_string(),
                    };
                }
            }
        }
    };

    let preopen_start_utc = session_open_utc - timing.preopen_lead;
    let postclose_finalize_utc = session_close_utc + timing.postclose_finalize_delay;

    if session_open_utc >= session_close_utc
        || preopen_start_utc > session_open_utc
        || postclose_finalize_utc < session_close_utc
    {
        return AutonomousDailySessionPlanResolution::Blocked {
            market_date: Some(market_date_str),
            reason_code: AutonomousDailyPlanReason::SessionPlanInvalid,
            detail: "derived session-plan boundaries violate the required ordering invariant"
                .to_string(),
        };
    }

    let calendar_source = schedule.calendar_source.to_string();
    let calendar_coverage_state = coverage_state_str(schedule.coverage_state).to_string();

    let session_plan_identity = compute_session_plan_identity(
        &market_date_str,
        session_open_utc,
        session_close_utc,
        &calendar_source,
        &calendar_coverage_state,
        schedule_source.as_str(),
        preopen_start_utc,
        postclose_finalize_utc,
    );

    AutonomousDailySessionPlanResolution::Applicable(AutonomousDailySessionPlan {
        market_date: market_date_str,
        session_open_utc,
        session_close_utc,
        calendar_source,
        calendar_coverage_state,
        schedule_source,
        preopen_start_utc,
        postclose_finalize_utc,
        session_plan_identity,
    })
}

/// Production entry point: obtains the authoritative exchange-calendar
/// provider through the exact shared canonical context every Bundle 2
/// readiness surface already uses
/// (`crate::daily_data_readiness::load_readiness_context_from_env`) and the
/// current [`FixedWindowOverrideConfig`] from env, then delegates to
/// [`resolve_autonomous_daily_session_plan`]. This wrapper must never
/// construct its own calendar provider — doing so would create a second,
/// independent provider-selection policy that could silently disagree with
/// Bundle 2's readiness composition under the same configuration
/// (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01).
/// No-override behavior is therefore byte-for-byte identical to the Bundle 2
/// authoritative schedule for the same `now_utc`.
pub fn resolve_autonomous_daily_session_plan_from_env(
    now_utc: DateTime<Utc>,
    timing: &AutonomousDailyPlanTiming,
) -> AutonomousDailySessionPlanResolution {
    let context = crate::daily_data_readiness::load_readiness_context_from_env();
    let override_config = resolve_fixed_window_override_config_from_env();
    resolve_autonomous_daily_session_plan(
        context.calendar_provider.as_ref(),
        now_utc,
        timing,
        &override_config,
    )
}

// ---------------------------------------------------------------------------
// B.3 — Session-plan identity
// ---------------------------------------------------------------------------

/// Deterministic, bounded identity for one [`AutonomousDailySessionPlan`]:
/// lowercase SHA-256 hex over a versioned, pipe-delimited canonical string of
/// every immutable plan field. Same inputs always produce the same identity;
/// any field change produces a different one. No random UUID, no
/// locale-dependent date formatting (RFC3339 throughout), no debug-format
/// identity, no environment dump.
#[allow(clippy::too_many_arguments)]
fn compute_session_plan_identity(
    market_date: &str,
    session_open_utc: DateTime<Utc>,
    session_close_utc: DateTime<Utc>,
    calendar_source: &str,
    calendar_coverage_state: &str,
    schedule_source: &str,
    preopen_start_utc: DateTime<Utc>,
    postclose_finalize_utc: DateTime<Utc>,
) -> String {
    let canonical = format!(
        "mqk.autonomous-daily-session-plan.v1|{}|{}|{}|{}|{}|{}|{}|{}",
        market_date,
        session_open_utc.to_rfc3339(),
        session_close_utc.to_rfc3339(),
        calendar_source,
        calendar_coverage_state,
        schedule_source,
        preopen_start_utc.to_rfc3339(),
        postclose_finalize_utc.to_rfc3339(),
    );
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// B.4 — Assignment and runtime-binding identities
// ---------------------------------------------------------------------------

/// Pure assignment identity over an already-resolved
/// [`MultiSymbolRuntimeConfig`]. Preserves `config.symbols`' canonical
/// dispatch order (never sorted — configured order is behaviorally
/// meaningful, per current multi-symbol dispatch source) and includes the
/// config source label, since a different source (env fallback vs. a
/// specific watchlist artifact path) is itself a meaningful identity
/// component.
pub fn derive_assignment_identity(config: &MultiSymbolRuntimeConfig) -> String {
    let source_label = match &config.source {
        MultiSymbolConfigSource::EnvSingleSymbolFallback => {
            "env_single_symbol_fallback".to_string()
        }
        MultiSymbolConfigSource::WatchlistArtifactV2 { path } => {
            format!("watchlist_v2:{}", path.trim())
        }
    };
    let assignments: Vec<String> = config
        .symbols
        .iter()
        .map(|a| {
            format!(
                "{}|{}|{}",
                a.symbol.trim().to_ascii_uppercase(),
                a.strategy_id.trim(),
                a.timeframe.trim()
            )
        })
        .collect();
    format!(
        "mqk.autonomous-daily-assignment.v1|{}|{}",
        source_label,
        assignments.join(";")
    )
}

/// Pure runtime-binding identity over an already-resolved
/// [`EffectiveRuntimeBinding`]. Each field is tagged `none`/`some:<value>` so
/// `None` is never conflated with `Some("")`.
pub fn derive_runtime_binding_identity(binding: &EffectiveRuntimeBinding) -> String {
    let strategy = match &binding.effective_runtime_strategy_id {
        Some(s) => format!("some:{}", s.trim()),
        None => "none".to_string(),
    };
    let symbol = match &binding.effective_runtime_target_symbol {
        Some(s) => format!("some:{}", s.trim().to_ascii_uppercase()),
        None => "none".to_string(),
    };
    let timeframe = match binding.effective_runtime_timeframe_secs {
        Some(secs) => format!("some:{secs}"),
        None => "none".to_string(),
    };
    format!(
        "mqk.autonomous-daily-runtime-binding.v1|{}|{}|{}",
        strategy, symbol, timeframe
    )
}

// ---------------------------------------------------------------------------
// B.5 — Operation identity
// ---------------------------------------------------------------------------

/// Deterministic UUIDv5 operation identity over all six identity components.
/// Never `Uuid::new_v4()`, never a current timestamp, never a process-local
/// sequence — two repeated resolutions for the same immutable identity
/// always return the same operation id.
pub fn derive_autonomous_daily_operation_id(
    plan: &AutonomousDailySessionPlan,
    deployment_mode: &str,
    adapter_id: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
) -> Uuid {
    let seed = format!(
        "mqk.autonomous-daily-operation.v1|{}|{}|{}|{}|{}|{}",
        plan.market_date,
        deployment_mode.trim(),
        adapter_id.trim(),
        plan.session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())
}

// ---------------------------------------------------------------------------
// B.9 — Shared, pure daemon read-model projection
// ---------------------------------------------------------------------------

/// One projected transition-history entry (mirrors
/// `mqk_db::AutonomousDailyOperationEventRecord`, minus the redundant
/// `operation_id` already carried by the enclosing
/// [`AutonomousDailyOperationReadModel`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousDailyOperationEventReadModel {
    pub transition_seq: i64,
    pub from_state: String,
    pub to_state: String,
    pub reason_code: Option<String>,
    pub occurred_at_utc: DateTime<Utc>,
    pub run_id: Option<Uuid>,
    pub bounded_detail: String,
}

/// Bounded projection of one `sys_autonomous_daily_operations` row (plus its
/// optionally-requested transition history) for daemon-internal consumption.
/// Pure: built entirely from already-fetched DB records, performs no I/O.
/// Unknown nullable DB values remain `None` here; explicit stored zero
/// counters remain zero — neither is ever coerced into the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomousDailyOperationReadModel {
    pub operation_id: Uuid,
    pub market_date: String,
    pub deployment_mode: String,
    pub adapter_id: String,
    pub session_plan_identity: String,
    pub assignment_identity: String,
    pub runtime_binding_identity: String,
    pub calendar_source: String,
    pub calendar_coverage_state: String,
    pub schedule_source: String,
    pub session_open_utc: DateTime<Utc>,
    pub session_close_utc: DateTime<Utc>,
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
    /// `None` when transition history was not requested/loaded; `Some(&[])`
    /// would be a data-integrity defect (every operation has at least its
    /// initial event) and is never produced by a correct caller.
    pub recent_events: Option<Vec<AutonomousDailyOperationEventReadModel>>,
}

/// Project one `mqk_db::AutonomousDailyOperationRecord` (plus optional
/// already-fetched events) onto the daemon-internal read model. Pure — no
/// DB access, no write of any kind.
pub fn project_autonomous_daily_operation_read_model(
    record: &mqk_db::AutonomousDailyOperationRecord,
    events: Option<&[mqk_db::AutonomousDailyOperationEventRecord]>,
) -> AutonomousDailyOperationReadModel {
    AutonomousDailyOperationReadModel {
        operation_id: record.operation_id,
        market_date: record.market_date.format("%Y-%m-%d").to_string(),
        deployment_mode: record.deployment_mode.clone(),
        adapter_id: record.adapter_id.clone(),
        session_plan_identity: record.session_plan_identity.clone(),
        assignment_identity: record.assignment_identity.clone(),
        runtime_binding_identity: record.runtime_binding_identity.clone(),
        calendar_source: record.calendar_source.clone(),
        calendar_coverage_state: record.calendar_coverage_state.clone(),
        schedule_source: record.schedule_source.clone(),
        session_open_utc: record.session_open_utc,
        session_close_utc: record.session_close_utc,
        preopen_start_utc: record.preopen_start_utc,
        postclose_finalize_utc: record.postclose_finalize_utc,
        state: record.state.clone(),
        state_reason_code: record.state_reason_code.clone(),
        state_version: record.state_version,
        run_id: record.run_id,
        start_attempt_count: record.start_attempt_count,
        last_start_attempt_utc: record.last_start_attempt_utc,
        next_retry_utc: record.next_retry_utc,
        data_refresh_state: record.data_refresh_state.clone(),
        last_provider_poll_utc: record.last_provider_poll_utc,
        provider_poll_attempt_count: record.provider_poll_attempt_count,
        provider_poll_success_count: record.provider_poll_success_count,
        provider_poll_failure_count: record.provider_poll_failure_count,
        last_completed_bar_ts: record.last_completed_bar_ts,
        last_dispatched_bar_ts: record.last_dispatched_bar_ts,
        bars_observed: record.bars_observed,
        bars_dispatched: record.bars_dispatched,
        started_at_utc: record.started_at_utc,
        stopped_at_utc: record.stopped_at_utc,
        finalized_at_utc: record.finalized_at_utc,
        outcome: record.outcome.clone(),
        no_trade_reason: record.no_trade_reason.clone(),
        last_error: record.last_error.clone(),
        created_at_utc: record.created_at_utc,
        updated_at_utc: record.updated_at_utc,
        recent_events: events.map(|evs| {
            evs.iter()
                .map(|e| AutonomousDailyOperationEventReadModel {
                    transition_seq: e.transition_seq,
                    from_state: e.from_state.clone(),
                    to_state: e.to_state.clone(),
                    reason_code: e.reason_code.clone(),
                    occurred_at_utc: e.occurred_at_utc,
                    run_id: e.run_id,
                    bounded_detail: e.bounded_detail.clone(),
                })
                .collect()
        }),
    }
}
