//! MULTI-SYMBOL-RUNTIME-CONFIG-01: pure multi-symbol runtime configuration layer.
//!
//! Builds a validated [`MultiSymbolRuntimeConfig`] from one of two sources:
//!
//! - **[`MultiSymbolConfigSource::EnvSingleSymbolFallback`]** — the legacy
//!   single-symbol env vars (`MQK_STRATEGY_SYMBOL`, first non-empty entry of
//!   `MQK_STRATEGY_IDS`, `MQK_STRATEGY_MD_TIMEFRAME`). Always produces exactly
//!   one [`SymbolStrategyAssignment`] and `max_concurrent_symbols = 1` (cap
//!   #1 default, preserves Tier A).
//! - **[`MultiSymbolConfigSource::WatchlistArtifactV2`]** — an approved
//!   `watchlist-v2` artifact (see `crate::watchlist_intake`), producing up to
//!   `MULTI_SYMBOL_HARD_CEILING` assignments.
//!
//! # Wiring status
//! This module is **not called from the legacy single-symbol
//! `tick_strategy_dispatch`, `loop_runner.rs`, or `POST
//! /api/v1/strategy/signal` path** — those remain single-symbol today.
//! It IS a real, wired config-construction seam consumed by the
//! autonomous-operations and dynamic-selection surfaces, including:
//! `daily_data_readiness.rs`, `routes/autonomous_daily_operator.rs`,
//! `state/autonomous_completed_bar_task.rs`,
//! `state/autonomous_daily_coordinator.rs` (multiple call sites),
//! `routes/market_data_readiness.rs`, and
//! `state/required_market_data_autofresh.rs` (all via
//! [`build_multi_symbol_runtime_config_from_env`]), plus
//! `state/lifecycle.rs`'s dynamic-selection start snapshot (via
//! [`read_multi_symbol_config_raw_inputs_from_env_and_fleet`]). Treat
//! this module as wired production config plumbing, not a dormant seam
//! awaiting a future patch.
//!
//! # Fail-closed reasons
//! [`MultiSymbolConfigError`] enumerates every way construction can fail.
//! Each variant's [`MultiSymbolConfigError::as_str`] is a stable string
//! suitable for logging / future API surfaces:
//!
//! - `multi_symbol_config_missing_symbol`
//! - `multi_symbol_config_missing_strategy_id`
//! - `multi_symbol_config_missing_timeframe`
//! - `multi_symbol_config_watchlist_not_v2`
//! - `multi_symbol_config_watchlist_not_approved`
//! - `multi_symbol_config_missing_assignment`
//! - `multi_symbol_config_concurrent_limit_exceeded`
//! - `multi_symbol_config_hard_ceiling_exceeded`
//!
//! # Selection semantics
//! [`build_multi_symbol_runtime_config_from_env_and_watchlist`] prefers an
//! approved `watchlist-v2` artifact; any failure constructing from it
//! (including "not v2", "not approved", or any of the watchlist-side
//! fail-closed reasons above) falls back to the legacy single-symbol env
//! path — per the design doc (`docs/design/native_multi_symbol_dispatch.md`
//! §4.1): "Invalid watchlist-v2 artifact -> falls back to
//! EnvSingleSymbolFallback (back-compat), logged, not an error." If the
//! legacy fallback *also* fails, the overall result is `Err` — fail-closed,
//! no config is ever fabricated.
//!
//! # Cap #1 (`max_concurrent_symbols`) — narrowing decision for this patch
//! The design doc (§6 cap #1) describes a richer behaviour for a future
//! patch: truncate `symbols` to the first `max_concurrent_symbols` entries
//! (artifact order) with excluded symbols surfaced for operator visibility.
//! That truncation/surfacing behaviour requires an additive field on
//! `WatchlistStatusResponse` (`api_types.rs` / `routes/watchlist.rs`), which
//! is out of scope for this patch (file-scope constraint). This patch instead
//! fails closed ([`MultiSymbolConfigError::ConcurrentLimitExceeded`] /
//! [`MultiSymbolConfigError::HardCeilingExceeded`]) if `symbols.len()` would
//! exceed `max_symbols_to_trade` or `MULTI_SYMBOL_HARD_CEILING`. In practice
//! this is unreachable via `crate::watchlist_intake::evaluate_watchlist_intake`
//! (which already enforces `symbols.len() <= max_symbols_to_trade <=
//! MULTI_SYMBOL_HARD_CEILING`); it exists as a defense-in-depth check on this
//! builder's own contract, independent of the upstream validator. The
//! truncate-and-surface behaviour remains open for a later patch.
//!
//! # Safety
//! - Pure functions: no network, no DB, no broker/OMS/order imports.
//! - `_from_env` wrappers read env vars via `std::env::var` only.
//! - [`MultiSymbolRuntimeConfig`] carries no live-routing field — there is no
//!   `approved_for_live` to set. The only path to
//!   [`MultiSymbolConfigSource::WatchlistArtifactV2`] is through
//!   `WatchlistIntakeOutcome::LoadedApproved`, which
//!   `crate::watchlist_intake::evaluate_watchlist_intake` never returns for
//!   an artifact with `approved_for_live=true` (hard live lock, unchanged).

use crate::watchlist_intake::{
    LoadedWatchlistArtifact, LoadedWatchlistArtifactV3, WatchlistIntakeOutcome,
    MULTI_SYMBOL_HARD_CEILING, WATCHLIST_SCHEMA_VERSION_V2, WATCHLIST_SCHEMA_VERSION_V3,
};

/// Schema version for [`MultiSymbolRuntimeConfig`] (design doc §4.1).
pub const MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION: &str = "multi-symbol-runtime-config-v1";

/// Legacy single-symbol env var (Tier A).
const ENV_STRATEGY_SYMBOL: &str = "MQK_STRATEGY_SYMBOL";
/// Legacy strategy fleet env var; only the first non-empty,
/// comma-separated entry is used here (mirrors `strategy_fleet` derivation
/// in `state.rs`).
const ENV_STRATEGY_IDS: &str = "MQK_STRATEGY_IDS";

// ---------------------------------------------------------------------------
// Types (design doc §4.1, §4.3)
// ---------------------------------------------------------------------------

/// One traded symbol mapped to one strategy and market-data timeframe
/// (design doc §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolStrategyAssignment {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe: String,
}

/// Where a [`MultiSymbolRuntimeConfig`] was built from (design doc §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiSymbolConfigSource {
    /// Legacy `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_IDS[0]` /
    /// `MQK_STRATEGY_MD_TIMEFRAME` env vars. Always exactly one symbol.
    EnvSingleSymbolFallback,
    /// An approved `watchlist-v2` artifact loaded from `path`
    /// (`MQK_PAPER_WATCHLIST_PATH`).
    WatchlistArtifactV2 { path: String },
}

/// Where an [`ExplicitMultiStrategyRuntimeConfig`] was built from. A
/// dedicated, separate enum from [`MultiSymbolConfigSource`] — not an added
/// variant on it — so every existing exhaustive match over
/// `MultiSymbolConfigSource` (all of which only ever see a
/// [`MultiSymbolRuntimeConfig`]'s source, never an explicit-v3 one) keeps
/// compiling without gaining a dead, provably-unreachable arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiStrategyConfigSourceV3 {
    /// An approved `watchlist-v3` artifact loaded from `path`
    /// (`MQK_PAPER_WATCHLIST_PATH`) — explicit per-symbol multi-strategy
    /// authorization (`MULTI-STRATEGY-RUNTIME-DISPATCH-01`).
    WatchlistArtifactV3 { path: String },
}

/// The central multi-symbol runtime configuration object (design doc §4.1).
///
/// Built once, pure, from either [`build_legacy_single_symbol_config`] or
/// [`build_multi_symbol_config_from_watchlist_artifact`] (or selected between
/// the two via [`build_multi_symbol_runtime_config_from_env_and_watchlist`]).
///
/// Carries no live-routing field — see module docs ("Safety").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiSymbolRuntimeConfig {
    /// Always [`MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Ordered; `symbols[0]` remains "primary" for back-compat telemetry.
    /// Never empty for an `Ok` result.
    pub symbols: Vec<SymbolStrategyAssignment>,
    /// Cap #1. `1` for [`MultiSymbolConfigSource::EnvSingleSymbolFallback`];
    /// `artifact.max_symbols_to_trade` for
    /// [`MultiSymbolConfigSource::WatchlistArtifactV2`].
    pub max_concurrent_symbols: usize,
    pub source: MultiSymbolConfigSource,
}

// ---------------------------------------------------------------------------
// Fail-closed error reasons
// ---------------------------------------------------------------------------

/// Every way [`MultiSymbolRuntimeConfig`] construction can fail.
///
/// All variants are fail-closed: none of them produce a partial or default
/// config. See module docs for the stable reason strings returned by
/// [`MultiSymbolConfigError::as_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiSymbolConfigError {
    /// Legacy fallback: `MQK_STRATEGY_SYMBOL` absent or empty after trim.
    MissingSymbol,
    /// Legacy fallback: `MQK_STRATEGY_IDS` absent or has no non-empty entry.
    MissingStrategyId,
    /// Either source: timeframe input absent or empty after trim.
    MissingTimeframe,
    /// Watchlist source: the approved artifact's `schema_version` is not
    /// `"watchlist-v2"` (a v1 artifact belongs to the legacy single-symbol
    /// path, not this builder).
    WatchlistNotV2,
    /// Watchlist source: `outcome` is not `LoadedApproved` (covers
    /// `NotConfigured`, `Missing`, `Invalid`, and `LoadedNotApproved`).
    WatchlistNotApproved,
    /// Watchlist source: a symbol in `artifact.symbols` has no corresponding
    /// entry in `artifact.strategy_assignments`. Defense-in-depth — already
    /// rejected by `evaluate_watchlist_intake` for v2, but checked again here
    /// independent of that validator.
    MissingAssignment { symbol: String },
    /// Watchlist source: `artifact.symbols.len() > artifact.max_symbols_to_trade`.
    /// Defense-in-depth — already rejected by `evaluate_watchlist_intake`.
    ConcurrentLimitExceeded { configured: usize, limit: usize },
    /// Watchlist source: `artifact.max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING`.
    /// Defense-in-depth — already rejected by `evaluate_watchlist_intake`.
    HardCeilingExceeded { configured: usize, ceiling: usize },
}

impl MultiSymbolConfigError {
    /// Stable reason string suitable for logging / future API surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingSymbol => "multi_symbol_config_missing_symbol",
            Self::MissingStrategyId => "multi_symbol_config_missing_strategy_id",
            Self::MissingTimeframe => "multi_symbol_config_missing_timeframe",
            Self::WatchlistNotV2 => "multi_symbol_config_watchlist_not_v2",
            Self::WatchlistNotApproved => "multi_symbol_config_watchlist_not_approved",
            Self::MissingAssignment { .. } => "multi_symbol_config_missing_assignment",
            Self::ConcurrentLimitExceeded { .. } => "multi_symbol_config_concurrent_limit_exceeded",
            Self::HardCeilingExceeded { .. } => "multi_symbol_config_hard_ceiling_exceeded",
        }
    }
}

// ---------------------------------------------------------------------------
// Explicit v3 (watchlist-v3) config-source error set
// ---------------------------------------------------------------------------

/// Every way [`ExplicitMultiStrategyRuntimeConfig`] construction can fail. A
/// dedicated, separate enum from [`MultiSymbolConfigError`] — not added
/// variants on it — so every existing exhaustive match over
/// `MultiSymbolConfigError` (all of which only ever see an error from the
/// v1/v2/legacy builders, never the explicit-v3 one) keeps compiling without
/// gaining a dead, provably-unreachable arm. All variants are fail-closed:
/// none of them produce a partial or default config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplicitMultiStrategyConfigError {
    /// `outcome` is not `WatchlistIntakeOutcome::LoadedApprovedV3` (covers
    /// `NotConfigured`, `Missing`, `Invalid`, `LoadedNotApprovedV3`, and any
    /// v1/v2 outcome).
    WatchlistNotApproved,
    /// `artifact.schema_version` is not `"watchlist-v3"`. Defense-in-depth —
    /// unreachable via `evaluate_watchlist_intake`, kept for
    /// hand-constructed fixtures.
    WatchlistNotV3,
    /// The artifact's shared `default_timeframe` input is absent or empty
    /// after trim.
    MissingTimeframe,
    /// The artifact's shared `default_timeframe` string does not parse to a
    /// known `timeframe_secs` value (`market_data_freshness::timeframe_secs`).
    UnsupportedTimeframeLabel { timeframe: String },
    /// `artifact.max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING`.
    /// Defense-in-depth — already rejected by `evaluate_watchlist_intake_v3`.
    HardCeilingExceeded { configured: usize, ceiling: usize },
    /// `artifact.symbols.len() > artifact.max_symbols_to_trade`.
    /// Defense-in-depth — already rejected by `evaluate_watchlist_intake_v3`.
    ConcurrentLimitExceeded { configured: usize, limit: usize },
    /// A symbol in `artifact.symbols` has no corresponding entry in
    /// `artifact.strategy_assignments`. Defense-in-depth — already rejected
    /// by `evaluate_watchlist_intake_v3` for v3 artifacts loaded from disk.
    MissingAssignment { symbol: String },
    /// The exact same `(symbol, strategy_id, timeframe_secs)` binding
    /// appeared more than once while flattening the artifact's
    /// `strategy_assignments` lists — never silently deduplicated into one
    /// authority. `evaluate_watchlist_intake_v3` bounds each symbol's list
    /// length but does not itself reject a repeated strategy id within one
    /// symbol's list; this builder is the fail-closed seam that does.
    DuplicateBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
}

impl ExplicitMultiStrategyConfigError {
    /// Stable reason string suitable for logging / future API surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WatchlistNotApproved => "explicit_multi_strategy_config_watchlist_not_approved",
            Self::WatchlistNotV3 => "explicit_multi_strategy_config_watchlist_not_v3",
            Self::MissingTimeframe => "explicit_multi_strategy_config_missing_timeframe",
            Self::UnsupportedTimeframeLabel { .. } => {
                "explicit_multi_strategy_config_unsupported_timeframe_label"
            }
            Self::HardCeilingExceeded { .. } => {
                "explicit_multi_strategy_config_hard_ceiling_exceeded"
            }
            Self::ConcurrentLimitExceeded { .. } => {
                "explicit_multi_strategy_config_concurrent_limit_exceeded"
            }
            Self::MissingAssignment { .. } => "explicit_multi_strategy_config_missing_assignment",
            Self::DuplicateBinding { .. } => "explicit_multi_strategy_config_duplicate_binding",
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 3 — legacy single-symbol fallback builder
// ---------------------------------------------------------------------------

/// Build a [`MultiSymbolRuntimeConfig`] from the legacy single-symbol inputs.
///
/// Pure: takes already-read, optional string values. Each is trimmed; an
/// absent or all-whitespace value fails closed with the corresponding
/// [`MultiSymbolConfigError`] variant, checked in this order: symbol,
/// strategy_id, timeframe.
///
/// On success, produces exactly one [`SymbolStrategyAssignment`] with
/// `source = MultiSymbolConfigSource::EnvSingleSymbolFallback` and
/// `max_concurrent_symbols = 1` (cap #1 default, preserves Tier A).
pub fn build_legacy_single_symbol_config(
    symbol: Option<&str>,
    strategy_id: Option<&str>,
    timeframe: Option<&str>,
) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
    let symbol = non_empty_trimmed(symbol).ok_or(MultiSymbolConfigError::MissingSymbol)?;
    let strategy_id =
        non_empty_trimmed(strategy_id).ok_or(MultiSymbolConfigError::MissingStrategyId)?;
    let timeframe = non_empty_trimmed(timeframe).ok_or(MultiSymbolConfigError::MissingTimeframe)?;

    Ok(MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: timeframe.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    })
}

/// [`build_legacy_single_symbol_config`], reading inputs from the
/// environment: `MQK_STRATEGY_SYMBOL`, the first non-empty entry of
/// `MQK_STRATEGY_IDS` (comma-separated), and `MQK_STRATEGY_MD_TIMEFRAME`
/// (`crate::state::STRATEGY_MD_TIMEFRAME_ENV`).
pub fn build_legacy_single_symbol_config_from_env(
) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
    let symbol = std::env::var(ENV_STRATEGY_SYMBOL).ok();
    let strategy_id = first_strategy_id_from_env();
    let timeframe = std::env::var(super::STRATEGY_MD_TIMEFRAME_ENV).ok();

    build_legacy_single_symbol_config(
        symbol.as_deref(),
        strategy_id.as_deref(),
        timeframe.as_deref(),
    )
}

// ---------------------------------------------------------------------------
// Phase 4 — watchlist-v2 artifact builder
// ---------------------------------------------------------------------------

/// Build a [`MultiSymbolRuntimeConfig`] from a watchlist intake outcome.
///
/// # Validation (in order; first failure wins — fail-closed)
/// 1. `outcome` must be `WatchlistIntakeOutcome::LoadedApproved` — anything
///    else (`NotConfigured`, `Missing`, `Invalid`, `LoadedNotApproved`) =>
///    [`MultiSymbolConfigError::WatchlistNotApproved`].
/// 2. `artifact.schema_version` must equal `"watchlist-v2"` => otherwise
///    [`MultiSymbolConfigError::WatchlistNotV2`] (a v1 artifact belongs to
///    [`build_legacy_single_symbol_config`], not this builder).
/// 3. `default_timeframe` must be non-empty after trim =>
///    [`MultiSymbolConfigError::MissingTimeframe`]. Timeframe is currently
///    global (Tier A); per-symbol overrides are a later patch.
/// 4. `artifact.max_symbols_to_trade <= MULTI_SYMBOL_HARD_CEILING` =>
///    otherwise [`MultiSymbolConfigError::HardCeilingExceeded`].
/// 5. `artifact.symbols.len() <= artifact.max_symbols_to_trade` =>
///    otherwise [`MultiSymbolConfigError::ConcurrentLimitExceeded`].
/// 6. Every entry in `artifact.symbols` must have a corresponding
///    `artifact.strategy_assignments` entry => otherwise
///    [`MultiSymbolConfigError::MissingAssignment`].
///
/// Steps 4-6 are defense-in-depth: `evaluate_watchlist_intake` already
/// enforces all three for v2 artifacts loaded from disk, but this builder
/// re-checks independently of that validator (e.g. for hand-constructed
/// `LoadedWatchlistArtifact` values in tests).
///
/// On success, `max_concurrent_symbols = artifact.max_symbols_to_trade` and
/// `source = MultiSymbolConfigSource::WatchlistArtifactV2 { path:
/// configured_path.to_string() }`.
pub fn build_multi_symbol_config_from_watchlist_artifact(
    outcome: &WatchlistIntakeOutcome,
    configured_path: &str,
    default_timeframe: Option<&str>,
) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
    let artifact = match outcome {
        WatchlistIntakeOutcome::LoadedApproved { artifact } => artifact,
        _ => return Err(MultiSymbolConfigError::WatchlistNotApproved),
    };

    if artifact.schema_version != WATCHLIST_SCHEMA_VERSION_V2 {
        return Err(MultiSymbolConfigError::WatchlistNotV2);
    }

    let timeframe =
        non_empty_trimmed(default_timeframe).ok_or(MultiSymbolConfigError::MissingTimeframe)?;

    let max_symbols_to_trade = artifact.max_symbols_to_trade as usize;
    if max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING as usize {
        return Err(MultiSymbolConfigError::HardCeilingExceeded {
            configured: max_symbols_to_trade,
            ceiling: MULTI_SYMBOL_HARD_CEILING as usize,
        });
    }

    if artifact.symbols.len() > max_symbols_to_trade {
        return Err(MultiSymbolConfigError::ConcurrentLimitExceeded {
            configured: artifact.symbols.len(),
            limit: max_symbols_to_trade,
        });
    }

    let symbols = symbol_assignments_from_artifact(artifact, timeframe)?;

    Ok(MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols,
        max_concurrent_symbols: max_symbols_to_trade,
        source: MultiSymbolConfigSource::WatchlistArtifactV2 {
            path: configured_path.to_string(),
        },
    })
}

/// Map every `artifact.symbols` entry to a [`SymbolStrategyAssignment`] using
/// `artifact.strategy_assignments` and the shared `timeframe`. Fails closed on
/// the first symbol missing its assignment entry.
fn symbol_assignments_from_artifact(
    artifact: &LoadedWatchlistArtifact,
    timeframe: &str,
) -> Result<Vec<SymbolStrategyAssignment>, MultiSymbolConfigError> {
    artifact
        .symbols
        .iter()
        .map(|symbol| {
            artifact
                .strategy_assignments
                .get(symbol)
                .map(|strategy_id| SymbolStrategyAssignment {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    timeframe: timeframe.to_string(),
                })
                .ok_or_else(|| MultiSymbolConfigError::MissingAssignment {
                    symbol: symbol.clone(),
                })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Phase 4b — watchlist-v3 explicit multi-strategy artifact builder
// (MULTI-STRATEGY-RUNTIME-DISPATCH-01, frozen contract §9)
// ---------------------------------------------------------------------------

/// The v3-aware analog of [`MultiSymbolRuntimeConfig`], for the explicit
/// per-symbol multi-strategy authorization path.
///
/// Deliberately **not** a [`MultiSymbolRuntimeConfig`]: that type's
/// `symbols: Vec<SymbolStrategyAssignment>` field has an established
/// one-assignment-per-symbol meaning relied on by its existing Tier
/// A/watchlist-v2 callers (`daily_data_readiness.rs`,
/// `autonomous_completed_bar_task.rs`, `autonomous_daily_coordinator.rs`,
/// `routes/market_data_readiness.rs`) — reusing it for v3 would either
/// silently narrow a multi-strategy symbol back to one strategy or silently
/// redefine what every existing caller of that field means. This is a wholly
/// separate, additive result type instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitMultiStrategyRuntimeConfig {
    /// Always [`WATCHLIST_SCHEMA_VERSION_V3`].
    pub schema_version: String,
    /// Flattened `(symbol, strategy_id, timeframe_secs)` binding set —
    /// [`crate::dynamic_selection_host_pool::HostPoolKey`]'s own shape, so a
    /// caller can feed this directly into
    /// [`crate::dynamic_selection_host_pool::DynamicSelectionHostPool::build`]
    /// with no translation. Ordered by artifact symbol order, then by each
    /// symbol's own artifact strategy-list order (frozen contract §5) —
    /// never `HashMap` iteration order.
    pub bindings: Vec<(String, String, i64)>,
    pub max_symbols_to_trade: usize,
    pub source: MultiStrategyConfigSourceV3,
}

/// Build an [`ExplicitMultiStrategyRuntimeConfig`] from a watchlist intake
/// outcome.
///
/// # Validation (in order; first failure wins — fail-closed)
/// 1. `outcome` must be `WatchlistIntakeOutcome::LoadedApprovedV3` — anything
///    else => [`ExplicitMultiStrategyConfigError::WatchlistNotApproved`].
/// 2. `artifact.schema_version` must equal `"watchlist-v3"` => otherwise
///    [`ExplicitMultiStrategyConfigError::WatchlistNotV3`].
/// 3. `default_timeframe` must be non-empty after trim and must parse to a
///    known `timeframe_secs` value (`market_data_freshness::timeframe_secs`)
///    => otherwise [`ExplicitMultiStrategyConfigError::MissingTimeframe`] /
///    [`ExplicitMultiStrategyConfigError::UnsupportedTimeframeLabel`].
///    Timeframe is currently global per artifact (frozen contract §3).
/// 4. `artifact.max_symbols_to_trade <= MULTI_SYMBOL_HARD_CEILING` =>
///    otherwise [`ExplicitMultiStrategyConfigError::HardCeilingExceeded`].
/// 5. `artifact.symbols.len() <= artifact.max_symbols_to_trade` =>
///    otherwise [`ExplicitMultiStrategyConfigError::ConcurrentLimitExceeded`].
/// 6. Flattening `artifact.symbols` x `artifact.strategy_assignments[symbol]`
///    (in artifact order both ways) must never repeat the exact same
///    `(symbol, strategy_id, timeframe_secs)` triple => otherwise
///    [`ExplicitMultiStrategyConfigError::DuplicateBinding`].
///
/// Steps 4-6 are defense-in-depth: `evaluate_watchlist_intake`/
/// `evaluate_watchlist_intake_v3` already enforce most of this for artifacts
/// loaded from disk, but this builder re-checks independently of that
/// validator (e.g. for hand-constructed `LoadedWatchlistArtifactV3` values in
/// tests), mirroring [`build_multi_symbol_config_from_watchlist_artifact`]'s
/// own rationale.
///
/// On success, `max_symbols_to_trade = artifact.max_symbols_to_trade` and
/// `source = MultiStrategyConfigSourceV3::WatchlistArtifactV3 { path:
/// configured_path.to_string() }`.
pub fn build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
    outcome: &WatchlistIntakeOutcome,
    configured_path: &str,
    default_timeframe: Option<&str>,
) -> Result<ExplicitMultiStrategyRuntimeConfig, ExplicitMultiStrategyConfigError> {
    let artifact: &LoadedWatchlistArtifactV3 = match outcome {
        WatchlistIntakeOutcome::LoadedApprovedV3 { artifact } => artifact,
        _ => return Err(ExplicitMultiStrategyConfigError::WatchlistNotApproved),
    };

    if artifact.schema_version != WATCHLIST_SCHEMA_VERSION_V3 {
        return Err(ExplicitMultiStrategyConfigError::WatchlistNotV3);
    }

    let timeframe_label = non_empty_trimmed(default_timeframe)
        .ok_or(ExplicitMultiStrategyConfigError::MissingTimeframe)?;
    let timeframe_secs =
        crate::market_data_freshness::timeframe_secs(timeframe_label).ok_or_else(|| {
            ExplicitMultiStrategyConfigError::UnsupportedTimeframeLabel {
                timeframe: timeframe_label.to_string(),
            }
        })?;

    let max_symbols_to_trade = artifact.max_symbols_to_trade as usize;
    if max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING as usize {
        return Err(ExplicitMultiStrategyConfigError::HardCeilingExceeded {
            configured: max_symbols_to_trade,
            ceiling: MULTI_SYMBOL_HARD_CEILING as usize,
        });
    }

    if artifact.symbols.len() > max_symbols_to_trade {
        return Err(ExplicitMultiStrategyConfigError::ConcurrentLimitExceeded {
            configured: artifact.symbols.len(),
            limit: max_symbols_to_trade,
        });
    }

    let mut bindings: Vec<(String, String, i64)> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, i64)> =
        std::collections::HashSet::new();
    for symbol in &artifact.symbols {
        let Some(strategy_ids) = artifact.strategy_assignments.get(symbol) else {
            return Err(ExplicitMultiStrategyConfigError::MissingAssignment {
                symbol: symbol.clone(),
            });
        };
        for strategy_id in strategy_ids {
            let key = (symbol.clone(), strategy_id.clone(), timeframe_secs);
            if !seen.insert(key.clone()) {
                let (symbol, strategy_id, timeframe_secs) = key;
                return Err(ExplicitMultiStrategyConfigError::DuplicateBinding {
                    symbol,
                    strategy_id,
                    timeframe_secs,
                });
            }
            bindings.push((symbol.clone(), strategy_id.clone(), timeframe_secs));
        }
    }

    Ok(ExplicitMultiStrategyRuntimeConfig {
        schema_version: WATCHLIST_SCHEMA_VERSION_V3.to_string(),
        bindings,
        max_symbols_to_trade,
        source: MultiStrategyConfigSourceV3::WatchlistArtifactV3 {
            path: configured_path.to_string(),
        },
    })
}

/// [`build_explicit_multi_strategy_config_from_watchlist_artifact_v3`],
/// reading `watchlist_outcome` via
/// `crate::watchlist_intake::evaluate_watchlist_intake_from_env` and the
/// configured path / timeframe from the same env vars as
/// [`build_multi_symbol_runtime_config_from_env`]'s watchlist-v2 sibling.
/// Not called by the real daemon-start path (Patch C3, `state/lifecycle.rs`)
/// -- that path already holds a frozen `StartAttemptAuthoritySnapshot` and
/// must never re-read the environment independently. Kept public for a
/// future direct env-based caller (CLI/test/status route), mirroring every
/// other builder in this file having this exact `_from_env` sibling.
#[allow(dead_code)]
pub fn build_explicit_multi_strategy_config_from_env(
) -> Result<ExplicitMultiStrategyRuntimeConfig, ExplicitMultiStrategyConfigError> {
    let watchlist_outcome = crate::watchlist_intake::evaluate_watchlist_intake_from_env();
    let configured_path =
        std::env::var(crate::watchlist_intake::ENV_PAPER_WATCHLIST_PATH).unwrap_or_default();
    let legacy_timeframe = std::env::var(super::STRATEGY_MD_TIMEFRAME_ENV).ok();
    build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
        &watchlist_outcome,
        &configured_path,
        legacy_timeframe.as_deref(),
    )
}

// ---------------------------------------------------------------------------
// Phase 5 — selection helper
// ---------------------------------------------------------------------------

/// Select between the watchlist-v2 source and the legacy single-symbol
/// fallback (design doc §4.1).
///
/// # Selection order
/// 1. If `watchlist_outcome` is `WatchlistIntakeOutcome::LoadedApproved` with
///    `schema_version == "watchlist-v2"`, try
///    [`build_multi_symbol_config_from_watchlist_artifact`]. If it succeeds,
///    return it (`source = WatchlistArtifactV2`).
/// 2. Otherwise — including when step 1 was skipped (not configured, not
///    approved, v1, `Invalid`) *or* the watchlist-v2 builder returned `Err`
///    for any reason — fall back to
///    [`build_legacy_single_symbol_config`] (`source =
///    EnvSingleSymbolFallback`). Per the design doc: "Invalid watchlist-v2
///    artifact -> falls back to EnvSingleSymbolFallback (back-compat),
///    logged, not an error."
/// 3. If the legacy fallback also fails, its [`MultiSymbolConfigError`] is
///    returned — fail-closed: no config is ever fabricated.
pub fn build_multi_symbol_runtime_config_from_env_and_watchlist(
    watchlist_outcome: &WatchlistIntakeOutcome,
    configured_watchlist_path: Option<&str>,
    legacy_symbol: Option<&str>,
    legacy_strategy_id: Option<&str>,
    legacy_timeframe: Option<&str>,
) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
    // A `LoadedApprovedV3`/`LoadedNotApprovedV3` outcome deliberately does
    // NOT match the `if let` below and falls through to the legacy
    // single-symbol builder — explicitly, not silently: this function
    // returns `MultiSymbolRuntimeConfig`, whose one-assignment-per-symbol
    // shape cannot represent a v3 artifact's multiple strategies per symbol
    // (frozen contract §9). A v3 artifact's own config source is
    // `ExplicitMultiStrategyRuntimeConfig`, built by
    // `build_explicit_multi_strategy_config_from_watchlist_artifact_v3`
    // above — a wholly separate seam, never this one. Callers that need the
    // v3 binding set must call that function directly, not this one.
    if let WatchlistIntakeOutcome::LoadedApproved { artifact } = watchlist_outcome {
        if artifact.schema_version == WATCHLIST_SCHEMA_VERSION_V2 {
            let path = configured_watchlist_path.unwrap_or("");
            if let Ok(cfg) = build_multi_symbol_config_from_watchlist_artifact(
                watchlist_outcome,
                path,
                legacy_timeframe,
            ) {
                return Ok(cfg);
            }
            // Fall through: invalid/unbuildable watchlist-v2 -> legacy, not an error.
        }
    }

    build_legacy_single_symbol_config(legacy_symbol, legacy_strategy_id, legacy_timeframe)
}

/// [`build_multi_symbol_runtime_config_from_env_and_watchlist`], reading
/// `watchlist_outcome` via `crate::watchlist_intake::evaluate_watchlist_intake_from_env`
/// and the legacy inputs from the same env vars as
/// [`build_legacy_single_symbol_config_from_env`].
///
/// Thin wrapper over [`read_multi_symbol_config_raw_inputs_from_env`] +
/// [`MultiSymbolConfigRawInputs::build_config`] — kept for existing callers
/// (`daily_data_readiness.rs`, `autonomous_completed_bar_task.rs`,
/// `autonomous_daily_coordinator.rs`, `routes/market_data_readiness.rs`)
/// that only need the resolved config, not the raw watchlist/env inputs a
/// single frozen start-attempt snapshot also needs to hand to
/// `market_data_freshness::required_symbols_with_source` without a second
/// watchlist read (BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 1).
pub fn build_multi_symbol_runtime_config_from_env(
) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
    read_multi_symbol_config_raw_inputs_from_env().build_config()
}

// ---------------------------------------------------------------------------
// BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 1 — one raw read, shared
// ---------------------------------------------------------------------------

/// Every env/watchlist input [`build_multi_symbol_runtime_config_from_env_and_watchlist`]
/// and `market_data_freshness::required_symbols_with_source` both need,
/// read exactly once.
///
/// A single frozen start-attempt snapshot (`StartAttemptAuthoritySnapshot`
/// in `state/lifecycle.rs`) reads this struct once via
/// [`read_multi_symbol_config_raw_inputs_from_env`] and derives both the
/// [`MultiSymbolRuntimeConfig`] (via [`MultiSymbolConfigRawInputs::build_config`])
/// and the premarket freshness gate's required-symbol vector from it — never
/// two independent watchlist-artifact reads or two independent legacy-env
/// reads within the same start attempt.
#[derive(Debug, Clone)]
pub(crate) struct MultiSymbolConfigRawInputs {
    pub(crate) watchlist_outcome: WatchlistIntakeOutcome,
    pub(crate) configured_watchlist_path: Option<String>,
    pub(crate) legacy_symbol: Option<String>,
    pub(crate) legacy_strategy_id: Option<String>,
    pub(crate) legacy_timeframe: Option<String>,
}

impl MultiSymbolConfigRawInputs {
    /// [`build_multi_symbol_runtime_config_from_env_and_watchlist`], driven
    /// from this already-resolved snapshot instead of re-reading env/the
    /// watchlist artifact.
    pub(crate) fn build_config(&self) -> Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError> {
        build_multi_symbol_runtime_config_from_env_and_watchlist(
            &self.watchlist_outcome,
            self.configured_watchlist_path.as_deref(),
            self.legacy_symbol.as_deref(),
            self.legacy_strategy_id.as_deref(),
            self.legacy_timeframe.as_deref(),
        )
    }
}

/// Read [`MultiSymbolConfigRawInputs`] from the environment: exactly one
/// `evaluate_watchlist_intake_from_env()` call and exactly one read each of
/// `MQK_PAPER_WATCHLIST_PATH`, `MQK_STRATEGY_SYMBOL`, `MQK_STRATEGY_IDS`
/// (first non-empty entry), and `MQK_STRATEGY_MD_TIMEFRAME`.
///
/// Kept for callers with no independently-resolved frozen fleet of their own
/// (`daily_data_readiness.rs`, `autonomous_completed_bar_task.rs`,
/// `autonomous_daily_coordinator.rs`, `routes/market_data_readiness.rs`).
/// The `start_execution_runtime` start path does **not** use this function —
/// see [`read_multi_symbol_config_raw_inputs_from_env_and_fleet`].
pub(crate) fn read_multi_symbol_config_raw_inputs_from_env() -> MultiSymbolConfigRawInputs {
    MultiSymbolConfigRawInputs {
        watchlist_outcome: crate::watchlist_intake::evaluate_watchlist_intake_from_env(),
        configured_watchlist_path: std::env::var(crate::watchlist_intake::ENV_PAPER_WATCHLIST_PATH)
            .ok(),
        legacy_symbol: std::env::var(ENV_STRATEGY_SYMBOL).ok(),
        legacy_strategy_id: first_strategy_id_from_env(),
        legacy_timeframe: std::env::var(super::STRATEGY_MD_TIMEFRAME_ENV).ok(),
    }
}

/// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE requirement 4:
/// [`read_multi_symbol_config_raw_inputs_from_env`], sourcing
/// `legacy_strategy_id` from `frozen_first_strategy_id` — the first entry of
/// an already-resolved, frozen start-attempt fleet capture — instead of a
/// second, independent `MQK_STRATEGY_IDS` read via
/// [`first_strategy_id_from_env`].
///
/// Every other field is read exactly once, exactly as
/// [`read_multi_symbol_config_raw_inputs_from_env`] reads it: one watchlist
/// evaluation, one legacy symbol read, one timeframe read. This is the
/// constructor `StartAttemptAuthoritySnapshot::resolve` (`state/lifecycle.rs`)
/// uses, so the legacy single-symbol assignment's `strategy_id` can never
/// disagree with the same frozen fleet the B1A bootstrap and the
/// dynamic-selection `configured_strategy_ids` universe were built from.
pub(crate) fn read_multi_symbol_config_raw_inputs_from_env_and_fleet(
    frozen_first_strategy_id: Option<&str>,
) -> MultiSymbolConfigRawInputs {
    MultiSymbolConfigRawInputs {
        watchlist_outcome: crate::watchlist_intake::evaluate_watchlist_intake_from_env(),
        configured_watchlist_path: std::env::var(crate::watchlist_intake::ENV_PAPER_WATCHLIST_PATH)
            .ok(),
        legacy_symbol: std::env::var(ENV_STRATEGY_SYMBOL).ok(),
        legacy_strategy_id: frozen_first_strategy_id.map(str::to_string),
        legacy_timeframe: std::env::var(super::STRATEGY_MD_TIMEFRAME_ENV).ok(),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// `Some(trimmed)` if `s` is `Some` and non-empty after trimming; `None` otherwise.
fn non_empty_trimmed(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|v| !v.is_empty())
}

/// First non-empty, trimmed entry of comma-separated `MQK_STRATEGY_IDS`
/// (mirrors the `strategy_fleet` derivation in `state.rs`).
fn first_strategy_id_from_env() -> Option<String> {
    std::env::var(ENV_STRATEGY_IDS).ok().and_then(|ids| {
        ids.split(',')
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(String::from)
    })
}

// ---------------------------------------------------------------------------
// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod frozen_fleet_raw_inputs_tests {
    use super::*;
    use tokio::sync::Mutex as TokioMutex;

    /// Serializes tests in this module that touch the process-global
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME`
    /// / `MQK_PAPER_WATCHLIST_PATH` env vars — delegates to the one
    /// crate-wide `strategy_fleet_env_test_lock` so these tests can never
    /// race `state::lifecycle`'s own env-mutating tests, which mutate the
    /// same process-global env vars.
    fn env_lock() -> &'static TokioMutex<()> {
        crate::state::shared_test_locks::strategy_fleet_env_test_lock()
    }

    fn clear_env() {
        std::env::remove_var(ENV_STRATEGY_SYMBOL);
        std::env::remove_var(ENV_STRATEGY_IDS);
        std::env::remove_var(super::super::STRATEGY_MD_TIMEFRAME_ENV);
        std::env::remove_var(crate::watchlist_intake::ENV_PAPER_WATCHLIST_PATH);
    }

    /// Requirement 4: `read_multi_symbol_config_raw_inputs_from_env_and_fleet`
    /// sources `legacy_strategy_id` from its `frozen_first_strategy_id`
    /// parameter, never from a second `MQK_STRATEGY_IDS` read — proven by
    /// setting the env var to a disagreeing value and confirming the frozen
    /// parameter wins.
    #[tokio::test]
    async fn legacy_strategy_id_comes_from_the_frozen_parameter_not_env() {
        let _guard = env_lock().lock().await;
        clear_env();
        std::env::set_var(ENV_STRATEGY_SYMBOL, "AAPL");
        std::env::set_var(ENV_STRATEGY_IDS, "env-strategy-should-be-ignored");
        std::env::set_var(super::super::STRATEGY_MD_TIMEFRAME_ENV, "5m");

        let raw = read_multi_symbol_config_raw_inputs_from_env_and_fleet(Some("frozen-strategy"));
        clear_env();

        assert_eq!(
            raw.legacy_strategy_id,
            Some("frozen-strategy".to_string()),
            "legacy_strategy_id must come from the frozen fleet parameter, \
             never from a second MQK_STRATEGY_IDS read"
        );
        let cfg = raw.build_config().expect("legacy config must resolve");
        assert_eq!(cfg.symbols[0].strategy_id, "frozen-strategy");
    }

    /// `frozen_first_strategy_id: None` (empty frozen fleet) must not
    /// fabricate a strategy id — legacy config resolution fails closed with
    /// `MissingStrategyId`, exactly as an absent `MQK_STRATEGY_IDS` did
    /// before this repair.
    #[tokio::test]
    async fn none_frozen_strategy_id_fails_closed_missing_strategy_id() {
        let _guard = env_lock().lock().await;
        clear_env();
        std::env::set_var(ENV_STRATEGY_SYMBOL, "AAPL");
        std::env::set_var(super::super::STRATEGY_MD_TIMEFRAME_ENV, "5m");

        let raw = read_multi_symbol_config_raw_inputs_from_env_and_fleet(None);
        clear_env();

        assert_eq!(raw.legacy_strategy_id, None);
        assert_eq!(
            raw.build_config(),
            Err(MultiSymbolConfigError::MissingStrategyId)
        );
    }

    /// Requirement 8: watchlist-v2 assignment admission is unaffected by the
    /// frozen fleet parameter — `build_multi_symbol_config_from_watchlist_
    /// artifact` never consults a configured-fleet vector at all; a symbol
    /// assigned to a strategy_id absent from the frozen fleet still resolves
    /// here (existing behavior, unchanged by this patch). The actual
    /// admission/registry check for that strategy_id happens downstream, in
    /// the durable `sys_strategy_registry`-backed dynamic-selection gate
    /// (`dynamic_selection_plan_builder.rs`), not in this pure config layer.
    #[test]
    fn watchlist_assignment_outside_any_fleet_still_resolves_at_this_layer() {
        let mut strategy_assignments = std::collections::HashMap::new();
        strategy_assignments.insert("MSFT".to_string(), "strategy-not-in-any-fleet".to_string());
        let artifact = LoadedWatchlistArtifact {
            schema_version: WATCHLIST_SCHEMA_VERSION_V2.to_string(),
            symbols: vec!["MSFT".to_string()],
            top_symbol: Some("MSFT".to_string()),
            strategy_assignments,
            max_symbols_to_trade: 1,
            max_concurrent_positions: 1,
            approved_for_autonomous_paper: false,
            dropped_symbols: vec![],
        };
        let outcome = WatchlistIntakeOutcome::LoadedApproved { artifact };

        let cfg =
            build_multi_symbol_config_from_watchlist_artifact(&outcome, "test-path", Some("5m"))
                .expect(
                    "this pure builder never filters by a configured fleet — \
                     fleet admission is a separate, downstream (DB-backed) concern",
                );
        assert_eq!(cfg.symbols[0].strategy_id, "strategy-not-in-any-fleet");
    }
}

// ---------------------------------------------------------------------------
// V4-STAGE-B-M2-REPAIR-03 / Patch C1 tests — explicit v3 config source
// ---------------------------------------------------------------------------
#[cfg(test)]
mod explicit_multi_strategy_v3_config_tests {
    use super::*;

    fn v3_artifact(
        symbols: &[&str],
        assignments: &[(&str, &[&str])],
        max_symbols_to_trade: u64,
    ) -> LoadedWatchlistArtifactV3 {
        let strategy_assignments = assignments
            .iter()
            .map(|(sym, ids)| (sym.to_string(), ids.iter().map(|s| s.to_string()).collect()))
            .collect();
        LoadedWatchlistArtifactV3 {
            schema_version: WATCHLIST_SCHEMA_VERSION_V3.to_string(),
            symbols: symbols.iter().map(|s| s.to_string()).collect(),
            top_symbol: symbols.first().map(|s| s.to_string()),
            strategy_assignments,
            max_symbols_to_trade,
            max_concurrent_positions: max_symbols_to_trade,
            approved_for_autonomous_paper: true,
            dropped_symbols: vec![],
        }
    }

    #[test]
    fn c1_happy_path_flattens_in_symbol_then_strategy_list_order() {
        let artifact = v3_artifact(
            &["AAPL", "MSFT"],
            &[
                ("AAPL", &["intraday_scalper", "intraday_short_scalper"]),
                ("MSFT", &["swing_momentum"]),
            ],
            2,
        );
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let cfg = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .expect("valid v3 artifact must build");
        assert_eq!(
            cfg.bindings,
            vec![
                ("AAPL".to_string(), "intraday_scalper".to_string(), 300),
                (
                    "AAPL".to_string(),
                    "intraday_short_scalper".to_string(),
                    300
                ),
                ("MSFT".to_string(), "swing_momentum".to_string(), 300),
            ]
        );
        assert_eq!(
            cfg.source,
            MultiStrategyConfigSourceV3::WatchlistArtifactV3 {
                path: "test-path".to_string()
            }
        );
    }

    #[test]
    fn c1_not_approved_outcome_fails_closed() {
        let outcome = WatchlistIntakeOutcome::NotConfigured;
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .unwrap_err();
        assert_eq!(err, ExplicitMultiStrategyConfigError::WatchlistNotApproved);
    }

    #[test]
    fn c1_v2_approved_outcome_is_not_approved_for_this_v3_only_builder() {
        let artifact = LoadedWatchlistArtifact {
            schema_version: WATCHLIST_SCHEMA_VERSION_V2.to_string(),
            symbols: vec!["AAPL".to_string()],
            top_symbol: Some("AAPL".to_string()),
            strategy_assignments: std::collections::HashMap::from([(
                "AAPL".to_string(),
                "intraday_scalper".to_string(),
            )]),
            max_symbols_to_trade: 1,
            max_concurrent_positions: 1,
            approved_for_autonomous_paper: true,
            dropped_symbols: vec![],
        };
        let outcome = WatchlistIntakeOutcome::LoadedApproved { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyConfigError::WatchlistNotApproved,
            "a v1/v2 LoadedApproved outcome must never be silently accepted by the v3-only builder"
        );
    }

    #[test]
    fn c1_missing_timeframe_fails_closed() {
        let artifact = v3_artifact(&["AAPL"], &[("AAPL", &["intraday_scalper"])], 1);
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            None,
        )
        .unwrap_err();
        assert_eq!(err, ExplicitMultiStrategyConfigError::MissingTimeframe);
    }

    #[test]
    fn c1_unsupported_timeframe_label_fails_closed() {
        let artifact = v3_artifact(&["AAPL"], &[("AAPL", &["intraday_scalper"])], 1);
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("not-a-real-timeframe"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyConfigError::UnsupportedTimeframeLabel {
                timeframe: "not-a-real-timeframe".to_string()
            }
        );
    }

    #[test]
    fn c1_hard_ceiling_exceeded_fails_closed() {
        let artifact = v3_artifact(
            &["AAPL"],
            &[("AAPL", &["intraday_scalper"])],
            MULTI_SYMBOL_HARD_CEILING + 1,
        );
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyConfigError::HardCeilingExceeded {
                configured: (MULTI_SYMBOL_HARD_CEILING + 1) as usize,
                ceiling: MULTI_SYMBOL_HARD_CEILING as usize,
            }
        );
    }

    #[test]
    fn c1_missing_assignment_fails_closed_defense_in_depth() {
        // Hand-constructed fixture bypassing evaluate_watchlist_intake_v3's
        // own guarantee — proves this builder re-checks independently.
        // max_symbols_to_trade=2 keeps the (earlier) concurrent-limit check
        // from firing first, so this specifically exercises the
        // missing-assignment check.
        let mut artifact = v3_artifact(&["AAPL"], &[("AAPL", &["intraday_scalper"])], 2);
        artifact.symbols.push("MSFT".to_string());
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyConfigError::MissingAssignment {
                symbol: "MSFT".to_string()
            }
        );
    }

    /// Duplicate exact binding (same strategy id repeated in one symbol's
    /// list) must never silently collapse into one authority nor silently
    /// double up — it must fail closed. `evaluate_watchlist_intake_v3` only
    /// bounds list length, so a hand-constructed fixture is needed to reach
    /// this defense-in-depth check (mirrors
    /// `c1_missing_assignment_fails_closed_defense_in_depth`'s rationale).
    #[test]
    fn c1_duplicate_binding_within_one_symbol_fails_closed() {
        let artifact = v3_artifact(
            &["AAPL"],
            &[("AAPL", &["intraday_scalper", "intraday_scalper"])],
            1,
        );
        let outcome = WatchlistIntakeOutcome::LoadedApprovedV3 { artifact };
        let err = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &outcome,
            "test-path",
            Some("5m"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyConfigError::DuplicateBinding {
                symbol: "AAPL".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                timeframe_secs: 300,
            }
        );
    }

    #[test]
    fn c1_reversed_symbol_order_input_produces_reversed_but_stable_binding_order() {
        // The builder preserves artifact order (frozen contract §5) rather
        // than re-sorting — proving the flattening is order-preserving, not
        // that two differently-ordered artifacts converge (that convergence
        // is HostPoolKey/DynamicSelectionHostPool's own BTreeMap-key
        // responsibility downstream, not this config layer's).
        let forward = v3_artifact(
            &["AAPL", "MSFT"],
            &[
                ("AAPL", &["intraday_scalper"]),
                ("MSFT", &["swing_momentum"]),
            ],
            2,
        );
        let reversed = v3_artifact(
            &["MSFT", "AAPL"],
            &[
                ("AAPL", &["intraday_scalper"]),
                ("MSFT", &["swing_momentum"]),
            ],
            2,
        );
        let forward_cfg = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &WatchlistIntakeOutcome::LoadedApprovedV3 { artifact: forward },
            "test-path",
            Some("5m"),
        )
        .unwrap();
        let reversed_cfg = build_explicit_multi_strategy_config_from_watchlist_artifact_v3(
            &WatchlistIntakeOutcome::LoadedApprovedV3 { artifact: reversed },
            "test-path",
            Some("5m"),
        )
        .unwrap();
        assert_eq!(
            forward_cfg.bindings,
            vec![
                ("AAPL".to_string(), "intraday_scalper".to_string(), 300),
                ("MSFT".to_string(), "swing_momentum".to_string(), 300),
            ]
        );
        assert_eq!(
            reversed_cfg.bindings,
            vec![
                ("MSFT".to_string(), "swing_momentum".to_string(), 300),
                ("AAPL".to_string(), "intraday_scalper".to_string(), 300),
            ]
        );
    }
}
