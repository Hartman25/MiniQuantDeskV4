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
use crate::state::{MultiSymbolConfigError, MultiSymbolRuntimeConfig, SymbolStrategyAssignment};

// ---------------------------------------------------------------------------
// Stable blocking reason codes
// ---------------------------------------------------------------------------

pub const REASON_REQUIRED_ASSIGNMENTS_MISSING: &str = "required_assignments_missing";
/// DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01 (REPAIR 3):
/// a non-missing-input assignment-resolution failure — reserved for
/// `MultiSymbolConfigError` variants that are not one of the three
/// missing-input variants (`MissingSymbol`/`MissingStrategyId`/
/// `MissingTimeframe`). See [`top_level_blocker_for_config_error`]'s doc
/// comment: this branch is currently unreachable through
/// `build_multi_symbol_runtime_config_from_env()`'s actual call graph, kept
/// for exhaustiveness and any future direct caller of the lower-level
/// watchlist-side builders.
pub const REASON_ASSIGNMENT_RESOLUTION_FAILED: &str = "assignment_resolution_failed";
pub const REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH: &str =
    "runtime_strategy_assignment_mismatch";
pub const REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH: &str =
    "runtime_strategy_symbol_binding_mismatch";
pub const REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH: &str = "runtime_strategy_timeframe_mismatch";
pub const REASON_STRATEGY_REQUIREMENT_UNKNOWN: &str = "strategy_requirement_unknown";
pub const REASON_ASSET_CLASS_UNKNOWN: &str = "asset_class_unknown";
pub const REASON_PROVIDER_PROVENANCE_INVALID: &str = "provider_provenance_invalid";
pub const REASON_PROVIDER_SYMBOL_MISMATCH: &str = "provider_symbol_mismatch";
pub const REASON_PROVIDER_ID_MISMATCH: &str = "provider_id_mismatch";
pub const REASON_PROVIDER_UNKNOWN: &str = "provider_unknown";
pub const REASON_PROVIDER_INGEST_TIME_FUTURE: &str = "provider_ingest_time_future";
pub const REASON_PROVIDER_DISABLED: &str = "provider_disabled";
pub const REASON_PROVIDER_CAPABILITY_MISMATCH: &str = "provider_capability_mismatch";
/// REPAIR 4 (DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01): no trustworthy
/// existing evidence in this repo (fixture, parser test, smoke artifact, or
/// genuinely-provenanced test-DB row) proves what `end_ts` value Alpaca's
/// `1D` bars actually use as their label. The existing
/// `market_calendar::midnight_utc_ts_for_date` mapping is retained for its
/// continuity-diffing arithmetic but is NOT certified against real/fixture
/// Alpaca daily data — every `1D` evaluation fails closed under this reason
/// until a separate, authorized fixture-based or network-verified proof
/// lands (out of scope for this patch: no network calls, no paper DB).
pub const REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED: &str =
    "provider_timestamp_convention_unverified";
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
// Provider-specific daily timestamp convention (§B2.2,
// DAILY-DATA-READINESS-01B-PROVIDER-CONTRACT-INTEGRATION-01)
// ---------------------------------------------------------------------------

/// How a provider labels the `end_ts` of a `1D` bar. Resolved per
/// `(provider_id, timeframe)` — never assumed uniform across providers (§6 of
/// the binding contract explicitly rejects a single universal `1D` rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DailyBarTimestampConvention {
    /// The provider's `1D` bar `end_ts` is proven (by a committed, non-network
    /// parser test against the provider's actual response shape) to land on
    /// midnight UTC of the trading date — i.e. exactly
    /// `market_calendar::midnight_utc_ts_for_date`.
    MidnightUtcMarketDate,
    /// Reserved for a future provider proven to echo an exact provider-side
    /// timestamp label distinct from `MidnightUtcMarketDate`. No currently
    /// enabled provider is bound to this variant.
    ExactProviderTimestamp,
    /// No trustworthy in-repo parser evidence proves this provider/timeframe's
    /// daily label convention. Fails closed — never treated as
    /// `MidnightUtcMarketDate` by default.
    Unverified,
}

/// Resolve the daily timestamp convention for `provider_id` — pure, no I/O.
///
/// Only meaningful for `Timeframe::D1`; callers must gate on that themselves
/// (this function does not take a timeframe parameter because every binding
/// here is `1D`-specific by construction).
///
/// # Verified bindings
/// - `"twelvedata"`: proven by
///   [`mqk_md`]'s own committed, non-network parser test
///   (`date_only_string_parses_to_midnight_utc_epoch` and the httpmock-backed
///   `rate_limit_retry_succeeds_after_one_body_429`/`fetch_bars` integration,
///   both in `mqk-md/src/lib.rs`) that a TwelveData `1D` response's
///   date-only `datetime` field (e.g. `"2024-01-02"`, the exact shape
///   TwelveData's real API returns for daily bars — see
///   `TwelveDataHistoricalProvider::fetch_bars`'s date-only parse branch)
///   is parsed to `00:00:00 UTC` of that date — exactly
///   `market_calendar::midnight_utc_ts_for_date`. TwelveData is also the
///   canonical `provider` assigned to every currently enabled equity in
///   `config/instruments/equities.json`, making this the one real,
///   production-relevant verified daily convention this bundle establishes.
/// - `"alpaca"`: **not verified.** No committed fixture or parser test in
///   this repo proves what `t` value Alpaca's `1Day` bars actually use as
///   their label — every existing Alpaca httpmock fixture
///   (`mqk-md/src/alpaca_provider.rs`) uses an intraday (`5Min`) timeframe
///   with a non-midnight time-of-day (e.g. `"...T13:30:00Z"`); none exercises
///   a `1Day` request. Fabricating a `1Day` fixture to manufacture "proof"
///   is explicitly forbidden by the binding contract. Alpaca `1D` therefore
///   remains `Unverified` — blocking only `(alpaca, 1D)`, never any other
///   provider's `1D` evaluation.
/// - Every other/unknown `provider_id`: `Unverified` (fail-closed default).
pub fn resolve_daily_bar_timestamp_convention(provider_id: &str) -> DailyBarTimestampConvention {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "twelvedata" => DailyBarTimestampConvention::MidnightUtcMarketDate,
        _ => DailyBarTimestampConvention::Unverified,
    }
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
    /// The instrument registry's configured provider id for this assignment's
    /// symbol (§B2.1). `None` when the symbol has no matching registry entry
    /// (already surfaced separately via `REASON_ASSET_CLASS_UNKNOWN`).
    pub expected_provider_id: Option<String>,
    /// The instrument registry's configured provider symbol (§B2.1).
    pub expected_provider_symbol: Option<String>,
    /// Distinct `provider_id` values actually found on bars in the bounded
    /// query window, deduplicated, in first-seen order. Empty when no bars
    /// were queried (db unavailable/query failed/blocked before the bar
    /// stage).
    pub actual_provider_ids: Vec<String>,
    /// Distinct `provider_symbol` values actually found on bars in the
    /// bounded query window, deduplicated, in first-seen order.
    pub actual_provider_symbols: Vec<String>,
    /// `"ready"` | `"blocked"` | `"db_unavailable"` | `"query_failed"`.
    pub readiness_state: &'static str,
    pub blockers: Vec<&'static str>,
    pub configured_grace_seconds: i64,
    pub effective_grace_seconds: i64,
    pub configured_future_skew_seconds: i64,
    pub effective_future_skew_seconds: i64,
    /// Phase C (route/response surface, §C.2): count of `is_complete` rows in
    /// the bounded query window. `None` when the bar stage was never reached
    /// (binding mismatch, unknown requirement/asset-class/provider capability,
    /// calendar unavailable) or the DB was unavailable/the query failed.
    pub loaded_completed_bars: Option<usize>,
    /// The last entry of the same expected-`end_ts` window
    /// [`evaluate_bar_readiness`] computes internally (§6/§7/§10), recomputed
    /// here from the identical inputs for response-surface exposure — never a
    /// second, independently-derived expectation. `None` under the same
    /// conditions as `loaded_completed_bars`, or when the window itself could
    /// not be produced (unsupported continuity / calendar walk exhausted).
    pub expected_latest_bar_ts: Option<i64>,
    /// `max(end_ts)` across every row in the bounded query window. `None`
    /// under the same conditions as `loaded_completed_bars`.
    pub actual_latest_bar_ts: Option<i64>,
    /// `"ok"` | `"insufficient"` | `"duplicate_detected"` | `"gap_detected"`
    /// | `"unsupported"` | `"unknown"` — derived from `blockers` (§C.2),
    /// never a second continuity computation. REPAIR 2
    /// (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): `"ok"` is never
    /// returned when `REASON_INSUFFICIENT_HISTORY` or
    /// `REASON_DUPLICATE_TIMESTAMP` is present.
    pub continuity_state: &'static str,
    /// `"ok"` | `"invalid"` | `"unknown"` — derived from `blockers` (§C.2),
    /// never a second provenance computation.
    pub provenance_state: &'static str,
    /// Operator-readable remediation, one entry per `blockers` entry, in the
    /// same order. Never suggests an ingest-job route for a timeframe the
    /// provider registry does not support (§13/§C.11).
    pub remediation: Vec<String>,
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

/// REPAIR 2 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): derive the
/// `continuity_state` summary from a finished evaluation's `blockers` list —
/// a pure function, directly unit-testable without a DB, so the "never
/// `ok` when a continuity/history blocker is present" invariant can be
/// proven for every blocker in isolation, not only combinations the live
/// evaluator happens to be able to produce today.
///
/// Every continuity/history blocker this evaluator can produce
/// (`REASON_INSUFFICIENT_HISTORY`, `REASON_INTERIOR_GAP`,
/// `REASON_EXPECTED_LATEST_BAR_MISSING`, `REASON_DUPLICATE_TIMESTAMP`,
/// `REASON_UNSUPPORTED_INTRADAY_CONTINUITY`, `REASON_CALENDAR_UNAVAILABLE`,
/// `REASON_MARKET_DATA_MISSING`) is checked explicitly below — none may fall
/// through to `"ok"`. `duplicate_bar_timestamp` does not exist as a distinct
/// reason code in this evaluator (only `duplicate_timestamp` does); auditing
/// the full blocker set confirmed this.
///
/// Ordered most-specific-first: unsupported-continuity and
/// calendar/market-data-missing (blockers that mean continuity could not
/// even be evaluated) take priority over a duplicate/insufficient/gap
/// finding computed from a window that was itself compromised.
///
/// Note: under the live bar-window evaluator, `md_bars`'s
/// `primary key (symbol, timeframe, end_ts)` makes a genuine duplicate
/// `end_ts` row structurally impossible to ingest, and a fixed-length
/// expected window makes `REASON_INSUFFICIENT_HISTORY` always co-occur with
/// an interior-gap/latest-bar-missing blocker (fewer completed bars than the
/// window length can never cover every expected slot). Both branches below
/// remain load-bearing regardless — they are the honest fallback the moment
/// either invariant does not hold (e.g. `evaluate_bar_readiness` exercised
/// directly, as this evaluator's own regression tests already do), and they
/// are what proves `"insufficient_history" -> continuity_state != "ok"` and
/// `"duplicate_timestamp" -> continuity_state != "ok"` in isolation.
pub fn derive_continuity_state(
    blockers: &[&'static str],
    loaded_completed_bars: Option<usize>,
) -> &'static str {
    if blockers.contains(&REASON_UNSUPPORTED_INTRADAY_CONTINUITY) {
        "unsupported"
    } else if blockers.contains(&REASON_CALENDAR_UNAVAILABLE)
        || blockers.contains(&REASON_MARKET_DATA_MISSING)
    {
        "unknown"
    } else if blockers.contains(&REASON_DUPLICATE_TIMESTAMP) {
        // A duplicate end_ts makes the bounded window structurally invalid —
        // never presentable as "ok", and distinguishable from an honest gap
        // or an honest shortfall.
        "duplicate_detected"
    } else if blockers.contains(&REASON_INTERIOR_GAP)
        || blockers.contains(&REASON_EXPECTED_LATEST_BAR_MISSING)
    {
        "gap_detected"
    } else if blockers.contains(&REASON_INSUFFICIENT_HISTORY) {
        // Fewer completed bars than required is a continuity shortfall, not
        // an "ok" continuity state, even though every bar actually present
        // is contiguous and duplicate-free.
        "insufficient"
    } else if loaded_completed_bars.is_some() {
        "ok"
    } else {
        "unknown"
    }
}

/// `pub(crate)`, not private: DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01
/// reuses this exact shape from `state::lifecycle`'s start gate to construct
/// a truthful blocked report for an assignment-resolution failure — never a
/// second, hand-rolled `DailyDataReadinessReport` literal.
pub(crate) fn blocked_report(reason: &'static str) -> DailyDataReadinessReport {
    DailyDataReadinessReport {
        start_allowed: false,
        aggregate_state: "blocked",
        top_level_blocker: Some(reason),
        assignments: Vec::new(),
    }
}

/// DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01 (REPAIR 3):
/// map a [`MultiSymbolConfigError`] to the stable top-level readiness
/// blocker reason for an applicable strict start-gate attempt whose
/// assignment resolution failed.
///
/// Verified directly against `build_multi_symbol_runtime_config_from_env`'s
/// actual call graph
/// (`state::multi_symbol_config::build_multi_symbol_runtime_config_from_env_and_watchlist`):
/// an invalid/unbuildable watchlist-v2 artifact never returns `Err` from
/// that function — it falls through to `build_legacy_single_symbol_config`
/// instead ("Fall through: invalid/unbuildable watchlist-v2 -> legacy, not
/// an error."). The only `Err` that can ever reach the start gate is
/// therefore one of `build_legacy_single_symbol_config`'s three
/// missing-input variants (`MissingSymbol`/`MissingStrategyId`/
/// `MissingTimeframe`) — all true "required assignment inputs missing"
/// facts, mapped to [`REASON_REQUIRED_ASSIGNMENTS_MISSING`]. The remaining
/// watchlist-side variants are matched here for exhaustiveness (so a future
/// direct caller of the lower-level watchlist builders gets a distinct,
/// honest reason instead of being silently folded into
/// `required_assignments_missing`), mapped to
/// [`REASON_ASSIGNMENT_RESOLUTION_FAILED`] — this arm is unreachable
/// through the current start-gate call site, not a speculative future
/// branch invented without source proof.
pub fn top_level_blocker_for_config_error(err: &MultiSymbolConfigError) -> &'static str {
    match err {
        MultiSymbolConfigError::MissingSymbol
        | MultiSymbolConfigError::MissingStrategyId
        | MultiSymbolConfigError::MissingTimeframe => REASON_REQUIRED_ASSIGNMENTS_MISSING,
        MultiSymbolConfigError::WatchlistNotV2
        | MultiSymbolConfigError::WatchlistNotApproved
        | MultiSymbolConfigError::MissingAssignment { .. }
        | MultiSymbolConfigError::ConcurrentLimitExceeded { .. }
        | MultiSymbolConfigError::HardCeilingExceeded { .. } => REASON_ASSIGNMENT_RESOLUTION_FAILED,
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
    let matched_instrument = instruments.iter().find(|i| {
        i.symbol
            .trim()
            .eq_ignore_ascii_case(assignment.symbol.trim())
    });
    let asset_class = matched_instrument
        .map(|i| i.trading_asset_class().to_string())
        .filter(|ac| !ac.trim().is_empty());
    if asset_class.is_none() {
        push_unique(&mut blockers, REASON_ASSET_CLASS_UNKNOWN);
    }
    // REPAIR 3 / B2.1: canonical provider identity for provenance validation —
    // sourced from the same registry entry asset_class came from, never a
    // second lookup or a guessed value.
    let expected_provider_id: Option<String> = matched_instrument.map(|i| i.provider.clone());
    let expected_provider_symbol_owned: Option<String> =
        matched_instrument.map(|i| i.provider_symbol.clone());
    let expected_provider_symbol = expected_provider_symbol_owned.as_deref().unwrap_or("");

    // --- Provider capability pre-check (§11/§13, B2.1) — config-only, no DB:
    // does the instrument's *exact configured provider* (never merely "any"
    // enabled provider) exist, is it enabled, and does it support this
    // (asset_class, timeframe)?
    let mut provider_capability_ok = false;
    if let (Ok(tf), Some(ac)) = (&parsed_timeframe, &asset_class) {
        match &expected_provider_id {
            None => {
                // No matched instrument at all — REASON_ASSET_CLASS_UNKNOWN
                // already covers this; nothing further to add here.
            }
            Some(pid) => match mqk_md::provider_registry::find_provider(provider_configs, pid) {
                None => push_unique(&mut blockers, REASON_PROVIDER_UNKNOWN),
                Some(cfg) if !cfg.enabled => push_unique(&mut blockers, REASON_PROVIDER_DISABLED),
                Some(cfg)
                    if !cfg.supports_asset_class(ac) || !cfg.supports_timeframe(tf.as_str()) =>
                {
                    push_unique(&mut blockers, REASON_PROVIDER_CAPABILITY_MISMATCH)
                }
                Some(_) => provider_capability_ok = true,
            },
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

    let mut actual_provider_ids: Vec<String> = Vec::new();
    let mut actual_provider_symbols: Vec<String> = Vec::new();
    // Phase C (§C.2 response fields) — populated only when the bar stage is
    // actually reached and the query succeeds; `None` otherwise (fail-closed,
    // never fabricated).
    let mut loaded_completed_bars: Option<usize> = None;
    let mut expected_latest_bar_ts: Option<i64> = None;
    let mut actual_latest_bar_ts: Option<i64> = None;

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
                        for row in &rows {
                            if !actual_provider_ids.contains(&row.provider_id) {
                                actual_provider_ids.push(row.provider_id.clone());
                            }
                            if let Some(ps) = row.provider_symbol.as_ref() {
                                if !actual_provider_symbols.contains(ps) {
                                    actual_provider_symbols.push(ps.clone());
                                }
                            }
                        }
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
                            expected_provider_id.as_deref().unwrap_or(""),
                            expected_provider_symbol,
                        );
                        for b in bar_blockers {
                            push_unique(&mut blockers, b);
                        }
                        // Phase C (§C.2): expose loaded/expected/actual bar
                        // facts on the response surface. Recomputes the same
                        // expected-window call `evaluate_bar_readiness` makes
                        // internally (identical inputs: calendar_provider,
                        // schedule, now_ts, eff_grace, required) rather than
                        // changing that function's signature, which is
                        // exercised directly (positional args) by the Phase B
                        // proof binary.
                        loaded_completed_bars = Some(rows.iter().filter(|r| r.is_complete).count());
                        actual_latest_bar_ts = rows.iter().map(|r| r.end_ts).max();
                        let expected_window = match tf {
                            Timeframe::D1 => expected_daily_end_ts_window(
                                calendar_provider,
                                &schedule,
                                now_utc.timestamp(),
                                eff_grace,
                                required,
                            ),
                            Timeframe::M1 | Timeframe::M5 => expected_intraday_end_ts_window(
                                calendar_provider,
                                &schedule,
                                now_utc.timestamp(),
                                tf.duration_secs(),
                                eff_grace,
                                required,
                            ),
                            Timeframe::H1 | Timeframe::M15 => None,
                        };
                        expected_latest_bar_ts = expected_window.and_then(|v| v.last().copied());
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

    // Phase C (§C.2) / REPAIR 2 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01):
    // derive the continuity summary state from the final `blockers` list —
    // never a second continuity computation.
    let continuity_state: &'static str = derive_continuity_state(&blockers, loaded_completed_bars);
    let provenance_state: &'static str = if blockers.iter().any(|b| {
        matches!(
            *b,
            REASON_PROVIDER_PROVENANCE_INVALID
                | REASON_PROVIDER_SYMBOL_MISMATCH
                | REASON_PROVIDER_ID_MISMATCH
                | REASON_PROVIDER_UNKNOWN
                | REASON_PROVIDER_DISABLED
                | REASON_PROVIDER_CAPABILITY_MISMATCH
                | REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED
                | REASON_PROVIDER_INGEST_TIME_FUTURE
        )
    }) {
        "invalid"
    } else if loaded_completed_bars.is_some() {
        "ok"
    } else {
        "unknown"
    };
    let remediation: Vec<String> = blockers.iter().map(|b| remediation_for_reason(b)).collect();

    AssignmentReadiness {
        assignment_symbol: assignment.symbol.clone(),
        assignment_timeframe: assignment.timeframe.clone(),
        configured_strategy_id: assignment.strategy_id.clone(),
        effective_runtime_strategy_id: binding.effective_runtime_strategy_id.clone(),
        effective_runtime_target_symbol: binding.effective_runtime_target_symbol.clone(),
        effective_runtime_timeframe_secs: binding.effective_runtime_timeframe_secs,
        required_history_bars,
        asset_class,
        expected_provider_id,
        expected_provider_symbol: expected_provider_symbol_owned,
        actual_provider_ids,
        actual_provider_symbols,
        readiness_state,
        blockers,
        configured_grace_seconds: configured_grace,
        effective_grace_seconds: eff_grace,
        configured_future_skew_seconds: configured_skew,
        effective_future_skew_seconds: eff_skew,
        loaded_completed_bars,
        expected_latest_bar_ts,
        actual_latest_bar_ts,
        continuity_state,
        provenance_state,
        remediation,
    }
}

/// Operator-readable remediation text for one blocking reason code (§C.2/§C.4).
/// Never suggests `POST /api/v1/ingest/jobs` for a timeframe the current
/// provider registry does not support (§13/§C.11) — `provider_capability_mismatch`
/// and `unsupported_intraday_continuity` explicitly say so instead.
fn remediation_for_reason(reason: &str) -> String {
    match reason {
        REASON_REQUIRED_ASSIGNMENTS_MISSING => {
            "configure a valid multi-symbol runtime assignment (approved watchlist-v2 artifact, \
             or MQK_STRATEGY_SYMBOL/MQK_STRATEGY_IDS/MQK_STRATEGY_MD_TIMEFRAME)"
                .to_string()
        }
        REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH => {
            "align configured_strategy_id with the active runtime strategy (MQK_STRATEGY_IDS[0]) \
             or correct the watchlist-v2 strategy assignment"
                .to_string()
        }
        REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH => {
            "align MQK_STRATEGY_SYMBOL with this assignment's symbol; the shared bootstrap can \
             only bind to one target symbol today (PER-SYMBOL-STRATEGY-BOOTSTRAP-01 is not yet built)"
                .to_string()
        }
        REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH => {
            "align this assignment's timeframe with the active strategy's StrategySpec.timeframe_secs"
                .to_string()
        }
        REASON_STRATEGY_REQUIREMENT_UNKNOWN => {
            "register StrategyDataRequirements for the active strategy in mqk-strategy".to_string()
        }
        REASON_ASSET_CLASS_UNKNOWN => {
            "add or correct this symbol's entry (asset_class) in the instrument registry"
                .to_string()
        }
        REASON_PROVIDER_PROVENANCE_INVALID => "re-ingest the bounded required window through a \
             registered, enabled provider via POST /api/v1/ingest/jobs (mode=sync_provider)"
            .to_string(),
        REASON_PROVIDER_SYMBOL_MISMATCH => {
            "correct provider_symbol in the instrument registry, then re-ingest with the \
             canonical provider symbol"
                .to_string()
        }
        REASON_PROVIDER_ID_MISMATCH => "re-ingest through the instrument registry's configured \
             provider, or correct the instrument registry's provider field"
            .to_string(),
        REASON_PROVIDER_UNKNOWN => {
            "register the provider in the provider registry (config/providers/providers.json)"
                .to_string()
        }
        REASON_PROVIDER_INGEST_TIME_FUTURE => {
            "investigate provider/system clock skew on ingested_at".to_string()
        }
        REASON_PROVIDER_DISABLED => "enable the provider in the provider registry".to_string(),
        REASON_PROVIDER_CAPABILITY_MISMATCH => {
            "this (provider, timeframe) combination is not supported by the current provider \
             registry; a separate provider/timeframe-support patch is required (out of scope) — \
             do not submit an ingest job for this timeframe"
                .to_string()
        }
        REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED => {
            "this provider's daily bar timestamp convention is unverified; a committed parser \
             proof is required before this provider can serve 1D readiness"
                .to_string()
        }
        REASON_CALENDAR_UNAVAILABLE => {
            "verify the active market calendar provider's coverage window and configuration"
                .to_string()
        }
        REASON_UNSUPPORTED_TIMEFRAME => {
            "use a timeframe supported by mqk_md::Timeframe::parse".to_string()
        }
        REASON_UNSUPPORTED_INTRADAY_CONTINUITY => {
            "this timeframe has no full session-anchored continuity proof yet; not remediable \
             via ingest alone (out of scope for this patch)"
                .to_string()
        }
        REASON_MARKET_DATA_MISSING => "run historical provider sync via POST \
             /api/v1/ingest/jobs (mode=sync_provider) for this symbol/timeframe"
            .to_string(),
        REASON_INSUFFICIENT_HISTORY => {
            "ingest additional history to satisfy required_history_bars".to_string()
        }
        REASON_DUPLICATE_TIMESTAMP => {
            "investigate duplicate end_ts rows in md_bars for this symbol/timeframe".to_string()
        }
        REASON_INTERIOR_GAP => {
            "run historical provider sync to fill the interior gap in the bounded window"
                .to_string()
        }
        REASON_LATEST_BAR_FUTURE => {
            "investigate a future-dated bar; check provider/system clock skew".to_string()
        }
        REASON_EXPECTED_LATEST_BAR_MISSING => {
            "refresh the latest closed bar via POST /api/v1/market-data/feed/poll-once, or wait \
             for publication grace"
                .to_string()
        }
        other => format!("investigate blocker '{other}'"),
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
    expected_provider_id: &str,
    expected_provider_symbol: &str,
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

    // Provider provenance (§11) — every bar in the bounded window, not just
    // the latest. REPAIR 3 completes this beyond provider_id/enabled/
    // capability to cover provider_source, provider_symbol, ingest_mode, and
    // ingest-time skew, per the audited provider-ingest write contract
    // (`mqk-db::md`'s `MdBarProviderMetadata`/write paths in
    // `routes/ingest.rs` and `mqk-cli::commands::md`).
    for row in rows {
        if row.provider_id.trim().is_empty() || row.provider_id.eq_ignore_ascii_case("unknown") {
            // Legacy/pre-migration-0042 rows (§12) — blank or the literal
            // "unknown" sentinel. Distinguished from B2.1's provider_unknown
            // (a real, non-blank provider_id string that just isn't
            // registered), which is a different, more specific defect.
            push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID);
        } else {
            match mqk_md::provider_registry::find_provider(provider_configs, &row.provider_id) {
                None => push_unique(&mut blockers, REASON_PROVIDER_UNKNOWN),
                Some(cfg) if !cfg.enabled => push_unique(&mut blockers, REASON_PROVIDER_DISABLED),
                Some(cfg)
                    if !cfg.supports_asset_class(asset_class)
                        || !cfg.supports_timeframe(timeframe.as_str()) =>
                {
                    push_unique(&mut blockers, REASON_PROVIDER_CAPABILITY_MISMATCH)
                }
                Some(_) => {}
            }

            // B2.1: the row's actual provider_id must match the instrument
            // registry's exact configured provider for this symbol — an
            // enabled, capable, but *wrong* provider must not silently pass.
            if !expected_provider_id.trim().is_empty()
                && !row
                    .provider_id
                    .trim()
                    .eq_ignore_ascii_case(expected_provider_id.trim())
            {
                push_unique(&mut blockers, REASON_PROVIDER_ID_MISMATCH);
            }
        }

        // provider_source: audited against every current write path
        // (`ingest_csv_to_md_bars`, `routes/ingest.rs`, `mqk-cli::commands::md`)
        // — every one of them stamps a real value; only the metadata-less
        // `MdBarProviderMetadata::unknown()` default (already caught above
        // via provider_id == "unknown") leaves it blank. No legitimately-
        // optional case was found, so blank always blocks.
        let source_blank = row
            .provider_source
            .as_deref()
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true);
        if source_blank {
            push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID);
        }

        // ingest_mode: every current write path stamps a real value
        // (e.g. "csv_import", "historical_backfill", "provider_ingest").
        let ingest_mode_blank = row
            .ingest_mode
            .as_deref()
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true);
        if ingest_mode_blank {
            push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID);
        }

        // provider_symbol: must match the canonical instrument-registry
        // mapping, normalized the same trim/case-insensitive way every other
        // symbol comparison in this evaluator already applies (§3a). Blank
        // blocks as invalid provenance; a present-but-wrong value blocks
        // under its own distinguishable reason.
        match row.provider_symbol.as_deref().map(str::trim) {
            None | Some("") => push_unique(&mut blockers, REASON_PROVIDER_PROVENANCE_INVALID),
            Some(actual) => {
                if !actual.eq_ignore_ascii_case(expected_provider_symbol.trim()) {
                    push_unique(&mut blockers, REASON_PROVIDER_SYMBOL_MISMATCH);
                }
            }
        }

        // ingested_at: structurally valid by construction (typed `DateTime<Utc>`
        // from a `timestamptz` column) — the remaining honest check is that it
        // is not materially in the future, using the same effective skew
        // ceiling already computed for bar timestamps.
        if row.ingested_at.timestamp() > now_ts + effective_future_skew_seconds {
            push_unique(&mut blockers, REASON_PROVIDER_INGEST_TIME_FUTURE);
        }

        // provider_updated_at_utc: audited against every current write path
        // (`routes/ingest.rs`, `mqk-cli::commands::md`, `mqk-db::md`'s CSV/
        // provider ingestion) and the Alpaca provider's own `ProviderBar`
        // shape (`mqk-md::alpaca_provider`, whose parsed `AlpacaRawBar` has
        // no update-timestamp field at all) — every call site sets this
        // field to `None` and no enabled provider supplies it. This is a
        // proven, documented optional field, not an unaudited gap; it is
        // intentionally never checked here.
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
        Timeframe::D1 => {
            // B2.2: the unverified-timestamp-convention blocker is now
            // provider-specific (§6/§B2.2) — it must not fire for a provider
            // whose daily label convention has real, committed parser proof
            // (currently only "twelvedata"; see
            // `resolve_daily_bar_timestamp_convention`'s doc comment).
            // Continuity is still computed and reported below regardless, so
            // other genuine defects remain visible alongside this one,
            // exactly like the H1/M15 unsupported-continuity arms.
            if resolve_daily_bar_timestamp_convention(expected_provider_id)
                == DailyBarTimestampConvention::Unverified
            {
                push_unique(
                    &mut blockers,
                    REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED,
                );
            }
            expected_daily_end_ts_window(
                calendar_provider,
                schedule,
                now_ts,
                effective_grace_seconds,
                required_history_bars,
            )
        }
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
///
/// REPAIR 1 (DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01): when `schedule`'s
/// own date is not a trading day (weekend or full-day exchange holiday),
/// there is no current-day grid at all — regardless of the wall-clock time
/// on that date — so the entire window comes from the previous trading
/// session's tail unconditionally.
pub fn expected_intraday_end_ts_window(
    provider: &dyn MarketCalendarProvider,
    schedule: &MarketSessionSchedule,
    now_ts: i64,
    interval_secs: i64,
    effective_grace_seconds: i64,
    required_history_bars: usize,
) -> Option<Vec<i64>> {
    if !schedule.is_trading_day {
        return previous_session_grid_tail(
            provider,
            schedule,
            interval_secs,
            required_history_bars,
        );
    }

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
    let prev_tail = previous_session_grid_tail(provider, schedule, interval_secs, remaining)?;
    let mut combined = prev_tail;
    combined.extend(window);
    Some(combined)
}

/// Resolves `schedule.previous_trading_date`'s own session schedule and
/// returns its final `count` session-open-anchored grid slots — shared by
/// both the non-trading-day branch and the too-early-in-the-session
/// spillover branch of [`expected_intraday_end_ts_window`].
///
/// REPAIR 2 (DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01): fails closed
/// (`None`) if that prior session's own resolved coverage is not `Active` —
/// the previous session used for spillover is itself part of the bounded
/// warmup window and must not be trusted merely because the *current*
/// evaluation date is in coverage.
fn previous_session_grid_tail(
    provider: &dyn MarketCalendarProvider,
    schedule: &MarketSessionSchedule,
    interval_secs: i64,
    count: usize,
) -> Option<Vec<i64>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let prev_schedule = market_calendar::resolve_market_session_schedule_for_date(
        provider,
        schedule.previous_trading_date,
    )?;
    if prev_schedule.coverage_state != CalendarCoverageState::Active {
        return None;
    }
    let prev_grid = intraday_grid_starts(
        prev_schedule.session_open_utc,
        prev_schedule.session_close_utc,
        interval_secs,
    );
    if prev_grid.len() < count {
        return None;
    }
    let tail_start = prev_grid.len() - count;
    Some(prev_grid[tail_start..].to_vec())
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
pub fn fleet_ids_from_env() -> Option<Vec<String>> {
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
///
/// # Lifecycle-safety contract (REPAIR 5, DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01)
///
/// This function constructs its **own** `NativeStrategyBootstrap` +
/// [`EffectiveRuntimeBinding`] internally (one call to
/// [`mqk_runtime::native_strategy::bootstrap_with_effective_binding`]) and
/// discards the bootstrap. **The future runtime start gate must NOT call this
/// function for its own start-attempt evaluation.** A start gate already
/// constructs its own bootstrap + binding for the actual run; calling this
/// convenience wrapper afterward would silently construct and discard a
/// *second*, independently-derived bootstrap — a start attempt must be
/// evaluated using the exact bootstrap/binding pair created for that attempt,
/// never a second env-driven reconstruction that could disagree with it if
/// env state or evaluation ordering ever diverges. The start gate must
/// instead call [`evaluate_assignments`] (or [`evaluate_assignment`])
/// directly, passing the binding it already holds. This function remains
/// safe for a read-only route to call (a fresh preview, not the active run's
/// stored binding) as long as the response honestly labels it as
/// preview/current-config truth rather than the active run's binding — this
/// phase adds no such route.
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

    let context = load_readiness_context_from_env();
    evaluate_readiness_with_binding(db, &config, &binding, &context, now_utc).await
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01C-ENFORCEMENT-01 (Phase C): canonical evaluation
// context shared by every readiness surface (routes + the runtime start
// gate), plus durable start-attempt evidence.
//
// Binding scopes (§C.1):
//   - `configuration_preview` — a fresh preview built from current env/DB
//     state, used by GET routes. Never proof of an active runtime.
//   - `start_attempt_binding` — the exact bootstrap/binding pair the runtime
//     start gate already constructed for its own attempt. Never a second,
//     independently-derived bootstrap (see the lifecycle-safety contract on
//     `evaluate_daily_data_readiness_from_env` above).
// ---------------------------------------------------------------------------

pub const BINDING_SCOPE_CONFIGURATION_PREVIEW: &str = "configuration_preview";
pub const BINDING_SCOPE_START_ATTEMPT_BINDING: &str = "start_attempt_binding";

pub const READINESS_RESPONSE_SCHEMA_VERSION: &str = "daily-data-readiness-response-v1";
pub const READINESS_EVIDENCE_SCHEMA_VERSION: &str = "daily-data-readiness-evidence-v1";

pub const EVENT_TYPE_READINESS_EVALUATED: &str = "daily_data_readiness_evaluated";
pub const EVENT_TYPE_READINESS_RUN_LINKED: &str = "daily_data_readiness_run_linked";

/// Start-gate-only reason (§C.9): a ready verdict whose durable pre-start
/// evidence could not be persisted is refused, never silently allowed
/// through.
pub const REASON_READINESS_EVIDENCE_PERSIST_FAILED: &str = "readiness_evidence_persist_failed";

/// Start-gate-only reason (REPAIR 4, DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01):
/// a `run_id` now exists for this start attempt, but the
/// `daily_data_readiness_run_linked` evidence event could not be persisted.
/// Fail-closed: the run row is never armed/begun/ticked and the execution
/// loop is never spawned — the run row must not be presented as an actively
/// started runtime merely because it exists.
pub const REASON_READINESS_RUN_LINK_PERSIST_FAILED: &str = "readiness_run_link_persist_failed";

/// Registries/calendar/strategy-registry composition shared by every
/// readiness evaluation surface — loaded once per call from the same
/// env-driven sources Phase B's `evaluate_daily_data_readiness_from_env`
/// always used, factored out so the start gate can build this composition
/// without re-deriving the assignment/binding resolution it already owns.
pub struct DailyDataReadinessContext {
    pub calendar_provider: Box<dyn MarketCalendarProvider>,
    pub provider_configs: Vec<ProviderConfig>,
    pub instruments: Vec<TrackedInstrument>,
    pub strategy_registry: PluginRegistry,
}

/// Load the shared [`DailyDataReadinessContext`] from env/config-file state.
/// Pure I/O composition (file reads only) — no DB, no provider/broker calls.
pub fn load_readiness_context_from_env() -> DailyDataReadinessContext {
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

    DailyDataReadinessContext {
        calendar_provider,
        provider_configs,
        instruments,
        strategy_registry,
    }
}

/// Evaluate `config`/`binding` against `context` — the single production
/// evaluation path shared by every configuration-preview (route) and
/// start-attempt-binding (lifecycle gate) caller. Never re-derives `config`
/// or `binding`; callers own that per their own binding scope (§C.1).
pub async fn evaluate_readiness_with_binding(
    db: Option<&PgPool>,
    config: &MultiSymbolRuntimeConfig,
    binding: &EffectiveRuntimeBinding,
    context: &DailyDataReadinessContext,
    now_utc: DateTime<Utc>,
) -> DailyDataReadinessReport {
    evaluate_assignments(
        db,
        config,
        binding,
        context.calendar_provider.as_ref(),
        &context.provider_configs,
        &context.instruments,
        &context.strategy_registry,
        now_utc,
    )
    .await
}

// ---------------------------------------------------------------------------
// Phase C: deterministic evaluation identity + durable evidence (§C.8-§C.10)
// ---------------------------------------------------------------------------

/// Deterministic `evaluation_id` for one start attempt (§C.8, REPAIR 1
/// DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): UUIDv5 over the
/// full-nanosecond-precision evaluation instant, a process-local monotonic
/// start-attempt sequence number (`AppState::next_daily_data_readiness_attempt_seq`),
/// the effective runtime binding identity, and the canonical assignment
/// identity — never `Uuid::new_v4()` (`.claude/rules/audit_repo_truth_rules.md`).
///
/// `attempt_seq` is the load-bearing collision-breaker: two start attempts
/// with otherwise-identical inputs (same wall-clock second/minute, same
/// binding, same assignment set — e.g. an operator retrying immediately
/// after a transient refusal) always receive distinct sequence numbers
/// (`AppState::lifecycle_op` serializes concurrent start attempts within one
/// process, so the counter is race-free), and therefore always produce
/// distinct `evaluation_id`s and distinct durable event rows — no prior
/// attempt's evidence is ever silently shadowed by
/// `sys_autonomous_session_events`'s `on conflict (id) do nothing`.
///
/// Never called for a configuration-preview GET evaluation (§C.1) — those
/// never persist durable evidence and never allocate an attempt sequence
/// number.
pub fn compute_evaluation_id(
    now_utc: DateTime<Utc>,
    attempt_seq: u64,
    binding: &EffectiveRuntimeBinding,
    config: &MultiSymbolRuntimeConfig,
) -> uuid::Uuid {
    let assignment_identity: Vec<String> = config
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
    compute_evaluation_id_from_assignment_identity(
        now_utc,
        attempt_seq,
        binding,
        &assignment_identity,
    )
}

/// DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01 (REPAIR 2):
/// [`compute_evaluation_id`]'s seam, generalized to accept an already-derived
/// assignment identity rather than requiring a valid [`MultiSymbolRuntimeConfig`].
/// [`compute_evaluation_id`] is now a thin wrapper deriving its
/// `assignment_identity` from `config.symbols` and delegating here — the seed
/// construction (and therefore every `evaluation_id` value) is byte-for-byte
/// unchanged for every existing caller.
///
/// Exists so an applicable strict start-gate attempt whose assignment
/// resolution itself failed can still receive a real, stable, distinct
/// `evaluation_id` — built from a bounded assignment-resolution-failure
/// identity (see the start gate's own
/// `"assignment_resolution_error:{MultiSymbolConfigError::as_str()}"`
/// construction) instead of fabricating a valid config. `attempt_seq` remains
/// the load-bearing collision-breaker described on [`compute_evaluation_id`].
pub fn compute_evaluation_id_from_assignment_identity(
    now_utc: DateTime<Utc>,
    attempt_seq: u64,
    binding: &EffectiveRuntimeBinding,
    assignment_identity: &[String],
) -> uuid::Uuid {
    let evaluated_at_nanos = now_utc
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now_utc.timestamp().saturating_mul(1_000_000_000));
    let seed = format!(
        "daily_data_readiness.v2|{}|{}|{}|{}|{}|{}",
        evaluated_at_nanos,
        attempt_seq,
        binding
            .effective_runtime_strategy_id
            .as_deref()
            .unwrap_or(""),
        binding
            .effective_runtime_target_symbol
            .as_deref()
            .unwrap_or(""),
        binding
            .effective_runtime_timeframe_secs
            .map(|s| s.to_string())
            .unwrap_or_default(),
        assignment_identity.join(","),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, seed.as_bytes())
}

/// Bounded, schema-versioned JSON detail for the `daily_data_readiness_evaluated`
/// pre-start evidence event (§C.8). Never carries full bar histories, secrets,
/// credentials, environment dumps, or unbounded error chains — only bounded
/// per-assignment summaries (symbol, timeframe, readiness_state, blockers).
pub fn build_pre_start_evidence_detail(
    evaluation_id: uuid::Uuid,
    evaluated_at_utc: DateTime<Utc>,
    applicability: &str,
    report: &DailyDataReadinessReport,
) -> serde_json::Value {
    let assignments: Vec<serde_json::Value> = report
        .assignments
        .iter()
        .map(|a| {
            serde_json::json!({
                "assignment_symbol": a.assignment_symbol,
                "assignment_timeframe": a.assignment_timeframe,
                "readiness_state": a.readiness_state,
                "blockers": a.blockers,
            })
        })
        .collect();
    serde_json::json!({
        "schema_version": READINESS_EVIDENCE_SCHEMA_VERSION,
        "evaluation_id": evaluation_id.to_string(),
        "evaluated_at_utc": evaluated_at_utc.to_rfc3339(),
        "binding_scope": BINDING_SCOPE_START_ATTEMPT_BINDING,
        "applicability": applicability,
        "start_allowed": report.start_allowed,
        "top_level_blocker": report.top_level_blocker,
        "assignment_count": report.assignments.len(),
        "assignments": assignments,
        // Trivially true: this field only exists inside a JSON blob that was
        // in fact handed to the persistence call. A write failure produces no
        // row at all (never a row with this field set to false) — the API
        // response's own `evidence_persisted` field (derived from whether the
        // write actually succeeded) is the authoritative signal for callers.
        "evidence_persisted": true,
    })
}

/// Persist the pre-start `daily_data_readiness_evaluated` evidence event
/// (§C.8/§C.9). Returns `true` iff the write succeeded. The event `id` is
/// derived solely from `evaluation_id`, so a genuine retry of the same
/// logical attempt is idempotent (`on conflict (id) do nothing`).
pub async fn persist_pre_start_readiness_evidence(
    db: &PgPool,
    evaluation_id: uuid::Uuid,
    evaluated_at_utc: DateTime<Utc>,
    applicability: &str,
    report: &DailyDataReadinessReport,
) -> bool {
    let detail =
        build_pre_start_evidence_detail(evaluation_id, evaluated_at_utc, applicability, report);
    let row = mqk_db::AutonomousSessionEventRow {
        id: format!("daily_data_readiness_evaluated:{evaluation_id}"),
        ts_utc: evaluated_at_utc,
        event_type: EVENT_TYPE_READINESS_EVALUATED.to_string(),
        resume_source: None,
        detail: detail.to_string(),
        run_id: None,
        source: "mqk-daemon.daily_data_readiness".to_string(),
    };
    mqk_db::persist_autonomous_session_event(db, &row)
        .await
        .is_ok()
}

/// Persist the `daily_data_readiness_run_linked` evidence event (§C.10),
/// correlating a successful start's `run_id` back to `evaluation_id`. A
/// second row, never an update — `sys_autonomous_session_events` has no
/// `UPDATE` path. Returns `true` iff the write succeeded.
pub async fn persist_run_linked_readiness_evidence(
    db: &PgPool,
    evaluation_id: uuid::Uuid,
    run_id: uuid::Uuid,
    linked_at_utc: DateTime<Utc>,
) -> bool {
    let detail = serde_json::json!({
        "schema_version": READINESS_EVIDENCE_SCHEMA_VERSION,
        "evaluation_id": evaluation_id.to_string(),
        "run_id": run_id.to_string(),
        "linked_at_utc": linked_at_utc.to_rfc3339(),
    });
    let row = mqk_db::AutonomousSessionEventRow {
        id: format!("daily_data_readiness_run_linked:{evaluation_id}:{run_id}"),
        ts_utc: linked_at_utc,
        event_type: EVENT_TYPE_READINESS_RUN_LINKED.to_string(),
        resume_source: None,
        detail: detail.to_string(),
        run_id: Some(run_id),
        source: "mqk-daemon.daily_data_readiness".to_string(),
    };
    mqk_db::persist_autonomous_session_event(db, &row)
        .await
        .is_ok()
}

// ---------------------------------------------------------------------------
// REPAIR 3/4 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): the narrow,
// production-used sequencing that advances an already-created run row to an
// active execution loop after a `start_allowed` readiness verdict's
// pre-start evidence has been persisted.
//
// `AppState::start_execution_runtime` (production) and the synthetic
// lifecycle proof test both call [`advance_run_to_active`] against
// [`RuntimeStartEffects`] — never two independently-ordered
// implementations. The trait abstracts only the two steps that would
// otherwise force a test to construct a real broker adapter: runtime
// construction/safety-setup and execution-loop spawn. Run creation and
// pre-start evidence persistence are NOT abstracted here — both the
// production path and the proof test call the exact same real, DB-only
// functions for those (`AppState::create_or_reuse_run_for_start`,
// `evaluate_readiness_with_binding`, `compute_evaluation_id`,
// `persist_pre_start_readiness_evidence`), so no separate test-only
// reimplementation of that ordering exists either.
// ---------------------------------------------------------------------------

/// Ordered trace tag pushed after local loop ownership has been reserved
/// (ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2), strictly before
/// any other local effect is built or published.
pub const TRACE_OWNERSHIP_RESERVED: &str = "ownership_reserved";
/// Ordered trace tag pushed after a successful run-linked evidence persist.
pub const TRACE_RUN_LINK_EVENT_PERSISTED: &str = "run_link_event_persisted";
/// Ordered trace tag pushed after the local run-start bundle has been
/// committed to `AppState` (requirement 2's "commit all run-scoped AppState
/// fields for run_id").
pub const TRACE_LOCAL_BUNDLE_COMMITTED: &str = "local_bundle_committed";
/// Ordered trace tag pushed after the execution loop has been spawned and
/// installed into the reserved slot.
pub const TRACE_LOOP_SPAWNED: &str = "loop_spawned";

/// Which `RuntimeLifecycleError` constructor a [`RuntimeStartEffectsError`]
/// must be converted through, so [`advance_run_to_active`] can report a
/// failure without depending on `crate::state`'s `pub(crate)` error
/// constructors itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStartEffectsErrorKind {
    Internal,
    Conflict,
}

/// Opaque failure from a [`RuntimeStartEffects`] method. `fault_class` and
/// `message` are combined by the caller exactly as
/// `RuntimeLifecycleError::internal`/`::conflict` would combine them
/// (`"{fault_class}: {message}"` for `Internal`, `message` alone for
/// `Conflict`), so production callers reproduce their pre-existing error
/// text exactly.
#[derive(Debug)]
pub struct RuntimeStartEffectsError {
    pub kind: RuntimeStartEffectsErrorKind,
    pub fault_class: &'static str,
    pub message: String,
}

impl RuntimeStartEffectsError {
    pub fn internal(fault_class: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeStartEffectsErrorKind::Internal,
            fault_class,
            message: message.into(),
        }
    }

    pub fn conflict(fault_class: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeStartEffectsErrorKind::Conflict,
            fault_class,
            message: message.into(),
        }
    }
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 4: how far a single
/// start attempt progressed before it failed, read from
/// [`RuntimeStartEffects::start_phase_reached`] after any failure. Ordered
/// earliest to latest; later phases carry stronger evidence that
/// broker/outbox/DB-visible side effects may already exist.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeStartPhase {
    /// Before `arm_run` has been attempted (includes a reservation conflict
    /// — no runtime construction was ever attempted).
    #[default]
    BeforeArm,
    /// `arm_run` succeeded; `begin_run` not yet attempted.
    ArmedBeforeBegin,
    /// `begin_run` and the initial heartbeat succeeded; `orchestrator.tick()`
    /// not yet called.
    RunningBeforeInitialTick,
    /// `orchestrator.tick()` has been called but had not yet returned when
    /// the failure was observed — its outcome is genuinely unknown; it may
    /// have already written to the DB, the outbox, or the broker.
    InitialTickStarted,
    /// `orchestrator.tick()` returned `Ok` and the post-tick heartbeat
    /// refresh succeeded.
    InitialTickCompleted,
    /// Every runtime-construction effect above succeeded; committing the
    /// local run-scoped bundle to `AppState` is underway.
    LocalCommitStarted,
    /// The local bundle is committed and the execution loop handle has been
    /// installed into the active slot.
    LoopInstalled,
}

impl RuntimeStartPhase {
    /// Requirement 4 policy: failures at or before
    /// `RunningBeforeInitialTick` (strictly before `tick()` is ever called)
    /// have caused no broker/outbox-visible side effects yet and may
    /// cleanly become `Stopped`. Once `tick()` has been called — whether or
    /// not it returned, and everything after — a clean retry is not proven,
    /// so the durable run must be `Halted` instead.
    pub fn cleanly_stoppable(&self) -> bool {
        matches!(
            self,
            RuntimeStartPhase::BeforeArm
                | RuntimeStartPhase::ArmedBeforeBegin
                | RuntimeStartPhase::RunningBeforeInitialTick
        )
    }
}

/// Local (in-process) rollback outcome from a single
/// [`RuntimeStartEffects::rollback_local_effects`] call.
#[derive(Debug, Default)]
pub struct LocalRollbackOutcome {
    /// `Some(Err(_))` when this attempt had acquired the orchestrator's
    /// runtime leadership lease and releasing it failed. `None` when no
    /// lease was ever held for this attempt (e.g. the attempt failed before
    /// the orchestrator was constructed, or reservation itself conflicted),
    /// OR when the lease was instead released by the spawned task itself
    /// (see `task_side_leadership_release_outcome`) after the orchestrator
    /// had already been handed off to it.
    pub leadership_release_outcome: Option<Result<(), String>>,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1: `Some(_)`
    /// when the orchestrator had already been moved into the spawned
    /// execution-loop task (a successful `spawn_loop` handoff) before a
    /// barrier/install failure required that task to release the lease
    /// itself, before `rollback_local_effects` ever ran (which finds
    /// `self.orchestrator` already taken and has nothing left to release).
    /// Distinct from `leadership_release_outcome` (released directly by
    /// `ProductionRuntimeStartEffects` before any handoff) so a test can
    /// assert precisely which code path performed the exactly-once release.
    pub task_side_leadership_release_outcome: Option<Result<(), String>>,
    /// `Some(Err(_))` when the join of that spawned task's handle itself
    /// failed (the task panicked) — never discarded via `let _ =`. `None`
    /// when no task was ever spawned for this attempt, or the join
    /// succeeded.
    pub task_side_join_outcome: Option<Result<(), String>>,
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 5: bounded, structured
/// truth about what happened when a failed start attempt was rolled back —
/// never silently swallowed. `durable_status_unknown` is the fail-closed
/// signal a caller must check before treating the original error as an
/// ordinary, fully-cleaned-up start refusal.
#[derive(Debug)]
pub enum DurableRollbackDisposition {
    /// The durable run reached `Armed`/`Running` and was moved to `Stopped`
    /// (phase was cleanly stoppable — no evidence `tick()` was ever called).
    Stopped,
    /// The durable run reached `Armed`/`Running` and was moved to `Halted`
    /// (phase was at or past `InitialTickStarted` — a clean retry is not
    /// proven).
    Halted,
    /// The durable run was never `Armed`/`Running` (e.g. a reservation
    /// conflict before `arm_run` was ever attempted) — nothing to roll back.
    AlreadyNonActive,
    /// `fetch_run` itself failed — the durable run's current status is
    /// unknown.
    QueryFailed,
    /// `stop_run`/`halt_run` failed after `fetch_run` confirmed
    /// `Armed`/`Running` — the durable run may still be `Armed`/`Running`
    /// with no local owner.
    TransitionFailed,
}

#[derive(Debug)]
pub struct RollbackOutcome {
    pub phase_reached: RuntimeStartPhase,
    pub local: LocalRollbackOutcome,
    pub durable: DurableRollbackDisposition,
    /// `true` for `QueryFailed`/`TransitionFailed` — the durable run's
    /// terminal status could not be confirmed by this rollback attempt.
    pub durable_status_unknown: bool,
}

impl RollbackOutcome {
    /// `true` when either the durable rollback could not confirm a safe
    /// terminal state, or the orchestrator lease release failed — either
    /// condition means a caller must not treat the original error as an
    /// ordinary, fully-cleaned-up start refusal.
    pub fn is_degraded(&self) -> bool {
        self.durable_status_unknown
            || self
                .local
                .leadership_release_outcome
                .as_ref()
                .is_some_and(|r| r.is_err())
            || self
                .local
                .task_side_leadership_release_outcome
                .as_ref()
                .is_some_and(|r| r.is_err())
            || self
                .local
                .task_side_join_outcome
                .as_ref()
                .is_some_and(|r| r.is_err())
    }
}

/// The production effects that follow run creation and (when applicable)
/// run-linked evidence persistence. Implemented once for production
/// (`state::lifecycle::ProductionRuntimeStartEffects`, wrapping the exact
/// existing orchestrator-build/arm/begin/tick/spawn sequence) and once for
/// each synthetic lifecycle proof test (no-op/counter-based fakes that
/// never construct a broker, provider, or scheduler) — every implementation
/// shares the same reserve-before-publish/rollback/phase contract, so a
/// hermetic test using a fake exercises the exact same
/// [`advance_run_to_active`] sequencing production uses, never a separate
/// test-only reimplementation of it.
#[async_trait::async_trait]
pub trait RuntimeStartEffects: Send + Sync {
    /// Reserve local loop ownership for `run_id` — the first
    /// `RuntimeStartEffects` call [`advance_run_to_active`] makes, strictly
    /// before any runtime construction or `AppState` publication
    /// (ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: "reserve
    /// ownership before publish"). Returns a `Conflict` when another local
    /// owner already holds the run slot; on conflict the implementation
    /// must not have touched any other local state for this attempt yet —
    /// there is nothing to release beyond the (never-acquired) reservation
    /// itself.
    async fn reserve_local_ownership(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), RuntimeStartEffectsError>;

    /// Construct/arm/begin/tick the runtime for `run_id`, then commit the
    /// prepared local run-start bundle to `AppState` as one call. Only
    /// called after [`RuntimeStartEffects::reserve_local_ownership`] has
    /// returned `Ok`. Must not spawn the execution loop itself (see
    /// [`RuntimeStartEffects::spawn_loop`]).
    async fn start_runtime_effects(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), RuntimeStartEffectsError>;

    /// Spawn the execution loop for `run_id` and install it into the
    /// reserved slot. Only called after
    /// [`RuntimeStartEffects::start_runtime_effects`] has returned `Ok`.
    async fn spawn_loop(&self, run_id: uuid::Uuid) -> Result<(), RuntimeStartEffectsError>;

    /// Fail-closed rollback: clear every local (in-process) effect this
    /// attempt committed for `run_id` — the loop-ownership reservation,
    /// dynamic-selection state, accepted artifact, native strategy
    /// bootstrap, execution snapshot, per-run counters, and per-symbol
    /// target state. Called by [`advance_run_to_active`] on any failure
    /// from [`RuntimeStartEffects::reserve_local_ownership`],
    /// [`RuntimeStartEffects::start_runtime_effects`], or
    /// [`RuntimeStartEffects::spawn_loop`] — regardless of how far the
    /// attempt got, so it must be idempotent and must never clear state
    /// committed by a different, newer `run_id` (compare-and-clear).
    async fn rollback_local_effects(&self, run_id: uuid::Uuid) -> LocalRollbackOutcome;

    /// How far this attempt progressed before it failed. Read synchronously
    /// (no `.await`) after any failure, for [`RuntimeStartPhase`]-aware
    /// durable rollback policy (requirement 4).
    fn start_phase_reached(&self) -> RuntimeStartPhase;
}

/// Failure from [`advance_run_to_active`]. `RunLinkPersistFailed` is
/// REPAIR 4's fail-closed policy: the run row already exists but is never
/// armed/begun/ticked and the loop is never spawned. Carries the same
/// structured [`RollbackOutcome`] truth `Effects` does (BUNDLE-7-PHASE-7A-
/// CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 8: "structured rollback
/// truth returned") — the only local effect a run-link failure can ever
/// need rolled back is the reservation this function's caller made just
/// before attempting the link write, so `rollback.local` here only ever
/// reflects a reservation release, never a runtime-effects/spawn rollback.
/// `Effects` carries the original fault plus the full rollback truth
/// (requirement 5) — never masked, and a caller can check `rollback.is_
/// degraded()` before treating the original error as an ordinary,
/// fully-cleaned-up start refusal.
#[derive(Debug)]
pub enum RuntimeStartSequenceError {
    RunLinkPersistFailed {
        evaluation_id: uuid::Uuid,
        run_id: uuid::Uuid,
        rollback: RollbackOutcome,
    },
    Effects {
        original: RuntimeStartEffectsError,
        rollback: RollbackOutcome,
    },
}

/// Reservation → run-linked-evidence (if `readiness_link` is `Some`,
/// fail-closed per REPAIR 4) → runtime effects/local commit → loop spawn,
/// in this exact order. `run_id` must already be durably created by the
/// caller (`AppState::create_or_reuse_run_for_start`) before calling this
/// function. `readiness_link` is `None` for deployments where the strict
/// daily-data readiness gate is not applicable (no `evaluation_id` was ever
/// computed for this start attempt) — such attempts proceed straight to
/// runtime effects, exactly as before this patch.
///
/// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 8:
/// `RuntimeStartEffects::reserve_local_ownership` is now the very first
/// call this function makes — strictly before the run-linked evidence
/// persist. On a run-link-persist failure, only this reservation is
/// released (via the same `rollback_failed_start_attempt` path every other
/// failure branch uses — `start_runtime_effects`/`spawn_loop` were never
/// called, so there is nothing else to roll back): no arm/begin/tick/spawn
/// ever happens, the durable run stays non-active (never reaches `Armed`),
/// and a prior legitimate owner (there cannot be one — reservation only
/// just succeeded for this exact `run_id`) is left untouched.
pub async fn advance_run_to_active<E: RuntimeStartEffects>(
    db: &PgPool,
    effects: &E,
    run_id: uuid::Uuid,
    readiness_link: Option<(uuid::Uuid, DateTime<Utc>)>,
    trace: &mut Vec<&'static str>,
) -> Result<(), RuntimeStartSequenceError> {
    if let Err(err) = effects.reserve_local_ownership(run_id).await {
        let rollback = rollback_failed_start_attempt(db, effects, run_id).await;
        return Err(RuntimeStartSequenceError::Effects {
            original: err,
            rollback,
        });
    }
    trace.push(TRACE_OWNERSHIP_RESERVED);

    if let Some((evaluation_id, linked_at_utc)) = readiness_link {
        let linked =
            persist_run_linked_readiness_evidence(db, evaluation_id, run_id, linked_at_utc).await;
        if !linked {
            let rollback = rollback_failed_start_attempt(db, effects, run_id).await;
            return Err(RuntimeStartSequenceError::RunLinkPersistFailed {
                evaluation_id,
                run_id,
                rollback,
            });
        }
        trace.push(TRACE_RUN_LINK_EVENT_PERSISTED);
    }

    if let Err(err) = effects.start_runtime_effects(run_id).await {
        let rollback = rollback_failed_start_attempt(db, effects, run_id).await;
        return Err(RuntimeStartSequenceError::Effects {
            original: err,
            rollback,
        });
    }
    trace.push(TRACE_LOCAL_BUNDLE_COMMITTED);

    if let Err(err) = effects.spawn_loop(run_id).await {
        let rollback = rollback_failed_start_attempt(db, effects, run_id).await;
        return Err(RuntimeStartSequenceError::Effects {
            original: err,
            rollback,
        });
    }
    trace.push(TRACE_LOOP_SPAWNED);
    Ok(())
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirements 2/4/5: the shared
/// fail-closed rollback [`advance_run_to_active`] runs on any failure.
///
/// 1. `effects.rollback_local_effects(run_id)` — clears every local
///    (in-process) effect this attempt may have committed, including its
///    loop-ownership reservation. Implementations own their own
///    run_id-scoped compare-and-clear so an older attempt's rollback can
///    never clear a newer run's committed state.
/// 2. Durable run rollback, phase-aware (requirement 4): if `run_id`
///    reached `Armed`/`Running`, move it to `Stopped` when
///    `start_phase_reached().cleanly_stoppable()`, otherwise `Halted`.
///    `AlreadyNonActive` when the run never reached `Armed`/`Running`
///    (naturally a no-op for a failure that happened before `arm_run`, e.g.
///    a reservation conflict).
///
/// The original error is always what the caller returns — this function
/// never masks it (requirement 5) — but every rollback outcome, including a
/// `fetch_run`/`stop_run`/`halt_run` failure, is now returned as structured
/// [`RollbackOutcome`] truth rather than merely logged.
async fn rollback_failed_start_attempt<E: RuntimeStartEffects>(
    db: &PgPool,
    effects: &E,
    run_id: uuid::Uuid,
) -> RollbackOutcome {
    let local = effects.rollback_local_effects(run_id).await;
    let phase_reached = effects.start_phase_reached();

    let (durable, durable_status_unknown) = match mqk_db::fetch_run(db, run_id).await {
        Ok(run)
            if matches!(
                run.status,
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
            ) =>
        {
            if phase_reached.cleanly_stoppable() {
                match mqk_db::stop_run(db, run_id).await {
                    Ok(()) => (DurableRollbackDisposition::Stopped, false),
                    Err(err) => {
                        tracing::error!(
                            run_id = %run_id,
                            "rollback_failed_start_attempt_stop_run_failed error={err}"
                        );
                        (DurableRollbackDisposition::TransitionFailed, true)
                    }
                }
            } else {
                match mqk_db::halt_run(db, run_id, Utc::now()).await {
                    Ok(()) => (DurableRollbackDisposition::Halted, false),
                    Err(err) => {
                        tracing::error!(
                            run_id = %run_id,
                            "rollback_failed_start_attempt_halt_run_failed error={err}"
                        );
                        (DurableRollbackDisposition::TransitionFailed, true)
                    }
                }
            }
        }
        Ok(_) => {
            // Already Created/Stopped/Halted — nothing to roll back
            // (idempotent: a retried rollback for the same run_id, or a
            // failure that happened before `arm_run`, both land here).
            (DurableRollbackDisposition::AlreadyNonActive, false)
        }
        Err(err) => {
            tracing::error!(
                run_id = %run_id,
                "rollback_failed_start_attempt_fetch_run_failed error={err}"
            );
            (DurableRollbackDisposition::QueryFailed, true)
        }
    };

    RollbackOutcome {
        phase_reached,
        local,
        durable,
        durable_status_unknown,
    }
}
