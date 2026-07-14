//! DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED Phase B: strict daily data
//! readiness evaluator.
//!
//! Binding contract:
//! `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md`.
//!
//! This module is additive alongside the legacy advisory evaluator
//! (`crate::market_data_freshness`) — it does not replace or modify it.
//! Foundation/evaluator only: **not** wired into
//! `AppState::start_execution_runtime`, any route, the scheduler, or durable
//! evidence in this phase. [`evaluate_daily_data_readiness_from_env`] exists
//! so a future phase can call it, but nothing in this codebase calls it yet.
//!
//! # Evaluation order (per assignment)
//! 1. assignment resolution (canonical `build_multi_symbol_runtime_config_from_env`)
//! 2. effective bootstrap binding resolution (immutable snapshot, one env read)
//! 3. strategy-ID / target-symbol / timeframe binding compatibility (evaluated
//!    independently — all three are always checked, never short-circuited)
//! 4. strategy history requirement (from the strategy that will *actually* run)
//! 5. asset class (canonical v1 instrument registry, never defaulted to equity)
//! 6. provider capability (config-level: can any enabled provider serve this
//!    asset class + timeframe combination at all?)
//! 7. calendar/session (typed `MarketSessionSchedule`, its own coverage check)
//! 8. bounded bar readiness (provenance + continuity, DB-bounded)

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_md::instrument_registry::TrackedInstrument;
use mqk_md::provider_registry::ProviderConfig;
use mqk_md::Timeframe;
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;

use crate::state::market_calendar::{
    self, resolve_market_session_schedule, CalendarCoverageState, MarketCalendarProvider,
    MarketSessionSchedule,
};
use crate::state::{MultiSymbolRuntimeConfig, SymbolStrategyAssignment};

// ---------------------------------------------------------------------------
// Stable blocking reason codes
// ---------------------------------------------------------------------------

pub const REASON_REQUIRED_ASSIGNMENTS_MISSING: &str = "required_assignments_missing";
pub const REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH: &str =
    "runtime_strategy_assignment_mismatch";
pub const REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH: &str =
    "runtime_strategy_symbol_binding_mismatch";
pub const REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH: &str = "runtime_strategy_timeframe_mismatch";
pub const REASON_STRATEGY_REQUIREMENT_UNKNOWN: &str = "strategy_requirement_unknown";
pub const REASON_ASSET_CLASS_UNKNOWN: &str = "asset_class_unknown";
pub const REASON_PROVIDER_PROVENANCE_INVALID: &str = "provider_provenance_invalid";
pub const REASON_PROVIDER_DISABLED: &str = "provider_disabled";
pub const REASON_PROVIDER_CAPABILITY_MISMATCH: &str = "provider_capability_mismatch";
pub const REASON_CALENDAR_UNAVAILABLE: &str = "calendar_unavailable";
pub const REASON_UNSUPPORTED_TIMEFRAME: &str = "unsupported_timeframe";
pub const REASON_UNSUPPORTED_INTRADAY_CONTINUITY: &str = "unsupported_intraday_continuity";
pub const REASON_MARKET_DATA_MISSING: &str = "market_data_missing";
pub const REASON_INSUFFICIENT_HISTORY: &str = "insufficient_history";
pub const REASON_DUPLICATE_TIMESTAMP: &str = "duplicate_timestamp";
pub const REASON_INTERIOR_GAP: &str = "interior_gap";
pub const REASON_LATEST_BAR_FUTURE: &str = "latest_bar_future";
pub const REASON_EXPECTED_LATEST_BAR_MISSING: &str = "expected_latest_bar_missing";

// ---------------------------------------------------------------------------
// Grace / future-skew configuration (§8/§9)
// ---------------------------------------------------------------------------

pub const GRACE_SECS_ENV: &str = "MQK_DATA_READINESS_GRACE_SECS";
pub const FUTURE_SKEW_SECS_ENV: &str = "MQK_DATA_READINESS_FUTURE_SKEW_SECS";
const DEFAULT_GRACE_SECS: i64 = 900;
const DEFAULT_FUTURE_SKEW_SECS: i64 = 300;

/// Configured grace ceiling from env, fail-closed to the default on
/// absent/invalid/negative input (mirrors
/// `market_data_freshness::intraday_bar_max_age_secs_from_env`).
pub fn configured_grace_seconds_from_env() -> i64 {
    std::env::var(GRACE_SECS_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n >= 0)
        .unwrap_or(DEFAULT_GRACE_SECS)
}

/// Configured future-skew ceiling from env, same fail-closed pattern.
pub fn configured_future_skew_seconds_from_env() -> i64 {
    std::env::var(FUTURE_SKEW_SECS_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n >= 0)
        .unwrap_or(DEFAULT_FUTURE_SKEW_SECS)
}

/// `effective_grace_seconds = min(configured_grace_seconds, timeframe.duration_secs())` (§8).
pub fn effective_grace_seconds(configured_grace_seconds: i64, timeframe_secs: i64) -> i64 {
    configured_grace_seconds.min(timeframe_secs)
}

/// `effective_future_skew_seconds = min(configured_future_skew_seconds, 60, timeframe.duration_secs())` (§9).
pub fn effective_future_skew_seconds(
    configured_future_skew_seconds: i64,
    timeframe_secs: i64,
) -> i64 {
    configured_future_skew_seconds.min(60).min(timeframe_secs)
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// Per-assignment readiness identity + verdict (§3c).
#[derive(Debug, Clone)]
pub struct AssignmentReadiness {
    pub assignment_symbol: String,
    pub assignment_timeframe: String,
    pub configured_strategy_id: String,
    pub effective_runtime_strategy_id: Option<String>,
    pub effective_runtime_target_symbol: Option<String>,
    pub effective_runtime_timeframe_secs: Option<i64>,
    pub required_history_bars: Option<usize>,
    pub asset_class: Option<String>,
    /// `"ready"` | `"blocked"` | `"db_unavailable"` | `"query_failed"`.
    pub readiness_state: &'static str,
    pub blockers: Vec<&'static str>,
    pub configured_grace_seconds: i64,
    pub effective_grace_seconds: i64,
    pub configured_future_skew_seconds: i64,
    pub effective_future_skew_seconds: i64,
}

impl AssignmentReadiness {
    pub fn is_ready(&self) -> bool {
        self.readiness_state == "ready"
    }
}

/// Aggregate readiness across every assignment.
#[derive(Debug, Clone)]
pub struct DailyDataReadinessReport {
    pub start_allowed: bool,
    /// `"ready"` | `"blocked"`.
    pub aggregate_state: &'static str,
    /// Set only when assignment resolution itself failed — no per-assignment
    /// evaluation was possible at all.
    pub top_level_blocker: Option<&'static str>,
    pub assignments: Vec<AssignmentReadiness>,
}

fn push_unique(blockers: &mut Vec<&'static str>, reason: &'static str) {
    if !blockers.contains(&reason) {
        blockers.push(reason);
    }
}

fn blocked_report(reason: &'static str) -> DailyDataReadinessReport {
    DailyDataReadinessReport {
        start_allowed: false,
        aggregate_state: "blocked",
        top_level_blocker: Some(reason),
        assignments: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Per-assignment evaluation
// ---------------------------------------------------------------------------

/// Evaluate one [`SymbolStrategyAssignment`] against the effective runtime
/// binding, strategy registry, provider registry, instrument registry, and
/// calendar — the production evaluator path (also used by
/// [`evaluate_daily_data_readiness_from_env`], not a test-only reimplementation).
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_assignment(
    db: Option<&PgPool>,
    assignment: &SymbolStrategyAssignment,
    binding: &EffectiveRuntimeBinding,
    calendar_provider: &dyn MarketCalendarProvider,
    provider_configs: &[ProviderConfig],
    instruments: &[TrackedInstrument],
    strategy_registry: &PluginRegistry,
    now_utc: DateTime<Utc>,
) -> AssignmentReadiness {
    let mut blockers: Vec<&'static str> = Vec::new();

    let parsed_timeframe = Timeframe::parse(&assignment.timeframe);
    if parsed_timeframe.is_err() {
        push_unique(&mut blockers, REASON_UNSUPPORTED_TIMEFRAME);
    }

    // --- Runtime-binding checks (§3a/§3b/§3c) — independent; all three are
    // always evaluated, never short-circuited after the first failure.
    if binding.effective_runtime_strategy_id.as_deref() != Some(assignment.strategy_id.as_str()) {
        push_unique(&mut blockers, REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH);
    }
    let symbol_matches = binding
        .effective_runtime_target_symbol
        .as_deref()
        .map(|s| s.trim().eq_ignore_ascii_case(assignment.symbol.trim()))
        .unwrap_or(false);
    if !symbol_matches {
        push_unique(
            &mut blockers,
            REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH,
        );
    }
    if let Ok(tf) = &parsed_timeframe {
        if binding.effective_runtime_timeframe_secs != Some(tf.duration_secs()) {
            push_unique(&mut blockers, REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH);
        }
    }

    // --- Strategy history requirement (§5) — from the strategy that will
    // actually run, never the configured-but-not-running id.
    let required_history_bars = match &binding.effective_runtime_strategy_id {
        Some(id) => match strategy_registry
            .lookup(id)
            .ok()
            .and_then(|meta| meta.data_requirements.clone())
        {
            Some(req) => Some(req.minimum_completed_bars),
            None => {
                push_unique(&mut blockers, REASON_STRATEGY_REQUIREMENT_UNKNOWN);
                None
            }
        },
        None => None,
    };

    // --- Asset class (§11a) — canonical v1 instrument registry only.
    let asset_class = instruments
        .iter()
        .find(|i| {
            i.symbol
                .trim()
                .eq_ignore_ascii_case(assignment.symbol.trim())
        })
        .map(|i| i.trading_asset_class().to_string())
        .filter(|ac| !ac.trim().is_empty());
    if asset_class.is_none() {
        push_unique(&mut blockers, REASON_ASSET_CLASS_UNKNOWN);
    }

    // --- Provider capability pre-check (§11/§13) — config-only, no DB: does
    // any enabled provider support this (asset_class, timeframe) at all?
    let mut provider_capability_ok = false;
    if let (Ok(tf), Some(ac)) = (&parsed_timeframe, &asset_class) {
        provider_capability_ok = provider_configs
            .iter()
            .any(|p| p.enabled && p.supports_asset_class(ac) && p.supports_timeframe(tf.as_str()));
        if !provider_capability_ok {
            push_unique(&mut blockers, REASON_PROVIDER_CAPABILITY_MISMATCH);
        }
    }

    // --- Calendar/session (§6a) — pure, no DB.
    let schedule = resolve_market_session_schedule(calendar_provider, now_utc);
    if schedule.coverage_state != CalendarCoverageState::Active {
        push_unique(&mut blockers, REASON_CALENDAR_UNAVAILABLE);
    }

    let configured_grace = configured_grace_seconds_from_env();
    let configured_skew = configured_future_skew_seconds_from_env();
    let (eff_grace, eff_skew) = match &parsed_timeframe {
        Ok(tf) => (
            effective_grace_seconds(configured_grace, tf.duration_secs()),
            effective_future_skew_seconds(configured_skew, tf.duration_secs()),
        ),
        Err(_) => (configured_grace, configured_skew),
    };

    let binding_ok = !blockers.contains(&REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH)
        && !blockers.contains(&REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH)
        && !blockers.contains(&REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH);

    let can_proceed_to_bar_stage = binding_ok
        && required_history_bars.is_some()
        && parsed_timeframe.is_ok()
        && asset_class.is_some()
        && provider_capability_ok
        && schedule.coverage_state == CalendarCoverageState::Active;

    let readiness_state: &'static str = if !can_proceed_to_bar_stage {
        "blocked"
    } else {
        let tf = *parsed_timeframe.as_ref().expect("checked Ok above");
        let required = required_history_bars.expect("checked Some above");
        match db {
            None => "db_unavailable",
            Some(pool) => {
                let bound = (required as i64) + 2;
                let query_symbol = assignment.symbol.trim().to_uppercase();
                match mqk_db::md::fetch_bounded_bars_with_provenance(
                    pool,
                    &query_symbol,
                    tf.as_str(),
                    bound,
                )
                .await
                {
                    Err(_) => "query_failed",
                    Ok(rows) => {
                        let bar_blockers = evaluate_bar_readiness(
                            &rows,
                            tf,
                            &schedule,
                            calendar_provider,
                            required,
                            now_utc.timestamp(),
                            eff_grace,
                            eff_skew,
                            provider_configs,
                            asset_class.as_deref().unwrap_or(""),
                        );
                        for b in bar_blockers {
                            push_unique(&mut blockers, b);
                        }
                        if blockers.is_empty() {
                            "ready"
                        } else {
                            "blocked"
                        }
                    }
                }
            }
        }
    };

    AssignmentReadiness {
        assignment_symbol: assignment.symbol.clone(),
        assignment_timeframe: assignment.timeframe.clone(),
        configured_strategy_id: assignment.strategy_id.clone(),
        effective_runtime_strategy_id: binding.effective_runtime_strategy_id.clone(),
        effective_runtime_target_symbol: binding.effective_runtime_target_symbol.clone(),
        effective_runtime_timeframe_secs: binding.effective_runtime_timeframe_secs,
        required_history_bars,
        asset_class,
        readiness_state,
        blockers,
        configured_grace_seconds: configured_grace,
        effective_grace_seconds: eff_grace,
        configured_future_skew_seconds: configured_skew,
        effective_future_skew_seconds: eff_skew,
    }
}

// ---------------------------------------------------------------------------
// Bar-level readiness (§9/§10/§11)
// ---------------------------------------------------------------------------

/// Bar-level provenance + continuity checks (§9/§10/§11), exposed so tests
/// can exercise this exact production sub-check directly with injected bar
/// fixtures — not a separate test-only reimplementation.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_bar_readiness(
    rows: &[mqk_db::md::MdBarRowWithProvenance],
    timeframe: Timeframe,
    schedule: &MarketSessionSchedule,
    calendar_provider: &dyn MarketCalendarProvider,
    required_history_bars: usize,
    now_ts: i64,
    effective_grace_seconds: i64,
    effective_future_skew_seconds: i64,
    provider_configs: &[ProviderConfig],
    asset_class: &str,
) -> Vec<&'static str> {
    let mut blockers: Vec<&'static str> = Vec::new();

    if rows.is_empty() {
        blockers.push(REASON_MARKET_DATA_MISSING);
        return blockers;
    }

    // Strict ascending order / no duplicate end_ts across every returned row.
    for w in rows.windows(2) {
        if w[0].end_ts >= w[1].end_ts {
            push_unique(&mut blockers, REASON_DUPLICATE_TIMESTAMP);
        }
    }

    // No material future rows (§9).
    let future_cutoff = now_ts + effective_future_skew_seconds;
    if rows.iter().any(|r| r.end_ts > future_cutoff) {
        push_unique(&mut blockers, REASON_LATEST_BAR_FUTURE);
    }

    // Provider provenance (§11) — every bar in the bounded window, not just the latest.
    for row in rows {
        if row.provider_id.trim().is_empty() || row.provider_id.eq_ignore_ascii_case("unknown") {
            push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID);
            continue;
        }
        match mqk_md::provider_registry::find_provider(provider_configs, &row.provider_id) {
            None => push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID),
            Some(cfg) if !cfg.enabled => push_unique(&mut blockers, REASON_PROVIDER_DISABLED),
            Some(cfg)
                if !cfg.supports_asset_class(asset_class)
                    || !cfg.supports_timeframe(timeframe.as_str()) =>
            {
                push_unique(&mut blockers, REASON_PROVIDER_CAPABILITY_MISMATCH)
            }
            Some(_) => {}
        }
    }

    let completed: Vec<&mqk_db::md::MdBarRowWithProvenance> =
        rows.iter().filter(|r| r.is_complete).collect();
    if completed.len() < required_history_bars {
        push_unique(&mut blockers, REASON_INSUFFICIENT_HISTORY);
    }

    // Continuity (§10) — full session-anchored proof for 1D/1m/5m only;
    // every other timeframe blocks honestly rather than passing on a weaker
    // count/order/duplicate/future-only proof.
    let expected = match timeframe {
        Timeframe::D1 => expected_daily_end_ts_window(
            calendar_provider,
            schedule,
            now_ts,
            effective_grace_seconds,
            required_history_bars,
        ),
        Timeframe::M1 | Timeframe::M5 => expected_intraday_end_ts_window(
            calendar_provider,
            schedule,
            now_ts,
            timeframe.duration_secs(),
            effective_grace_seconds,
            required_history_bars,
        ),
        Timeframe::H1 | Timeframe::M15 => {
            push_unique(&mut blockers, REASON_UNSUPPORTED_INTRADAY_CONTINUITY);
            None
        }
    };

    match expected {
        Some(expected_ts) => {
            let actual: HashSet<i64> = completed.iter().map(|r| r.end_ts).collect();
            let last_idx = expected_ts.len().saturating_sub(1);
            for (i, ts) in expected_ts.iter().enumerate() {
                if !actual.contains(ts) {
                    if i == last_idx {
                        push_unique(&mut blockers, REASON_EXPECTED_LATEST_BAR_MISSING);
                    } else {
                        push_unique(&mut blockers, REASON_INTERIOR_GAP);
                    }
                }
            }
        }
        None if !matches!(timeframe, Timeframe::H1 | Timeframe::M15) => {
            // Calendar walk could not produce an expected window at all
            // (bounded search exhausted) — fail closed.
            push_unique(&mut blockers, REASON_CALENDAR_UNAVAILABLE);
        }
        None => {}
    }

    blockers
}

/// Expected daily `end_ts` window (§6/§7): the last `required_history_bars`
/// real trading dates ending at the trading date whose row is currently
/// expected (today's, once past `session_close_utc + grace`; otherwise the
/// previous trading date), each mapped to its expected midnight-UTC label.
pub fn expected_daily_end_ts_window(
    provider: &dyn MarketCalendarProvider,
    schedule: &MarketSessionSchedule,
    now_ts: i64,
    effective_grace_seconds: i64,
    required_history_bars: usize,
) -> Option<Vec<i64>> {
    let last_expected_date =
        if now_ts >= schedule.session_close_utc.timestamp() + effective_grace_seconds {
            schedule.market_date
        } else {
            schedule.previous_trading_date
        };
    let dates = market_calendar::walk_back_trading_dates(
        provider,
        last_expected_date,
        required_history_bars,
    )?;
    Some(
        dates
            .into_iter()
            .map(market_calendar::midnight_utc_ts_for_date)
            .collect(),
    )
}

/// Session-open-anchored bar-start grid for one session (§6): `open, open +
/// interval, ..., ` up to (not including) any slot whose interval would run
/// past `session_close_utc`.
pub fn intraday_grid_starts(
    session_open_utc: DateTime<Utc>,
    session_close_utc: DateTime<Utc>,
    interval_secs: i64,
) -> Vec<i64> {
    let open_ts = session_open_utc.timestamp();
    let close_ts = session_close_utc.timestamp();
    let mut out = Vec::new();
    let mut ts = open_ts;
    while ts + interval_secs <= close_ts {
        out.push(ts);
        ts += interval_secs;
    }
    out
}

/// Expected intraday `end_ts` window (§6/§7): the last `required_history_bars`
/// session-anchored grid slots whose interval has closed plus grace, spilling
/// into the previous trading session's tail when the current session does not
/// yet have enough (before the first interval closes, or early in the day).
pub fn expected_intraday_end_ts_window(
    provider: &dyn MarketCalendarProvider,
    schedule: &MarketSessionSchedule,
    now_ts: i64,
    interval_secs: i64,
    effective_grace_seconds: i64,
    required_history_bars: usize,
) -> Option<Vec<i64>> {
    let current_grid = intraday_grid_starts(
        schedule.session_open_utc,
        schedule.session_close_utc,
        interval_secs,
    );
    let last_expected_idx = current_grid
        .iter()
        .rposition(|&s| s + interval_secs + effective_grace_seconds <= now_ts);

    let window: Vec<i64> = match last_expected_idx {
        Some(idx) => current_grid[..=idx].to_vec(),
        None => Vec::new(),
    };

    if window.len() >= required_history_bars {
        let start = window.len() - required_history_bars;
        return Some(window[start..].to_vec());
    }

    let remaining = required_history_bars - window.len();
    let prev_schedule = market_calendar::resolve_market_session_schedule_for_date(
        provider,
        schedule.previous_trading_date,
    )?;
    let prev_grid = intraday_grid_starts(
        prev_schedule.session_open_utc,
        prev_schedule.session_close_utc,
        interval_secs,
    );
    if prev_grid.len() < remaining {
        return None;
    }
    let tail_start = prev_grid.len() - remaining;
    let mut combined = prev_grid[tail_start..].to_vec();
    combined.extend(window);
    Some(combined)
}

// ---------------------------------------------------------------------------
// Aggregate + env-driven entry point
// ---------------------------------------------------------------------------

/// Evaluate every assignment in `config` and aggregate the results.
///
/// The production evaluator path — also used by
/// [`evaluate_daily_data_readiness_from_env`]. A single blocked assignment
/// blocks the aggregate; every assignment must independently reach `ready`
/// for `start_allowed = true`.
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_assignments(
    db: Option<&PgPool>,
    config: &MultiSymbolRuntimeConfig,
    binding: &EffectiveRuntimeBinding,
    calendar_provider: &dyn MarketCalendarProvider,
    provider_configs: &[ProviderConfig],
    instruments: &[TrackedInstrument],
    strategy_registry: &PluginRegistry,
    now_utc: DateTime<Utc>,
) -> DailyDataReadinessReport {
    if config.symbols.is_empty() {
        return blocked_report(REASON_REQUIRED_ASSIGNMENTS_MISSING);
    }

    let mut assignments = Vec::with_capacity(config.symbols.len());
    for assignment in &config.symbols {
        assignments.push(
            evaluate_assignment(
                db,
                assignment,
                binding,
                calendar_provider,
                provider_configs,
                instruments,
                strategy_registry,
                now_utc,
            )
            .await,
        );
    }

    let all_ready = assignments.iter().all(AssignmentReadiness::is_ready);
    DailyDataReadinessReport {
        start_allowed: all_ready,
        aggregate_state: if all_ready { "ready" } else { "blocked" },
        top_level_blocker: None,
        assignments,
    }
}

/// `MQK_STRATEGY_IDS`, split/trimmed/filtered — mirrors the `strategy_fleet`
/// derivation in `state.rs` and `multi_symbol_config::first_strategy_id_from_env`.
fn fleet_ids_from_env() -> Option<Vec<String>> {
    std::env::var("MQK_STRATEGY_IDS").ok().map(|ids| {
        ids.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect::<Vec<_>>()
    })
}

/// Full env-driven evaluation: resolves the canonical assignment source, the
/// immutable effective runtime binding, the provider/instrument registries,
/// and the active calendar provider, then delegates to [`evaluate_assignments`].
///
/// **Not called by any route, lifecycle gate, or scheduler in this phase.**
/// Exists so a future phase can wire it in without re-deriving this
/// composition — Phase B is foundation/evaluator only.
pub async fn evaluate_daily_data_readiness_from_env(
    db: Option<&PgPool>,
    now_utc: DateTime<Utc>,
) -> DailyDataReadinessReport {
    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(cfg) => cfg,
        Err(_) => return blocked_report(REASON_REQUIRED_ASSIGNMENTS_MISSING),
    };

    let fleet_ids = fleet_ids_from_env();
    let (_bootstrap, binding) =
        mqk_runtime::native_strategy::bootstrap_with_effective_binding(fleet_ids.as_deref());

    let mut strategy_registry = PluginRegistry::new();
    let _ =
        mqk_strategy::engines::register_builtin_strategies(&mut strategy_registry, String::new());

    let provider_registry_path = std::env::var("MQK_PROVIDER_REGISTRY_PATH")
        .unwrap_or_else(|_| "config/providers/providers.json".to_string());
    let provider_configs = mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
        &provider_registry_path,
    ))
    .unwrap_or_default();

    let instrument_registry_path = std::env::var("MQK_INSTRUMENT_REGISTRY_PATH")
        .unwrap_or_else(|_| "config/instruments/equities.json".to_string());
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        &instrument_registry_path,
    ))
    .unwrap_or_default();

    let calendar_provider = market_calendar::active_calendar_provider_from_env();

    evaluate_assignments(
        db,
        &config,
        &binding,
        calendar_provider.as_ref(),
        &provider_configs,
        &instruments,
        &strategy_registry,
        now_utc,
    )
    .await
}
