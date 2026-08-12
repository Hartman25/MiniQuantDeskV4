//! MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: automatic required-universe
//! market-data freshness controller.
//!
//! Before this patch, the daemon had strict readiness *gates*
//! (`market_data_freshness`, `daily_data_readiness`) and manual refresh
//! *tools* (`POST /api/v1/market-data/feed/poll-once`,
//! `POST /api/v1/market-data/feed/scheduler/start`), but no single
//! authoritative controller that (a) derived the complete set of
//! symbol/timeframe data the currently configured autonomous Paper
//! operation actually requires, (b) resolved each requirement's provider
//! through the canonical instrument/provider registries (never a hardcoded
//! or first-provider-wins default), and (c) kept that exact set fresh
//! automatically, before and throughout the trading session, without an
//! operator manually running a Windows refresh script or hand-typing a
//! poll-once request per symbol.
//!
//! This module supplies that missing layer. It does not replace any
//! existing seam:
//! - Required-symbol resolution reuses
//!   [`crate::market_data_freshness::required_symbols_with_source_from_env`]
//!   verbatim — the same resolver `GET /api/v1/market-data/ingest-plan` and
//!   the premarket readiness gate already use, so all three surfaces can
//!   never disagree about which symbols are required (§45 of the patch
//!   spec).
//! - Provider/timeframe admission reuses the same registry-based rules
//!   [`crate::state::market_data_latest_bar::resolve_latest_bar_poll_target`]
//!   already enforces (provider comes from the instrument's own registered
//!   `provider` field — never a fixed/default provider, never
//!   first-provider-wins).
//! - Freshness/sufficiency evaluation reuses
//!   [`crate::market_data_freshness::evaluate_md_freshness_status`] /
//!   [`aggregate_freshness_statuses`](crate::market_data_freshness::aggregate_freshness_statuses).
//! - The actual provider call + durable ingest reuses
//!   [`crate::state::market_data_latest_bar::poll_and_ingest_latest_closed_bar`]
//!   — no second HTTP client, no second Alpaca/TwelveData parser, no second
//!   `md_bars` writer.
//! - Session/calendar timing reuses
//!   [`crate::state::market_calendar::resolve_market_session_schedule`] — no
//!   hardcoded HST/ET wall-clock constant anywhere in this module.
//!
//! What is genuinely new here: (1) grouping resolved requirements by
//! `(provider_id, timeframe)` so a single scheduler cycle never mixes
//! incompatible provider/timeframe authority across symbols
//! ([`RequiredMarketDataRefreshPlan`]); (2) a typed, bounded-set distinction
//! between registry/provenance blockers (not fixed by polling — surfaced
//! and left alone) and freshness blockers (may trigger a bounded refresh
//! attempt) ([`RequirementConfigBlocker`]); (3) a required-universe
//! scheduler loop that maintains every requirement's freshness on a cadence
//! derived from the authoritative session schedule instead of a fixed
//! interval from process start.
//!
//! # Safety invariants
//! - This module writes only market-data tables (`md_bars`, via the reused
//!   ingest seam) and process-local status. It never calls
//!   `submit_order`/broker submit, never starts/stops the execution
//!   runtime, never arms/disarms, never clears halt or the kill switch,
//!   and never mutates reconcile state.
//! - Registry/provenance blockers are never "auto-repaired" by retrying —
//!   see [`RequirementConfigBlocker`].
//! - A required-universe cycle on a non-trading day makes zero provider
//!   calls (§39 of the patch spec).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use tokio::task::JoinHandle;

use mqk_md::instrument_registry::TrackedInstrument;
use mqk_md::provider_registry::ProviderConfig;

use crate::daily_data_readiness::{self, AssignmentReadiness};
use crate::market_data_freshness::{
    normalize_required_symbols, required_symbols_with_source_from_env, RequiredSymbolTimeframe,
    RequiredSymbolsResolution,
};
use crate::state::market_calendar::{
    active_calendar_provider_from_env, resolve_market_session_schedule, MarketCalendarProvider,
    MarketSessionSchedule,
};
use crate::state::market_data_latest_bar::{
    instrument_authorizes_timeframe, poll_and_ingest_latest_closed_bar,
    resolve_latest_bar_poll_target, ExpectedLatestBarConstraint, LatestBarPollOutcome,
    LatestBarPollSeamInput,
};
use crate::state::{
    build_multi_symbol_runtime_config_from_env, AppState, MultiSymbolConfigError,
    MultiSymbolRuntimeConfig, SymbolStrategyAssignment,
};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;

/// Post-session-close grace before the required-universe scheduler treats
/// intraday maintenance for `market_date` as complete (§22/§23/§session-
/// close proof of the patch spec). A *duration* added on top of the
/// authoritative `MarketSessionSchedule::session_close_utc` — never a fixed
/// wall-clock time.
pub const SESSION_CLOSE_POLL_BUFFER_SECS: i64 = 15 * 60;

// ---------------------------------------------------------------------------
// Requirement resolution (§7, §9, §19, §20)
// ---------------------------------------------------------------------------
//
// §7's "logical unit is symbol+timeframe, not a bare ticker" is satisfied by
// reusing `crate::market_data_freshness::RequiredSymbolTimeframe` — the
// existing (symbol, timeframe) pair type the premarket readiness gate and
// ingest-plan already resolve to — rather than introducing a second,
// structurally-identical type. `ResolvedRequirement`/`BlockedRequirement`
// below extend it with the provider identity this patch adds.

/// Typed, bounded-set registry/provenance blocker (§19/§20). These are
/// config-shaped defects — a stale/missing/invalid registry entry, a
/// disabled provider, an unauthorized timeframe — and are **not** fixed by
/// more polling. Callers must surface them and stop, never retry in a loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequirementConfigBlocker {
    /// The provider registry file could not be loaded/parsed, or the
    /// instrument's registered `provider` is not present in it.
    ProviderRegistryInvalid { detail: String },
    /// The instrument registry file could not be loaded/parsed, the symbol
    /// is absent, the instrument is disabled, or its `asset_class` is
    /// outside this patch's Paper equity/ETF scope.
    InstrumentRegistryInvalid { detail: String },
    /// Reserved for a caller-supplied provider identity that disagrees with
    /// the instrument's registered provider (not produced by plan
    /// resolution itself, which always reads the provider from the
    /// registry — see module docs; kept for symmetry with the ingest.rs
    /// admission rejection this mirrors).
    ProviderIdMismatch {
        configured_provider: String,
        requested_provider: String,
    },
    /// The instrument's canonical `provider_symbol` is blank.
    ProviderSymbolMismatch { detail: String },
    /// The requested timeframe is not authorized for this instrument, or is
    /// not declared as supported by the resolved provider.
    UnsupportedTimeframe { requested: String },
    /// The resolved provider is registered but `enabled=false`.
    ProviderDisabled { provider_id: String },
    /// The resolved provider does not declare support for the `equity`
    /// asset class.
    ProviderCapabilityMismatch { detail: String },
    /// A previously-ingested bar for this requirement carries provider
    /// metadata (`provider_id`/`provider_symbol`) that disagrees with the
    /// registry-resolved provider identity. Never auto-repaired: the
    /// mismatched row is left exactly as ingested and the requirement is
    /// blocked until an operator investigates (§ wrong-provider negative
    /// control).
    ProviderProvenanceInvalid { detail: String },
}

impl RequirementConfigBlocker {
    /// Machine-readable reason code, matching the bounded set named in the
    /// patch spec (§19/§20).
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ProviderRegistryInvalid { .. } => "provider_registry_invalid",
            Self::InstrumentRegistryInvalid { .. } => "instrument_registry_invalid",
            Self::ProviderIdMismatch { .. } => "provider_id_mismatch",
            Self::ProviderSymbolMismatch { .. } => "provider_symbol_mismatch",
            Self::UnsupportedTimeframe { .. } => "unsupported_timeframe",
            Self::ProviderDisabled { .. } => "provider_disabled",
            Self::ProviderCapabilityMismatch { .. } => "provider_capability_mismatch",
            Self::ProviderProvenanceInvalid { .. } => "provider_provenance_invalid",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::ProviderRegistryInvalid { detail }
            | Self::InstrumentRegistryInvalid { detail }
            | Self::ProviderSymbolMismatch { detail }
            | Self::ProviderCapabilityMismatch { detail }
            | Self::ProviderProvenanceInvalid { detail } => detail.clone(),
            Self::ProviderIdMismatch {
                configured_provider,
                requested_provider,
            } => format!(
                "configured provider '{configured_provider}' does not match requested provider \
                 '{requested_provider}'"
            ),
            Self::UnsupportedTimeframe { requested } => {
                format!("timeframe '{requested}' is not authorized/supported")
            }
            Self::ProviderDisabled { provider_id } => {
                format!("provider '{provider_id}' is disabled in the provider registry")
            }
        }
    }

    /// `true` for the provenance/registry class the patch spec says must
    /// never be retried by polling (§19). `false` for the pure freshness
    /// class ([`FreshnessBlocker`]) that lives outside this enum.
    pub fn is_registry_class(&self) -> bool {
        true
    }
}

/// One required requirement successfully resolved to a provider identity
/// through the canonical, validated registries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRequirement {
    pub symbol: String,
    pub timeframe: String,
    pub provider_id: String,
    pub provider_symbol: String,
}

/// One required requirement that could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedRequirement {
    pub symbol: String,
    pub timeframe: String,
    pub blocker: RequirementConfigBlocker,
}

/// Resolution outcome for one required `(symbol, timeframe)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RequirementResolution {
    Resolved(ResolvedRequirement),
    Blocked(BlockedRequirement),
}

impl RequirementResolution {
    pub fn symbol(&self) -> &str {
        match self {
            Self::Resolved(r) => &r.symbol,
            Self::Blocked(b) => &b.symbol,
        }
    }

    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}

/// One provider-call group: every resolved requirement sharing the same
/// `(provider_id, timeframe)` — a single provider poll/request must never
/// silently mix incompatible provider or timeframe authority (§10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTimeframeGroup {
    pub provider_id: String,
    pub timeframe: String,
    pub symbols: Vec<String>,
}

/// The one authoritative required-universe refresh plan for a given market
/// date (§10/§11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredMarketDataRefreshPlan {
    /// `YYYY-MM-DD`, Eastern-Time trading date the plan was resolved for.
    pub market_date: String,
    /// Which source produced the required-symbol set:
    /// `"watchlist_v2"` | `"env_strategy_symbol"` | `"none"`.
    pub symbol_source: String,
    pub resolutions: Vec<RequirementResolution>,
    pub groups: Vec<ProviderTimeframeGroup>,
}

impl RequiredMarketDataRefreshPlan {
    pub fn resolved(&self) -> impl Iterator<Item = &ResolvedRequirement> {
        self.resolutions.iter().filter_map(|r| match r {
            RequirementResolution::Resolved(resolved) => Some(resolved),
            RequirementResolution::Blocked(_) => None,
        })
    }

    pub fn blocked(&self) -> impl Iterator<Item = &BlockedRequirement> {
        self.resolutions.iter().filter_map(|r| match r {
            RequirementResolution::Resolved(_) => None,
            RequirementResolution::Blocked(blocked) => Some(blocked),
        })
    }

    pub fn required_symbols(&self) -> Vec<String> {
        self.resolutions
            .iter()
            .map(|r| r.symbol().to_string())
            .collect()
    }
}

/// Resolve one required `(symbol, timeframe)` pair against already-loaded,
/// already-validated instrument and provider registries. Pure: no DB,
/// network, or broker access. Provider identity always comes from the
/// instrument's own registered `provider` field (§9) — never a fixed
/// default, never first-provider-wins, never inferred from an API key or
/// existing DB rows.
fn resolve_one_requirement(
    instruments: &[TrackedInstrument],
    providers: &[ProviderConfig],
    req: &RequiredSymbolTimeframe,
) -> Result<ResolvedRequirement, RequirementConfigBlocker> {
    let instrument = instruments
        .iter()
        .find(|i| i.symbol.trim().eq_ignore_ascii_case(req.symbol.trim()))
        .ok_or_else(|| RequirementConfigBlocker::InstrumentRegistryInvalid {
            detail: format!(
                "symbol '{}' not found in instrument registry; cannot resolve canonical \
                 provider mapping",
                req.symbol
            ),
        })?;

    if !instrument.enabled {
        return Err(RequirementConfigBlocker::InstrumentRegistryInvalid {
            detail: format!(
                "instrument '{}' is disabled in the instrument registry",
                req.symbol
            ),
        });
    }
    if instrument.asset_class.trim() != "equity" {
        return Err(RequirementConfigBlocker::InstrumentRegistryInvalid {
            detail: format!(
                "instrument '{}' has asset_class '{}'; required-universe autofresh is scoped \
                 to Paper US equity/ETF only",
                req.symbol, instrument.asset_class
            ),
        });
    }

    let timeframe = mqk_md::Timeframe::parse(&req.timeframe).map_err(|_| {
        RequirementConfigBlocker::UnsupportedTimeframe {
            requested: req.timeframe.clone(),
        }
    })?;
    if !instrument_authorizes_timeframe(instrument, timeframe) {
        return Err(RequirementConfigBlocker::UnsupportedTimeframe {
            requested: req.timeframe.clone(),
        });
    }

    let provider_symbol = instrument.provider_symbol.trim();
    if provider_symbol.is_empty() {
        return Err(RequirementConfigBlocker::ProviderSymbolMismatch {
            detail: format!(
                "instrument '{}' has a blank canonical provider_symbol",
                req.symbol
            ),
        });
    }

    let configured_provider_id = instrument.provider.trim();
    let provider = providers
        .iter()
        .find(|p| p.provider_id.eq_ignore_ascii_case(configured_provider_id))
        .ok_or_else(|| RequirementConfigBlocker::ProviderRegistryInvalid {
            detail: format!(
                "instrument '{}' is configured for provider '{configured_provider_id}', which \
                 is not present in the provider registry",
                req.symbol
            ),
        })?;

    if !provider.enabled {
        return Err(RequirementConfigBlocker::ProviderDisabled {
            provider_id: provider.provider_id.clone(),
        });
    }
    if !provider.supports_asset_class("equity") {
        return Err(RequirementConfigBlocker::ProviderCapabilityMismatch {
            detail: format!(
                "provider '{}' does not declare support for asset_class 'equity'",
                provider.provider_id
            ),
        });
    }
    if !provider.supports_timeframe(timeframe.as_str()) {
        return Err(RequirementConfigBlocker::UnsupportedTimeframe {
            requested: req.timeframe.clone(),
        });
    }

    Ok(ResolvedRequirement {
        symbol: req.symbol.clone(),
        timeframe: timeframe.as_str().to_string(),
        provider_id: provider.provider_id.clone(),
        provider_symbol: provider_symbol.to_string(),
    })
}

/// Build the one authoritative [`RequiredMarketDataRefreshPlan`] for
/// `market_date` from an already-resolved required-symbol set and
/// already-loaded, already-validated registries. Pure: no DB, network, or
/// broker access — every input is caller-supplied.
///
/// Groups resolved requirements by `(provider_id, timeframe)` (§10);
/// iteration order over the input `BTreeMap` keys is deterministic, so the
/// same registry + required set always produces byte-identical group
/// ordering.
pub fn build_required_market_data_refresh_plan(
    instruments: &[TrackedInstrument],
    providers: &[ProviderConfig],
    resolution: &RequiredSymbolsResolution,
    market_date: (i64, i64, i64),
) -> RequiredMarketDataRefreshPlan {
    let normalized = normalize_required_symbols(&resolution.required);
    let mut resolutions = Vec::with_capacity(normalized.len());
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();

    for req in &normalized {
        match resolve_one_requirement(instruments, providers, req) {
            Ok(resolved) => {
                groups
                    .entry((resolved.provider_id.clone(), resolved.timeframe.clone()))
                    .or_default()
                    .push(resolved.symbol.clone());
                resolutions.push(RequirementResolution::Resolved(resolved));
            }
            Err(blocker) => {
                resolutions.push(RequirementResolution::Blocked(BlockedRequirement {
                    symbol: req.symbol.clone(),
                    timeframe: req.timeframe.clone(),
                    blocker,
                }));
            }
        }
    }

    let groups = groups
        .into_iter()
        .map(
            |((provider_id, timeframe), symbols)| ProviderTimeframeGroup {
                provider_id,
                timeframe,
                symbols,
            },
        )
        .collect();

    RequiredMarketDataRefreshPlan {
        market_date: format!(
            "{:04}-{:02}-{:02}",
            market_date.0, market_date.1, market_date.2
        ),
        symbol_source: resolution.source.to_string(),
        resolutions,
        groups,
    }
}

// ---------------------------------------------------------------------------
// Provenance guard (wrong-provider negative control)
// ---------------------------------------------------------------------------

/// The provider metadata durably stamped on the latest completed bar for a
/// `(symbol, timeframe)`, or `None` if no completed bar exists yet.
struct LatestBarProvenance {
    provider_id: Option<String>,
}

async fn latest_bar_provenance(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
) -> Result<Option<LatestBarProvenance>, String> {
    let row = sqlx::query(
        r#"
        select provider_id
        from md_bars
        where symbol = $1
          and timeframe = $2
          and is_complete = true
        order by end_ts desc
        limit 1
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .fetch_optional(pool)
    .await
    .map_err(|err| format!("latest_bar_provenance query failed: {err}"))?;

    Ok(row.map(|r| LatestBarProvenance {
        provider_id: r
            .try_get::<Option<String>, _>("provider_id")
            .unwrap_or(None),
    }))
}

/// Check that the most recently ingested completed bar for a resolved
/// requirement, if any exists, carries provider metadata agreeing with the
/// registry-resolved provider. Never rewrites/repairs a mismatched row —
/// only reports the mismatch as a blocker.
async fn check_requirement_provenance(
    pool: &PgPool,
    resolved: &ResolvedRequirement,
) -> Result<Option<RequirementConfigBlocker>, String> {
    let Some(provenance) =
        latest_bar_provenance(pool, &resolved.symbol, &resolved.timeframe).await?
    else {
        return Ok(None);
    };
    match provenance.provider_id {
        Some(stamped) if stamped.eq_ignore_ascii_case(&resolved.provider_id) => Ok(None),
        Some(stamped) => Ok(Some(RequirementConfigBlocker::ProviderProvenanceInvalid {
            detail: format!(
                "latest completed bar for {}/{} carries provider_id='{stamped}', which \
                 disagrees with the registry-resolved provider '{}'; not auto-repaired",
                resolved.symbol, resolved.timeframe, resolved.provider_id
            ),
        })),
        None => Ok(Some(RequirementConfigBlocker::ProviderProvenanceInvalid {
            detail: format!(
                "latest completed bar for {}/{} has no provider_id recorded",
                resolved.symbol, resolved.timeframe
            ),
        })),
    }
}

// ---------------------------------------------------------------------------
// Status surface (§26)
// ---------------------------------------------------------------------------

/// Per-requirement status row for the required-universe status surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredMarketDataStatus {
    pub symbol: String,
    pub timeframe: String,
    pub provider_id: Option<String>,
    pub provider_symbol: Option<String>,
    /// `"ready"` | one of the freshness states (`"missing"`, `"insufficient"`,
    /// `"stale"`, `"unavailable"`) | one of the registry blocker reason
    /// codes (§19/§20).
    pub freshness_state: String,
    pub latest_completed_bar_ts: Option<String>,
    pub blockers: Vec<String>,
}

/// Full required-universe status response (§26).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredUniverseStatusReport {
    pub truth_state: String,
    pub market_date: String,
    /// `"ready"` | `"blocked"` | `"not_applicable"`.
    pub overall_state: String,
    pub symbol_source: String,
    pub requirements: Vec<RequiredMarketDataStatus>,
    pub groups: Vec<ProviderTimeframeGroup>,
    pub is_trading_day: bool,
    pub session_open_utc: Option<String>,
    pub session_close_utc: Option<String>,
    pub last_poll_utc: Option<String>,
    pub next_poll_utc: Option<String>,
    pub provider_api_calls_made_this_cycle: u64,
    /// `true` when at least one required symbol became `manual_intervention_
    /// required`-shaped and an operator retry (AUTONOMOUS-DAILY-OPERATOR-
    /// RETRY-01, separate/out of scope for this controller) may be needed.
    /// This controller never invokes that retry itself (§33).
    pub operator_retry_required: bool,
}

// ---------------------------------------------------------------------------
// One controller cycle: resolve plan -> evaluate freshness -> bounded
// refresh -> re-evaluate (§14-18, §24-25, §34-35, §43, §46)
// ---------------------------------------------------------------------------

/// Load and validate the instrument registry at `path`. Mirrors
/// `routes::ingest::load_validated_instrument_registry` exactly (kept as a
/// separate small copy so this module has no dependency on the `routes`
/// layer — pure state/business logic only calls into `routes` never the
/// reverse in this codebase's layering).
fn load_validated_instrument_registry(path: &str) -> Result<Vec<TrackedInstrument>, String> {
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        path,
    ))
    .map_err(|error| {
        format!("instrument_registry_unavailable: failed to load instrument registry at '{path}': {error}")
    })?;
    mqk_md::instrument_registry::validate_registry(&instruments).map_err(|error| {
        format!("instrument_registry_invalid: instrument registry at '{path}' failed validation: {error}")
    })?;
    Ok(instruments)
}

fn load_validated_provider_registry(path: &str) -> Result<Vec<ProviderConfig>, String> {
    mqk_md::provider_registry::load_provider_registry(std::path::Path::new(path))
        .map_err(|err| format!("provider_registry_invalid: provider registry load failed: {err}"))
}

/// Outcome of one bounded refresh attempt for a single resolved requirement.
///
/// REPAIR-01 Defect D: the prior two-variant-plus-error shape
/// (`Skipped`/`Success`/`ProviderError`) conflated "no provider call was
/// made" with "a provider call was made and returned no usable data" — a
/// capability refusal (no call) and a historical fetch that genuinely
/// succeeded with zero completed bars (one real call) both surfaced as
/// `Skipped`, so `provider_api_calls_made_this_cycle` could under-report the
/// true number of provider method invocations. This shape makes "was the
/// provider actually invoked" the only axis the `Skipped`/`Skipped` mixup
/// could hide: [`RefreshAttemptOutcome::NoCall`] is the sole variant that
/// must not increment the counter; every other variant represents exactly
/// one real provider call (§18/§19 of the repair spec).
#[derive(Debug, Clone)]
enum RefreshAttemptOutcome {
    /// No provider method was invoked (capability/timeframe/admission
    /// refusal, or provider-client construction failure — a config
    /// problem, not a network call).
    NoCall,
    /// A provider call was made and returned usable data that was ingested.
    CalledSuccess,
    /// A provider call was made and returned no usable data (empty
    /// historical result, or no completed bar available yet).
    CalledNoData,
    /// A provider call was made and failed (transport error, rejected,
    /// provenance-invalid response, or the subsequent DB ingest failed).
    CalledError(String),
}

impl RefreshAttemptOutcome {
    /// `true` for every variant except [`Self::NoCall`] — the exact
    /// predicate `provider_api_calls_made_this_cycle` must count against
    /// (§19).
    fn is_actual_call(&self) -> bool {
        !matches!(self, Self::NoCall)
    }
}

/// Bounded historical lookback window, in calendar days, used only to
/// satisfy §17/§18's "bounded provider sync" requirement when a
/// requirement's freshness is `missing`/`insufficient`/`interior_gap` —
/// never a full history resync, and never re-run once the readiness
/// authority reports ready.
///
/// REPAIR-01 §4/§29: sized from the strategy's own `required_history_bars`
/// (sourced from [`daily_data_readiness::evaluate_assignment`], never a
/// fixed 5-bar assumption) instead of a flat per-timeframe constant, so a
/// strategy that genuinely needs more history than a handful of bars gets a
/// lookback window wide enough to have a chance of covering it in one
/// bounded attempt. Intraday timeframes still need far fewer calendar days
/// than daily to cover the same bar count (~78 5m bars/session).
fn bounded_bootstrap_lookback_days(
    timeframe: mqk_md::Timeframe,
    required_history_bars: usize,
) -> i64 {
    match timeframe {
        mqk_md::Timeframe::D1 => (required_history_bars as i64) * 2 + 10,
        _ => ((required_history_bars as i64 / 50) + 1) * 10,
    }
}

/// Resolve the provider client to use for one requirement's provider call:
/// the test-injected client when present, otherwise a real client built
/// from the registry config. Shared by both the bounded historical
/// bootstrap and the bounded latest-bar refresh so neither path can drift
/// from the other's provider-construction policy.
enum ProviderHandle {
    Injected(Arc<dyn mqk_md::MarketDataProvider>),
    Built(mqk_md::MarketDataProviderBox),
}

impl ProviderHandle {
    fn as_dyn(&self) -> &dyn mqk_md::MarketDataProvider {
        match self {
            ProviderHandle::Injected(p) => p.as_ref(),
            ProviderHandle::Built(p) => p.as_ref(),
        }
    }
}

fn resolve_provider_handle(
    provider_config: &ProviderConfig,
    injected_client: Option<&Arc<dyn mqk_md::MarketDataProvider>>,
) -> Result<ProviderHandle, String> {
    if let Some(client) = injected_client {
        return Ok(ProviderHandle::Injected(client.clone()));
    }
    mqk_md::build_market_data_provider_from_config(provider_config, |name| std::env::var(name).ok())
        .map(ProviderHandle::Built)
        .map_err(|error| error.to_string())
}

/// Attempt one bounded historical-bars provider call to satisfy §17/§18's
/// "historical/readiness bootstrap" responsibility — distinct from the
/// latest-closed-bar intraday maintenance path. Requests a small, fixed
/// calendar-day window (never the provider's full available history) and
/// writes every returned completed bar through the same metadata-aware
/// ingest function `poll_and_ingest_latest_closed_bar` uses internally, so
/// provenance (`provider_id`/`provider_symbol`/`ingest_mode`) is truthful
/// and md_bars upsert idempotency is preserved for repeated calls with
/// overlapping ranges (§24).
async fn attempt_bounded_historical_bootstrap(
    pool: &PgPool,
    provider_config: &ProviderConfig,
    injected_client: Option<&Arc<dyn mqk_md::MarketDataProvider>>,
    resolved: &ResolvedRequirement,
    required_history_bars: usize,
    now_utc: DateTime<Utc>,
) -> RefreshAttemptOutcome {
    let Ok(timeframe) = mqk_md::Timeframe::parse(&resolved.timeframe) else {
        return RefreshAttemptOutcome::NoCall;
    };
    // Provider-client construction is a config-shaped step (registry
    // lookup/credential resolution), not a network call — a failure here
    // must not be counted as an actual provider invocation (§19).
    let handle = match resolve_provider_handle(provider_config, injected_client) {
        Ok(handle) => handle,
        Err(_error) => return RefreshAttemptOutcome::NoCall,
    };
    let provider = handle.as_dyn();
    let capabilities = provider.capabilities();
    if !capabilities.historical_bars {
        return RefreshAttemptOutcome::NoCall;
    }

    let lookback_days = bounded_bootstrap_lookback_days(timeframe, required_history_bars);
    let end = now_utc.date_naive();
    let start = end - chrono::Duration::days(lookback_days);

    let bars = match provider
        .fetch_historical_bars(mqk_md::HistoricalBarsRequest {
            symbols: vec![resolved.provider_symbol.clone()],
            timeframe,
            start,
            end,
        })
        .await
    {
        Ok(bars) => bars,
        // Every historical-fetch error (rate-limited, provider-unavailable,
        // or otherwise) is reported the same way here: a bootstrap failure
        // never retries within this bounded call (§34/§35) regardless of
        // whether the underlying cause is transient. The call itself DID
        // happen — this counts against the provider-call counter (§19).
        Err(error) => return RefreshAttemptOutcome::CalledError(error.to_string()),
    };

    let completed_bars: Vec<mqk_db::md::ProviderBar> = bars
        .into_iter()
        .filter(|b| b.is_complete && b.symbol.eq_ignore_ascii_case(&resolved.provider_symbol))
        .map(|b| mqk_db::md::ProviderBar {
            symbol: resolved.symbol.clone(),
            timeframe: timeframe.as_str().to_string(),
            end_ts: b.end_ts,
            open: b.open,
            high: b.high,
            low: b.low,
            close: b.close,
            volume: b.volume,
            is_complete: b.is_complete,
        })
        .collect();

    if completed_bars.is_empty() {
        // The call happened and succeeded; the provider simply had nothing
        // to return in the requested window (§19 "empty historical result").
        return RefreshAttemptOutcome::CalledNoData;
    }

    let ingest_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-required-universe-bootstrap.v1|{}|{}|{}|{}|{}",
            resolved.provider_id,
            timeframe.as_str(),
            resolved.symbol,
            start,
            end
        )
        .as_bytes(),
    );

    match mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: resolved.provider_id.clone(),
            timeframe: timeframe.as_str().to_string(),
            ingest_id,
            bars: completed_bars,
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: resolved.provider_id.clone(),
            provider_source: Some(resolved.provider_id.clone()),
            provider_symbol: Some(resolved.provider_symbol.clone()),
            ingest_mode: Some("required_universe_bootstrap".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    {
        Ok(_) => RefreshAttemptOutcome::CalledSuccess,
        Err(error) => RefreshAttemptOutcome::CalledError(format!("db ingest failed: {error}")),
    }
}

/// Attempt exactly one bounded latest-closed-bar poll for `resolved`,
/// reusing the same production seam the manual poll-once route and the
/// Phase C autonomous driver use. Never loops, never retries within this
/// call — one call in, at most one provider call out (§18/§34/§35 bounded
/// refresh).
/// Attempt exactly one bounded latest-closed-bar poll for `resolved`,
/// reusing the same production seam the manual poll-once route and the
/// Phase C autonomous driver use. Never loops, never retries within this
/// call — one call in, at most one provider call out (§18/§34/§35 bounded
/// refresh).
///
/// REPAIR-01 §24: `expected_bar_constraint`, when `Some`, is the strict
/// readiness authority's own `expected_latest_bar_ts` for this requirement
/// (never `None` when the caller is repairing a genuine
/// `expected_latest_bar_missing` blocker) — so a provider that returns an
/// older, lagging bar is recorded as `ProviderLaggingExpectedBar` rather
/// than silently accepted as satisfying today's expectation (§24 "provider
/// lag").
#[allow(clippy::too_many_arguments)]
async fn attempt_bounded_refresh(
    pool: &PgPool,
    instruments: &[TrackedInstrument],
    provider_config: &ProviderConfig,
    injected_client: Option<&Arc<dyn mqk_md::MarketDataProvider>>,
    resolved: &ResolvedRequirement,
    expected_bar_constraint: Option<ExpectedLatestBarConstraint>,
    now_utc: DateTime<Utc>,
) -> RefreshAttemptOutcome {
    let Ok(timeframe) = mqk_md::Timeframe::parse(&resolved.timeframe) else {
        return RefreshAttemptOutcome::NoCall;
    };
    let target = match resolve_latest_bar_poll_target(
        instruments,
        &provider_config.provider_id,
        &resolved.symbol,
        timeframe,
    ) {
        Ok(target) => target,
        Err(_) => return RefreshAttemptOutcome::NoCall,
    };

    let handle = match resolve_provider_handle(provider_config, injected_client) {
        Ok(handle) => handle,
        Err(_error) => return RefreshAttemptOutcome::NoCall,
    };

    match poll_and_ingest_latest_closed_bar(
        pool,
        LatestBarPollSeamInput {
            provider: handle.as_dyn(),
            target: &target,
            now_utc,
            ingest_mode: "required_universe_autofresh",
            expected_bar_constraint,
        },
    )
    .await
    {
        LatestBarPollOutcome::InsertedNewBar { .. }
        | LatestBarPollOutcome::AlreadyStored { .. }
        | LatestBarPollOutcome::ProviderLaggingExpectedBar { .. }
        | LatestBarPollOutcome::UnexpectedOrFutureBar { .. } => {
            RefreshAttemptOutcome::CalledSuccess
        }
        LatestBarPollOutcome::NoCompletedBarAvailable => RefreshAttemptOutcome::CalledNoData,
        // Capability/timeframe refusal decided before any provider call was
        // dispatched (see `poll_and_ingest_latest_closed_bar`'s own guard) —
        // not an actual invocation (§19).
        LatestBarPollOutcome::Unsupported { .. } => RefreshAttemptOutcome::NoCall,
        LatestBarPollOutcome::ProviderRejected { detail }
        | LatestBarPollOutcome::ProviderTemporarilyUnavailable { detail } => {
            RefreshAttemptOutcome::CalledError(detail)
        }
        LatestBarPollOutcome::ProvenanceRejected { detail, .. } => {
            RefreshAttemptOutcome::CalledError(detail)
        }
        LatestBarPollOutcome::DatabaseFailure { detail, .. } => {
            RefreshAttemptOutcome::CalledError(detail)
        }
    }
}

// ---------------------------------------------------------------------------
// Strict readiness authority integration (REPAIR-01 Defect A, §2-4)
// ---------------------------------------------------------------------------
//
// Before this repair, `run_required_universe_cycle` decided missing/
// insufficient/stale/ok purely from `market_data_freshness::
// evaluate_md_freshness_status` — a fixed 5-completed-bar, wall-clock-age
// evaluator that can disagree with the strict readiness authority
// (`daily_data_readiness::evaluate_assignment`) autonomous Paper trading's
// own start gate uses, most concretely on how many bars a requirement
// actually needs (the *strategy's* `required_history_bars`, not a hardcoded
// constant) and on session-anchored (not wall-clock-age) staleness.
//
// This integration reuses `evaluate_assignment` verbatim — never a second,
// hand-rolled readiness calculator — via the same "synthetic binding"
// pattern `dynamic_selection_plan_builder::evaluate_candidate` already
// establishes: construct an `EffectiveRuntimeBinding`/`SymbolStrategyAssignment`
// pair that trivially matches the one requirement under evaluation, which
// neutralizes `evaluate_assignment`'s binding-mismatch checks (those exist
// to gate Tier A's single *actively bound* runtime strategy, not a
// required-universe requirement that may not be the active binding at all)
// and lets the real bar/provider/calendar readiness logic run standalone.

/// The exact bounded set of readiness blockers this controller may attempt
/// to repair by polling (§21). Every other blocker
/// `daily_data_readiness::evaluate_assignment` can produce is a
/// registry/provenance/binding/calendar-config defect — not remediable by
/// more polling, and never retried here.
const REFRESHABLE_READINESS_REASONS: &[&str] = &[
    daily_data_readiness::REASON_MARKET_DATA_MISSING,
    daily_data_readiness::REASON_INSUFFICIENT_HISTORY,
    daily_data_readiness::REASON_INTERIOR_GAP,
    daily_data_readiness::REASON_EXPECTED_LATEST_BAR_MISSING,
];

fn is_refreshable_reason(reason: &str) -> bool {
    REFRESHABLE_READINESS_REASONS.contains(&reason)
}

/// `true` when the bounded historical-bars bootstrap (not the latest-bar
/// poll) is the correct repair for `blockers` (§17/§22): an interior gap
/// cannot be closed by observing only the newest bar, and a missing/
/// insufficient history requirement needs the same wider provider fetch.
fn needs_historical_bootstrap(blockers: &[&'static str]) -> bool {
    blockers.contains(&daily_data_readiness::REASON_MARKET_DATA_MISSING)
        || blockers.contains(&daily_data_readiness::REASON_INSUFFICIENT_HISTORY)
        || blockers.contains(&daily_data_readiness::REASON_INTERIOR_GAP)
}

/// Derive this requirement's `freshness_state` status-surface string from a
/// finished [`AssignmentReadiness`] evaluation (§26). Refreshable reasons
/// take priority over other simultaneously-present blockers so the status
/// surface reflects what the controller is actually acting on; falls back to
/// the first blocker for the non-refreshable (registry/binding/calendar)
/// case.
fn freshness_state_from_readiness(readiness: &AssignmentReadiness) -> String {
    if readiness.is_ready() {
        return "ok".to_string();
    }
    if matches!(readiness.readiness_state, "db_unavailable" | "query_failed") {
        return "unavailable".to_string();
    }
    for reason in REFRESHABLE_READINESS_REASONS {
        if readiness.blockers.contains(reason) {
            return (*reason).to_string();
        }
    }
    readiness
        .blockers
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "blocked".to_string())
}

/// Resolve the one [`SymbolStrategyAssignment`] the current multi-symbol
/// runtime configuration assigns to `symbol` — the strategy identity
/// [`daily_data_readiness::evaluate_assignment`]'s `required_history_bars`
/// (§4/§29) and binding checks are evaluated against. `Err` is a
/// registry-class blocker (never auto-repaired by polling): either the
/// multi-symbol runtime configuration itself could not be built, or this
/// exact required symbol has no assignment in it (can only happen if the
/// required-symbol resolver and the multi-symbol config builder — which
/// read the same watchlist/legacy env inputs — momentarily disagree, e.g. a
/// watchlist file edited between the two reads within one cycle).
fn resolve_strategy_assignment<'a>(
    multi_symbol_config: &'a Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError>,
    symbol: &str,
) -> Result<&'a SymbolStrategyAssignment, String> {
    match multi_symbol_config {
        Ok(cfg) => cfg
            .symbols
            .iter()
            .find(|a| a.symbol.trim().eq_ignore_ascii_case(symbol.trim()))
            .ok_or_else(|| {
                format!(
                    "required symbol '{symbol}' has no resolvable strategy assignment in the \
                     current multi-symbol runtime configuration"
                )
            }),
        Err(err) => Err(format!(
            "multi-symbol runtime configuration could not be resolved: {}",
            err.as_str()
        )),
    }
}

/// Evaluate the strict readiness authority for one resolved requirement.
/// `Err` is a registry-class blocker (strategy assignment/plugin-registry
/// construction failure) — never retried by polling; `Ok` is the real
/// [`AssignmentReadiness`] verdict, ready or blocked.
async fn evaluate_strict_readiness_for_resolved(
    db_pool: &PgPool,
    resolved: &ResolvedRequirement,
    multi_symbol_config: &Result<MultiSymbolRuntimeConfig, MultiSymbolConfigError>,
    calendar_provider: &dyn MarketCalendarProvider,
    providers: &[ProviderConfig],
    instruments: &[TrackedInstrument],
    now_utc: DateTime<Utc>,
) -> Result<AssignmentReadiness, String> {
    let assignment = resolve_strategy_assignment(multi_symbol_config, &resolved.symbol)?;
    let strategy_id = assignment.strategy_id.clone();

    let mut registry = mqk_strategy::PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies(&mut registry, resolved.symbol.clone())
        .map_err(|error| format!("strategy registry construction failed: {error}"))?;

    let synthetic_binding = EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(strategy_id.clone()),
        effective_runtime_target_symbol: Some(resolved.symbol.clone()),
        effective_runtime_timeframe_secs: mqk_md::Timeframe::parse(&resolved.timeframe)
            .ok()
            .map(|tf| tf.duration_secs()),
    };
    let synthetic_assignment = SymbolStrategyAssignment {
        symbol: resolved.symbol.clone(),
        strategy_id,
        timeframe: resolved.timeframe.clone(),
    };

    Ok(daily_data_readiness::evaluate_assignment(
        Some(db_pool),
        &synthetic_assignment,
        &synthetic_binding,
        calendar_provider,
        providers,
        instruments,
        &registry,
        now_utc,
    )
    .await)
}

/// Result of one full required-universe controller cycle.
pub struct RequiredUniverseCycleResult {
    pub report: RequiredUniverseStatusReport,
    pub provider_api_calls_made: u64,
}

/// Run exactly one required-universe controller cycle: resolve the plan,
/// evaluate current freshness for every resolved requirement, and — only
/// for requirements blocked by a freshness reason (never a registry/
/// provenance reason, §19) on a trading day within the session+grace window
/// — attempt one bounded refresh per requirement, then re-evaluate.
///
/// `dry_run=true` performs zero provider calls and zero DB writes (§46
/// checkonly path) — it still reads `md_bars` (read-only) to report current
/// freshness.
#[allow(clippy::too_many_arguments)]
pub async fn run_required_universe_cycle(
    pool: Option<&PgPool>,
    instrument_registry_path: &str,
    provider_registry_path: &str,
    injected_provider_client: Option<&Arc<dyn mqk_md::MarketDataProvider>>,
    calendar_provider: &dyn MarketCalendarProvider,
    now_utc: DateTime<Utc>,
    dry_run: bool,
) -> RequiredUniverseCycleResult {
    let resolution = required_symbols_with_source_from_env();
    let schedule = resolve_market_session_schedule(calendar_provider, now_utc);

    if resolution.required.is_empty() {
        return RequiredUniverseCycleResult {
            report: not_applicable_report(&resolution, &schedule),
            provider_api_calls_made: 0,
        };
    }

    if !schedule.is_trading_day {
        return RequiredUniverseCycleResult {
            report: not_applicable_report(&resolution, &schedule),
            provider_api_calls_made: 0,
        };
    }

    let instruments = match load_validated_instrument_registry(instrument_registry_path) {
        Ok(instruments) => instruments,
        Err(error) => {
            return RequiredUniverseCycleResult {
                report: registry_unavailable_report(
                    &resolution,
                    &schedule,
                    "instrument_registry_invalid",
                    error,
                ),
                provider_api_calls_made: 0,
            };
        }
    };
    let providers = match load_validated_provider_registry(provider_registry_path) {
        Ok(providers) => providers,
        Err(error) => {
            return RequiredUniverseCycleResult {
                report: registry_unavailable_report(
                    &resolution,
                    &schedule,
                    "provider_registry_invalid",
                    error,
                ),
                provider_api_calls_made: 0,
            };
        }
    };

    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution,
        schedule.market_date,
    );

    // §2-4: the strict readiness authority's `required_history_bars` comes
    // from the strategy actually assigned to each required symbol, resolved
    // once per cycle from the same watchlist/legacy-env inputs the
    // required-symbol resolver above just read.
    let multi_symbol_config = build_multi_symbol_runtime_config_from_env();

    let mut statuses: Vec<RequiredMarketDataStatus> = Vec::with_capacity(plan.resolutions.len());
    let mut provider_api_calls_made = 0_u64;

    let now_ts = now_utc.timestamp();
    let past_close_grace =
        now_ts > schedule.session_close_utc.timestamp() + SESSION_CLOSE_POLL_BUFFER_SECS;

    for resolution_entry in &plan.resolutions {
        match resolution_entry {
            RequirementResolution::Blocked(blocked) => {
                statuses.push(RequiredMarketDataStatus {
                    symbol: blocked.symbol.clone(),
                    timeframe: blocked.timeframe.clone(),
                    provider_id: None,
                    provider_symbol: None,
                    freshness_state: blocked.blocker.reason_code().to_string(),
                    latest_completed_bar_ts: None,
                    blockers: vec![blocked.blocker.detail()],
                });
            }
            RequirementResolution::Resolved(resolved) => {
                let Some(db_pool) = pool else {
                    statuses.push(RequiredMarketDataStatus {
                        symbol: resolved.symbol.clone(),
                        timeframe: resolved.timeframe.clone(),
                        provider_id: Some(resolved.provider_id.clone()),
                        provider_symbol: Some(resolved.provider_symbol.clone()),
                        freshness_state: "unavailable".to_string(),
                        latest_completed_bar_ts: None,
                        blockers: vec!["no_db: database pool is not configured".to_string()],
                    });
                    continue;
                };

                // Provenance guard first: a previously-mis-stamped bar is a
                // registry-class blocker, never repaired by polling.
                match check_requirement_provenance(db_pool, resolved).await {
                    Ok(Some(blocker)) => {
                        statuses.push(RequiredMarketDataStatus {
                            symbol: resolved.symbol.clone(),
                            timeframe: resolved.timeframe.clone(),
                            provider_id: Some(resolved.provider_id.clone()),
                            provider_symbol: Some(resolved.provider_symbol.clone()),
                            freshness_state: blocker.reason_code().to_string(),
                            latest_completed_bar_ts: None,
                            blockers: vec![blocker.detail()],
                        });
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        statuses.push(RequiredMarketDataStatus {
                            symbol: resolved.symbol.clone(),
                            timeframe: resolved.timeframe.clone(),
                            provider_id: Some(resolved.provider_id.clone()),
                            provider_symbol: Some(resolved.provider_symbol.clone()),
                            freshness_state: "unavailable".to_string(),
                            latest_completed_bar_ts: None,
                            blockers: vec![error],
                        });
                        continue;
                    }
                }

                // §2-4: strict readiness authority, never the legacy
                // fixed-5-bar/wall-clock-age evaluator.
                let readiness = match evaluate_strict_readiness_for_resolved(
                    db_pool,
                    resolved,
                    &multi_symbol_config,
                    calendar_provider,
                    &providers,
                    &instruments,
                    now_utc,
                )
                .await
                {
                    Ok(readiness) => readiness,
                    Err(detail) => {
                        statuses.push(RequiredMarketDataStatus {
                            symbol: resolved.symbol.clone(),
                            timeframe: resolved.timeframe.clone(),
                            provider_id: Some(resolved.provider_id.clone()),
                            provider_symbol: Some(resolved.provider_symbol.clone()),
                            freshness_state: "strategy_assignment_unresolved".to_string(),
                            latest_completed_bar_ts: None,
                            blockers: vec![detail],
                        });
                        continue;
                    }
                };

                // §21: only the bounded, typed refreshable-reason set may
                // trigger a bounded provider attempt — never a
                // registry/binding/calendar/provenance blocker, and never
                // when the bar stage itself could not run (db_unavailable/
                // query_failed).
                let should_attempt_refresh = !dry_run
                    && !past_close_grace
                    && !readiness.is_ready()
                    && !matches!(readiness.readiness_state, "db_unavailable" | "query_failed")
                    && readiness.blockers.iter().any(|b| is_refreshable_reason(b));

                if should_attempt_refresh {
                    if let Some(provider_config) = providers
                        .iter()
                        .find(|p| p.provider_id.eq_ignore_ascii_case(&resolved.provider_id))
                    {
                        // §17/§22/§23: interior gaps and missing/insufficient
                        // history need the bounded historical bootstrap; an
                        // otherwise-ready requirement missing only today's
                        // expected latest bar needs exactly one constrained
                        // latest-bar poll (§24 — never `expected_bar_
                        // constraint=None` for this repair path).
                        let outcome = if needs_historical_bootstrap(&readiness.blockers) {
                            let required = readiness.required_history_bars.unwrap_or(0);
                            attempt_bounded_historical_bootstrap(
                                db_pool,
                                provider_config,
                                injected_provider_client,
                                resolved,
                                required,
                                now_utc,
                            )
                            .await
                        } else {
                            let constraint = readiness
                                .expected_latest_bar_ts
                                .map(|exact_end_ts| ExpectedLatestBarConstraint { exact_end_ts });
                            attempt_bounded_refresh(
                                db_pool,
                                &instruments,
                                provider_config,
                                injected_provider_client,
                                resolved,
                                constraint,
                                now_utc,
                            )
                            .await
                        };
                        if outcome.is_actual_call() {
                            provider_api_calls_made += 1;
                        }

                        // Re-run the strict readiness authority after the
                        // bounded attempt (§18) regardless of its outcome —
                        // it is the single source of truth for whether the
                        // requirement is now ready, not the refresh call's
                        // own return value.
                        let reevaluated = evaluate_strict_readiness_for_resolved(
                            db_pool,
                            resolved,
                            &multi_symbol_config,
                            calendar_provider,
                            &providers,
                            &instruments,
                            now_utc,
                        )
                        .await;

                        match reevaluated {
                            Ok(readiness2) => {
                                let mut blockers: Vec<String> =
                                    readiness2.blockers.iter().map(|s| s.to_string()).collect();
                                if let RefreshAttemptOutcome::CalledError(detail) = &outcome {
                                    if !readiness2.is_ready() {
                                        blockers.push(detail.clone());
                                    }
                                }
                                statuses.push(RequiredMarketDataStatus {
                                    symbol: resolved.symbol.clone(),
                                    timeframe: resolved.timeframe.clone(),
                                    provider_id: Some(resolved.provider_id.clone()),
                                    provider_symbol: Some(resolved.provider_symbol.clone()),
                                    freshness_state: freshness_state_from_readiness(&readiness2),
                                    latest_completed_bar_ts: readiness2
                                        .actual_latest_bar_ts
                                        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
                                        .map(|dt| dt.to_rfc3339()),
                                    blockers,
                                });
                            }
                            Err(detail) => {
                                statuses.push(RequiredMarketDataStatus {
                                    symbol: resolved.symbol.clone(),
                                    timeframe: resolved.timeframe.clone(),
                                    provider_id: Some(resolved.provider_id.clone()),
                                    provider_symbol: Some(resolved.provider_symbol.clone()),
                                    freshness_state: "strategy_assignment_unresolved".to_string(),
                                    latest_completed_bar_ts: None,
                                    blockers: vec![detail],
                                });
                            }
                        }
                        continue;
                    }
                }

                statuses.push(RequiredMarketDataStatus {
                    symbol: resolved.symbol.clone(),
                    timeframe: resolved.timeframe.clone(),
                    provider_id: Some(resolved.provider_id.clone()),
                    provider_symbol: Some(resolved.provider_symbol.clone()),
                    freshness_state: freshness_state_from_readiness(&readiness),
                    latest_completed_bar_ts: readiness
                        .actual_latest_bar_ts
                        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
                        .map(|dt| dt.to_rfc3339()),
                    blockers: readiness.blockers.iter().map(|s| s.to_string()).collect(),
                });
            }
        }
    }

    // §14/§25/§45: overall readiness requires every required requirement
    // ready — one blocked/stale/missing symbol blocks the whole universe,
    // regardless of how many other symbols succeeded (no partial green).
    let any_blocking = statuses
        .iter()
        .any(|s| s.freshness_state != "ok" || !s.blockers.is_empty());
    let overall_state = if any_blocking { "blocked" } else { "ready" };

    let report = RequiredUniverseStatusReport {
        truth_state: "active".to_string(),
        market_date: plan.market_date.clone(),
        overall_state: overall_state.to_string(),
        symbol_source: plan.symbol_source.clone(),
        requirements: statuses,
        groups: plan.groups.clone(),
        is_trading_day: schedule.is_trading_day,
        session_open_utc: Some(schedule.session_open_utc.to_rfc3339()),
        session_close_utc: Some(schedule.session_close_utc.to_rfc3339()),
        last_poll_utc: Some(now_utc.to_rfc3339()),
        next_poll_utc: next_poll_time_for_groups(&plan.groups, &schedule, now_utc),
        provider_api_calls_made_this_cycle: provider_api_calls_made,
        operator_retry_required: false,
    };

    RequiredUniverseCycleResult {
        report,
        provider_api_calls_made,
    }
}

fn not_applicable_report(
    resolution: &RequiredSymbolsResolution,
    schedule: &MarketSessionSchedule,
) -> RequiredUniverseStatusReport {
    RequiredUniverseStatusReport {
        truth_state: "active".to_string(),
        market_date: format!(
            "{:04}-{:02}-{:02}",
            schedule.market_date.0, schedule.market_date.1, schedule.market_date.2
        ),
        overall_state: "not_applicable".to_string(),
        symbol_source: resolution.source.to_string(),
        requirements: Vec::new(),
        groups: Vec::new(),
        is_trading_day: schedule.is_trading_day,
        session_open_utc: Some(schedule.session_open_utc.to_rfc3339()),
        session_close_utc: Some(schedule.session_close_utc.to_rfc3339()),
        last_poll_utc: None,
        next_poll_utc: None,
        provider_api_calls_made_this_cycle: 0,
        operator_retry_required: false,
    }
}

/// REPAIR-01 §16/§20: `reason_code` must name the registry that actually
/// failed to load — `"instrument_registry_invalid"` or
/// `"provider_registry_invalid"` — never a hardcoded
/// `"provider_registry_invalid"` regardless of which registry the caller
/// was loading. Both call sites in [`run_required_universe_cycle`] pass the
/// reason matching the specific loader (`load_validated_instrument_registry`
/// / `load_validated_provider_registry`) that returned `Err`.
fn registry_unavailable_report(
    resolution: &RequiredSymbolsResolution,
    schedule: &MarketSessionSchedule,
    reason_code: &'static str,
    error: String,
) -> RequiredUniverseStatusReport {
    let normalized = normalize_required_symbols(&resolution.required);
    let requirements = normalized
        .into_iter()
        .map(|r| RequiredMarketDataStatus {
            symbol: r.symbol,
            timeframe: r.timeframe,
            provider_id: None,
            provider_symbol: None,
            freshness_state: reason_code.to_string(),
            latest_completed_bar_ts: None,
            blockers: vec![error.clone()],
        })
        .collect();
    RequiredUniverseStatusReport {
        truth_state: "active".to_string(),
        market_date: format!(
            "{:04}-{:02}-{:02}",
            schedule.market_date.0, schedule.market_date.1, schedule.market_date.2
        ),
        overall_state: "blocked".to_string(),
        symbol_source: resolution.source.to_string(),
        requirements,
        groups: Vec::new(),
        is_trading_day: schedule.is_trading_day,
        session_open_utc: Some(schedule.session_open_utc.to_rfc3339()),
        session_close_utc: Some(schedule.session_close_utc.to_rfc3339()),
        last_poll_utc: None,
        next_poll_utc: None,
        provider_api_calls_made_this_cycle: 0,
        operator_retry_required: false,
    }
}

/// REPAIR-01 §6/§7/§8: the next session-anchored publication-grace-aware
/// poll boundary for one timeframe. `None` means "nothing left to poll for
/// `market_date` on this timeframe" — either the schedule's own date is not
/// a trading day, or every grid boundary through the final-capture horizon
/// has already passed.
///
/// Deliberately never `mqk_md::next_poll_time_ts` (an epoch-boundary
/// cadence with no session/grace awareness): that produced a wake every
/// `timeframe` epoch-boundary regardless of whether the market had even
/// opened yet (§7's "no 5-minute heartbeat before open" defect). Every
/// candidate here is `session_open_utc`-anchored grid-slot-close +
/// `effective_grace_seconds` — the same session-anchored expectation
/// `daily_data_readiness::expected_intraday_end_ts_window`/
/// `intraday_grid_starts` compute, reused via the same grid-construction
/// helper (`daily_data_readiness::intraday_grid_starts`) rather than a
/// second, hand-rolled grid walk.
fn next_session_anchored_boundary(
    timeframe: mqk_md::Timeframe,
    schedule: &MarketSessionSchedule,
    now_ts: i64,
    effective_grace_seconds: i64,
) -> Option<i64> {
    if !schedule.is_trading_day {
        return None;
    }
    let close_ts = schedule.session_close_utc.timestamp();
    let close_horizon_ts = close_ts + SESSION_CLOSE_POLL_BUFFER_SECS;
    if now_ts >= close_horizon_ts {
        return None;
    }
    if timeframe == mqk_md::Timeframe::D1 {
        // One expected bar per session, labeled at session close (§8's
        // "final capture horizon" retained as the outer clamp).
        let boundary = close_ts + effective_grace_seconds;
        return Some(boundary.max(now_ts + 1).min(close_horizon_ts));
    }
    let interval = timeframe.duration_secs();
    for slot in daily_data_readiness::intraday_grid_starts(
        schedule.session_open_utc,
        schedule.session_close_utc,
        interval,
    ) {
        let boundary = slot + interval + effective_grace_seconds;
        if boundary > now_ts {
            return Some(boundary.min(close_horizon_ts));
        }
    }
    // Every intraday grid boundary already passed, but the final-capture
    // horizon has not — keep one last scheduled poll there (§8).
    Some(close_horizon_ts)
}

/// Compute the next scheduler poll instant for a plan's groups: the
/// earliest [`next_session_anchored_boundary`] across every distinct
/// timeframe in `groups`. Returns `None` when there are no groups (nothing
/// to poll) or once every timeframe's own session-anchored horizon has
/// passed for the day (§22/§23, session-close proof: the scheduler stops
/// scheduling further polls for `market_date`).
fn next_poll_time_for_groups(
    groups: &[ProviderTimeframeGroup],
    schedule: &MarketSessionSchedule,
    now_utc: DateTime<Utc>,
) -> Option<String> {
    if groups.is_empty() {
        return None;
    }
    let now_ts = now_utc.timestamp();
    let configured_grace = daily_data_readiness::configured_grace_seconds_from_env();
    let mut earliest: Option<i64> = None;
    for group in groups {
        let Ok(timeframe) = mqk_md::Timeframe::parse(&group.timeframe) else {
            continue;
        };
        let grace = daily_data_readiness::effective_grace_seconds(
            configured_grace,
            timeframe.duration_secs(),
        );
        if let Some(candidate) = next_session_anchored_boundary(timeframe, schedule, now_ts, grace)
        {
            earliest = Some(match earliest {
                Some(current) => current.min(candidate),
                None => candidate,
            });
        }
    }
    earliest
        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
        .map(|dt| dt.to_rfc3339())
}

// ---------------------------------------------------------------------------
// Scheduler runtime state (§14, §27, §28)
// ---------------------------------------------------------------------------

/// Process-local required-universe scheduler runtime state. Disabled at
/// boot, becomes active only after the operator (or the Paper startup
/// wrapper) calls the start route. Not durable across restart — the
/// authoritative source of truth after a restart is `md_bars` in Postgres
/// plus a freshly re-derived plan, never a remembered in-memory flag (§27/
/// §28 restart proof).
pub struct RequiredUniverseSchedulerRuntimeState {
    pub running: bool,
    pub dry_run: bool,
    pub last_report: Option<RequiredUniverseStatusReport>,
    pub last_cycle_utc: Option<i64>,
    pub next_cycle_utc: Option<i64>,
    pub started_at_ts: Option<i64>,
    pub stopped_at_ts: Option<i64>,
    pub cycle_count: u64,
    pub provider_api_calls_made: u64,
    pub last_error: Option<String>,
    pub stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub task: Option<JoinHandle<()>>,
    /// REPAIR-02 §8/§9: monotonically-changing ownership token. Every
    /// successful `start` obtains a strictly newer generation than any
    /// generation previously issued for this scheduler slot. A cycle/task
    /// belonging to an old generation must never mutate state or install a
    /// task once a newer generation is current — see
    /// [`run_and_record_cycle`], [`start_required_universe_scheduler`], and
    /// [`required_universe_scheduler_loop`], which each re-check
    /// `generation` under the lock before writing/spawning. Never reset to
    /// `0` on a state reset at `start` — see `start_required_universe_scheduler`,
    /// which increments the previous value rather than relying on
    /// `..Default::default()` for this field.
    pub generation: u64,
}

impl Default for RequiredUniverseSchedulerRuntimeState {
    fn default() -> Self {
        Self {
            running: false,
            dry_run: true,
            last_report: None,
            last_cycle_utc: None,
            next_cycle_utc: None,
            started_at_ts: None,
            stopped_at_ts: None,
            cycle_count: 0,
            provider_api_calls_made: 0,
            last_error: None,
            stop_tx: None,
            task: None,
            generation: 0,
        }
    }
}

/// Run one required-universe cycle against `st` and record the result on
/// the scheduler runtime state — but ONLY if `generation` still matches the
/// scheduler's current ownership token at the moment the result is ready to
/// write (REPAIR-02 §11). The provider/DB work behind `run_required_universe_
/// cycle` always runs and cannot be undone once dispatched, but a cycle
/// whose generation has been superseded by a newer `start` while it was
/// in-flight (the stop/restart ABA race, §8) must not overwrite
/// `last_report`/`next_cycle_utc`, increment `cycle_count`/`provider_api_
/// calls_made`, or otherwise touch state that now belongs to the newer
/// generation. The caller always receives the actual computed report
/// regardless of whether it was recorded, so a synchronous HTTP caller (the
/// immediate cycle inside `start_required_universe_scheduler`) still gets a
/// truthful answer even when superseded.
pub async fn run_and_record_cycle(
    st: &Arc<AppState>,
    generation: u64,
    now_utc: DateTime<Utc>,
    dry_run: bool,
) -> RequiredUniverseStatusReport {
    let calendar_provider = active_calendar_provider_from_env();
    let result = run_required_universe_cycle(
        st.db.as_ref(),
        &st.instrument_registry_path,
        &st.provider_registry_path,
        st.latest_bar_provider_client.as_ref(),
        calendar_provider.as_ref(),
        now_utc,
        dry_run,
    )
    .await;

    let mut scheduler = st.required_universe_scheduler.lock().await;
    if scheduler.generation == generation {
        scheduler.last_cycle_utc = Some(now_utc.timestamp());
        scheduler.cycle_count += 1;
        scheduler.provider_api_calls_made += result.provider_api_calls_made;
        scheduler.next_cycle_utc = result
            .report
            .next_poll_utc
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc).timestamp());
        scheduler.last_report = Some(result.report.clone());
    }
    result.report
}

/// Start the required-universe scheduler: run one cycle immediately and
/// record it, then (unless the immediate cycle already determined there is
/// nothing left to poll for `market_date` — no groups, or past the session
/// close + grace horizon) spawn a background loop that re-runs a cycle at
/// each computed next-poll instant until stopped or the session-close
/// horizon is reached (§14/§21-23).
///
/// Returns `Err` (no state mutation) if the scheduler is already running —
/// mirrors the existing feed scheduler's `already_running` conflict
/// contract.
///
/// REPAIR-01 Defect B: the stopped -> running transition is now the check
/// and the set inside one single mutex acquisition, so two concurrent
/// callers can never both observe `running == false` and both proceed to
/// claim ownership — exactly one of any two concurrent calls gets `Ok`, the
/// other gets the `already_running` `Err` with no state mutation, and the
/// immediate controller cycle + background task are each started exactly
/// once (proven by `concurrent_start_admits_exactly_one_starter` in the
/// scenario test file).
pub async fn start_required_universe_scheduler(
    st: &Arc<AppState>,
    dry_run: bool,
    now_utc: DateTime<Utc>,
) -> Result<RequiredUniverseStatusReport, String> {
    let generation = {
        let mut scheduler = st.required_universe_scheduler.lock().await;
        if scheduler.running {
            return Err("required-universe scheduler is already running".to_string());
        }
        // REPAIR-02 §9/§10: claim a strictly newer generation than any
        // previously issued, preserved explicitly across the state reset
        // below rather than left to `..Default::default()` (which would
        // reset it to `0` and reopen the ABA race this token exists to
        // close). Wrapping is intentional and harmless: a wrap back to `0`
        // is still guaranteed distinct from whatever generation an
        // in-flight cycle from `u64::MAX` cycles ago could hold in
        // practice.
        let new_generation = scheduler.generation.wrapping_add(1);
        // Atomic with the check above: no other caller can observe
        // `running == false` between the check and this assignment, because
        // both happen under the same held lock and this function never
        // `.await`s while holding it.
        *scheduler = RequiredUniverseSchedulerRuntimeState {
            running: true,
            dry_run,
            started_at_ts: Some(now_utc.timestamp()),
            generation: new_generation,
            ..RequiredUniverseSchedulerRuntimeState::default()
        };
        new_generation
    };

    let report = run_and_record_cycle(st, generation, now_utc, dry_run).await;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // §11/§12: if the immediate cycle already determined there is nothing
    // left to poll for `market_date` (no groups, non-trading day, or
    // already past the session-close horizon), settle the scheduler
    // truthfully as stopped right here — never leave `running=true` with no
    // task and no future cycle. Both branches require `generation` to still
    // be current: a stopped-then-restarted scheduler (a newer generation
    // now owns the slot) must never be settled-stopped or spawned into by
    // this superseded caller.
    let should_spawn = {
        let mut scheduler = st.required_universe_scheduler.lock().await;
        if !scheduler.running || scheduler.generation != generation {
            // Stopped concurrently while the immediate cycle ran, or a
            // newer generation has already taken ownership of the slot.
            false
        } else if report.next_poll_utc.is_none() {
            scheduler.running = false;
            scheduler.stopped_at_ts = Some(now_utc.timestamp());
            scheduler.stop_tx = None;
            scheduler.next_cycle_utc = None;
            false
        } else {
            scheduler.stop_tx = Some(stop_tx);
            true
        }
    };

    if should_spawn {
        let task = tokio::spawn(required_universe_scheduler_loop(
            st.clone(),
            stop_rx,
            dry_run,
            generation,
        ));
        let mut scheduler = st.required_universe_scheduler.lock().await;
        if scheduler.running && scheduler.generation == generation {
            scheduler.task = Some(task);
        } else {
            // Ownership flipped to a newer generation between spawn and
            // this re-check (§12) — never install this generation's task
            // as if it were current.
            task.abort();
        }
    }

    Ok(report)
}

/// Stop the required-universe scheduler. Idempotent: stopping an
/// already-stopped scheduler is a no-op that returns `false`.
pub async fn stop_required_universe_scheduler(st: &Arc<AppState>, now_utc: DateTime<Utc>) -> bool {
    let mut scheduler = st.required_universe_scheduler.lock().await;
    if !scheduler.running {
        return false;
    }
    scheduler.running = false;
    scheduler.stopped_at_ts = Some(now_utc.timestamp());
    if let Some(tx) = scheduler.stop_tx.take() {
        let _ = tx.send(true);
    }
    if let Some(task) = scheduler.task.take() {
        task.abort();
    }
    scheduler.next_cycle_utc = None;
    true
}

/// REPAIR-02 §13: `generation` is the ownership token this loop was spawned
/// under. Every checkpoint — before the wait, after the wait, and after the
/// awaited cycle — re-verifies `scheduler.running && scheduler.generation ==
/// generation` before doing anything that would act as the current
/// scheduler. An old loop whose generation has been superseded (a stop
/// followed by a new start) must return immediately rather than continue
/// polling or mutate state belonging to the newer generation.
async fn required_universe_scheduler_loop(
    st: Arc<AppState>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    dry_run: bool,
    generation: u64,
) {
    loop {
        let next_ts = {
            let scheduler = st.required_universe_scheduler.lock().await;
            if !scheduler.running || scheduler.generation != generation {
                return;
            }
            match scheduler.next_cycle_utc {
                Some(ts) => ts,
                // Nothing left to poll for today (session-close horizon
                // already reached, or no provider groups) — the immediate
                // cycle already recorded that; the loop's job is done.
                None => {
                    return;
                }
            }
        };

        let now_ts = Utc::now().timestamp();
        let wait_secs = next_ts.saturating_sub(now_ts).max(0) as u64;
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(wait_secs)) => {}
        }
        if *stop_rx.borrow() {
            return;
        }
        {
            let scheduler = st.required_universe_scheduler.lock().await;
            if !scheduler.running || scheduler.generation != generation {
                return;
            }
        }

        let now_utc = Utc::now();
        let report = run_and_record_cycle(&st, generation, now_utc, dry_run).await;
        if report.next_poll_utc.is_none() {
            // Session-close horizon reached: mark the scheduler stopped for
            // this market_date rather than spinning forever with no work —
            // but only if this generation still owns the slot.
            let mut scheduler = st.required_universe_scheduler.lock().await;
            if scheduler.running && scheduler.generation == generation {
                scheduler.running = false;
                scheduler.stopped_at_ts = Some(now_utc.timestamp());
                scheduler.stop_tx = None;
            }
            return;
        }
    }
}
