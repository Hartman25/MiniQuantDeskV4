//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER.
//!
//! Foundation only: this module implements one injected, fully-deterministic
//! driver-tick seam (`tick_autonomous_completed_bar_driver`) that decides
//! whether to poll a market-data provider for a genuinely new completed bar
//! for the currently supported single effective runtime binding, and — when
//! a new bar is observed and every strict readiness/binding precondition
//! holds — dispatches native strategy evaluation for it at most once,
//! durably.
//!
//! Nothing in this module is started from `main.rs`, wired into
//! `session_controller.rs`, or otherwise made to run automatically in this
//! patch. The task-runner scaffold at the bottom of this file
//! (`AutonomousCompletedBarDriverTask`) is provided for Phase D to adopt;
//! Phase C does not spawn it.
//!
//! No historical sync, no backfill, no ingest-job creation: this driver
//! only ever calls the latest-closed-bar poll seam
//! (`super::market_data_latest_bar`). No provider call is made unless
//! [`AutonomousProviderCallAuthorization::Authorized`] holds.
//!
//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01
//! (Phase C production-path closure repair): the original Phase C
//! implementation checked strict readiness once, before polling, and reused
//! that same pre-poll snapshot to authorize dispatch after ingest — so a bar
//! that only became ready *after* being ingested was never re-checked, and a
//! provider call was refused outright whenever the missing expected bar was
//! itself the only blocker. This repair splits evaluation into two stages
//! (see [`AutonomousAssignmentReadinessEvaluator`]): a pre-poll evaluation
//! that only gates *whether polling is eligible* (a missing expected bar is
//! poll-remediable, not a hard block), and a mandatory post-poll
//! re-evaluation from current DB truth that alone authorizes dispatch.
//!
//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01 (Phase C
//! observed-bar recovery and provider-dependency closure): the prior repair
//! still left two gaps. First, `operation.last_completed_bar_ts ==
//! expected_ts` was treated as a terminal "nothing to do" signal
//! (`PollNotDue`), even though that field proves only that the bar was
//! *observed* at some point — never that readiness later passed, that a
//! dispatch claim exists, or that the claim completed. A crash between
//! observing a bar and claiming its dispatch left the operation permanently
//! stuck: every subsequent tick saw `last_completed_bar_ts == expected_ts`
//! and refused to do anything further. Second, the driver required a
//! provider-call authorization and an already-constructed provider object
//! before it would even look at canonical data already sitting in
//! `md_bars` — so a disabled/invalid authorization blocked recovery of a
//! bar the driver did not need to fetch at all. This repair removes the
//! short-circuit (every tick with a known expected timestamp reconciles
//! canonical bar presence, observation evidence, post-observation
//! readiness, and dispatch-claim status via
//! [`reconcile_observed_expected_bar`]), replaces the mandatory
//! already-built provider with a lazy [`AutonomousLatestBarProviderResolver`]
//! seam invoked only once a provider call is genuinely about to happen, and
//! adds [`AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent`]
//! / [`AutonomousCompletedBarDriverOutcome::ObservedBarSequenceInconsistent`]
//! as typed fail-closed truth for durable-evidence corruption that must
//! never be silently re-polled away. Phase D integration is out of scope.
//!
//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01 (Phase C
//! preparation-versus-dispatch repair): `1d4b2674` closed observed-bar
//! recovery, but the driver still accepted every nonterminal operation state
//! (the top-level gate here only excludes terminal states and
//! `manual_intervention_required`) all the way through to dispatch-claim
//! creation — including every state that exists *before* runtime start, when
//! no native-strategy bootstrap can possibly be active. This repair adds an
//! explicit, caller-chosen [`AutonomousCompletedBarDriverMode`]:
//! `PrepareDataOnly` performs the identical observation/poll/readiness
//! pipeline but never creates a dispatch claim, deposits a pending strategy
//! bar, or invokes native strategy code; `RunningDispatch` proves runtime-
//! dispatch eligibility ([`AutonomousStrategyDispatchRuntimeTruth`], via
//! `AppState::autonomous_strategy_dispatch_runtime_truth`) before ever
//! calling `claim_autonomous_daily_bar_dispatch`. No Phase D controller
//! integration, no task auto-start, and no real provider/broker/network call
//! are introduced by this repair.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::market_data_latest_bar::{
    poll_and_ingest_latest_closed_bar, resolve_latest_bar_poll_target, ExpectedLatestBarConstraint,
    LatestBarPollOutcome, LatestBarPollSeamInput, LatestBarRegistryAdmissionRejection,
    ResolvedLatestBarPollTarget,
};
use super::multi_symbol_config::MultiSymbolRuntimeConfig;
use super::{AppState, StrategyBarInput};
use crate::daily_data_readiness::{
    self, AssignmentReadiness, DailyDataReadinessContext, REASON_EXPECTED_LATEST_BAR_MISSING,
    REASON_INSUFFICIENT_HISTORY,
};
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;

// ---------------------------------------------------------------------------
// C.8 — Production registry/provider resolution wrapper
// ---------------------------------------------------------------------------
//
// The tick function itself takes an already-loaded instrument registry and
// a lazily-resolved provider seam (so tests can inject fakes without
// touching the filesystem or a provider factory). `load_driver_instruments`
// and `load_driver_instruments_and_provider` are the thin production seams
// that load and validate registries from disk before the tick function ever
// runs — reusing the exact same `mqk_md::instrument_registry` /
// `mqk_md::provider_registry` functions the manual poll-once route and
// historical provider sync already use (C.8), never ad hoc parsing. A
// rejection here happens strictly before any provider call.

/// Why the driver's registry/provider setup could not be resolved. Every
/// variant is reached before any provider network call.
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomousDriverSetupRejection {
    InstrumentRegistryUnavailable(String),
    InstrumentRegistryInvalid(String),
    ProviderRegistryUnavailable(String),
    ProviderUnknownOrDisabled(String),
    ProviderConstructionFailed(String),
}

/// Load and validate the instrument registry only. Zero provider-registry
/// reads and zero credential reads — this is the half of
/// [`load_driver_instruments_and_provider`] that
/// [`AutonomousCompletedBarDriverInput::instruments`] production callers
/// need unconditionally (canonical provider/local-symbol mapping is
/// required for both the exact local-bar provenance check and provider
/// polling), independent of whether a provider call ever happens this tick.
pub fn load_driver_instruments(
    instrument_registry_path: &str,
) -> Result<Vec<TrackedInstrument>, AutonomousDriverSetupRejection> {
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        instrument_registry_path,
    ))
    .map_err(|e| AutonomousDriverSetupRejection::InstrumentRegistryUnavailable(e.to_string()))?;
    mqk_md::instrument_registry::validate_registry(&instruments)
        .map_err(|e| AutonomousDriverSetupRejection::InstrumentRegistryInvalid(e.to_string()))?;
    Ok(instruments)
}

/// Load and validate the instrument registry, and resolve/build the market
/// data provider, from paths and an already-resolved `provider_id`. Zero
/// provider calls are made by this function itself — provider construction
/// only prepares a client, it never calls `fetch_latest_closed_bar`.
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: retained
/// unchanged for the two admission tests that call it directly, and for any
/// caller that genuinely wants eager (non-lazy) construction. Production
/// driver wiring should prefer [`load_driver_instruments`] together with
/// [`ProductionAutonomousLatestBarProviderResolver`], which defers the
/// provider-registry read and credential lookup this function performs
/// eagerly until a provider call is actually about to happen.
pub fn load_driver_instruments_and_provider(
    instrument_registry_path: &str,
    provider_registry_path: &str,
    provider_id: &str,
) -> Result<(Vec<TrackedInstrument>, mqk_md::MarketDataProviderBox), AutonomousDriverSetupRejection>
{
    let instruments = load_driver_instruments(instrument_registry_path)?;
    let resolver = ProductionAutonomousLatestBarProviderResolver::new(provider_registry_path);
    let provider = resolver.resolve(provider_id)?;
    Ok((instruments, provider))
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01
// Lazy provider resolution seam
// ---------------------------------------------------------------------------
//
// `AutonomousCompletedBarDriverInput` no longer carries an already-built
// `&dyn mqk_md::MarketDataProvider` on every tick — that forced
// provider-registry construction and credential loading even when no
// provider call was required this tick (e.g. the exact expected bar is
// already canonical in `md_bars`, or authorization is disabled/invalid).
// This seam resolves a provider only once every non-provider precondition
// has passed and a poll is genuinely about to happen.

/// Resolve a market-data provider by id, lazily. Implementations must not
/// perform any provider network call themselves — only construction
/// (registry lookup, credential loading, client setup).
pub trait AutonomousLatestBarProviderResolver: Send + Sync {
    fn resolve(
        &self,
        provider_id: &str,
    ) -> Result<mqk_md::MarketDataProviderBox, AutonomousDriverSetupRejection>;
}

/// Production resolver: reads the provider registry from disk and builds
/// the provider (including any environment-variable credential lookup)
/// only inside `resolve()` — never while merely loading instruments, and
/// never on the local exact-bar path (`resolve()` is not called at all in
/// that case; see [`reconcile_observed_expected_bar`]).
pub struct ProductionAutonomousLatestBarProviderResolver {
    provider_registry_path: String,
}

impl ProductionAutonomousLatestBarProviderResolver {
    pub fn new(provider_registry_path: impl Into<String>) -> Self {
        Self {
            provider_registry_path: provider_registry_path.into(),
        }
    }
}

impl AutonomousLatestBarProviderResolver for ProductionAutonomousLatestBarProviderResolver {
    fn resolve(
        &self,
        provider_id: &str,
    ) -> Result<mqk_md::MarketDataProviderBox, AutonomousDriverSetupRejection> {
        let providers = mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
            &self.provider_registry_path,
        ))
        .map_err(|e| AutonomousDriverSetupRejection::ProviderRegistryUnavailable(e.to_string()))?;
        let config = mqk_md::provider_registry::find_provider(&providers, provider_id)
            .filter(|c| c.enabled)
            .ok_or_else(|| {
                AutonomousDriverSetupRejection::ProviderUnknownOrDisabled(provider_id.to_string())
            })?;
        mqk_md::build_market_data_provider_from_config(config, |name| std::env::var(name).ok())
            .map_err(|e| AutonomousDriverSetupRejection::ProviderConstructionFailed(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// C.2/C.3 — Two-part autonomous provider-call authorization
// ---------------------------------------------------------------------------

pub const AUTONOMOUS_DATA_REFRESH_ENABLED_ENV: &str = "MQK_AUTONOMOUS_DATA_REFRESH_ENABLED";
pub const ALLOW_PROVIDER_API_CALLS_ENV: &str = "MQK_ALLOW_PROVIDER_API_CALLS";

/// Frozen logical gate: `autonomous_data_refresh_enabled == true AND
/// allow_provider_api_calls == true`. PAPER mode alone is never sufficient
/// authorization to make a provider call.
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: this
/// authorization gates provider *calls* only — it must never prohibit use of
/// trusted canonical data already stored in `md_bars`. See
/// [`reconcile_observed_expected_bar`] for the exact boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousProviderCallAuthorization {
    Authorized,
    Disabled,
    Invalid {
        reason_code: &'static str,
        detail: String,
    },
}

fn parse_required_bool_flag(
    name: &'static str,
    raw: Option<&str>,
) -> Result<Option<bool>, (&'static str, String)> {
    match raw.map(str::trim) {
        None => Ok(None),
        Some("") => Ok(None),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err((
                name,
                format!("{name} must be 'true' or 'false' (got '{v}')"),
            )),
        },
    }
}

/// Pure classification of the two-part authorization from already-read raw
/// env-var values. Never reads env itself — production callers use
/// [`resolve_autonomous_provider_call_authorization_from_env`]; tests inject
/// raw values directly.
pub fn resolve_autonomous_provider_call_authorization(
    refresh_enabled_raw: Option<&str>,
    allow_provider_calls_raw: Option<&str>,
) -> AutonomousProviderCallAuthorization {
    let refresh =
        parse_required_bool_flag(AUTONOMOUS_DATA_REFRESH_ENABLED_ENV, refresh_enabled_raw);
    let allow = parse_required_bool_flag(ALLOW_PROVIDER_API_CALLS_ENV, allow_provider_calls_raw);

    match (refresh, allow) {
        (Err((reason_code, detail)), _) | (_, Err((reason_code, detail))) => {
            AutonomousProviderCallAuthorization::Invalid {
                reason_code,
                detail,
            }
        }
        (Ok(Some(true)), Ok(Some(true))) => AutonomousProviderCallAuthorization::Authorized,
        _ => AutonomousProviderCallAuthorization::Disabled,
    }
}

/// Production entry point: reads both env vars once and delegates to the
/// pure classifier.
pub fn resolve_autonomous_provider_call_authorization_from_env(
) -> AutonomousProviderCallAuthorization {
    let refresh = std::env::var(AUTONOMOUS_DATA_REFRESH_ENABLED_ENV).ok();
    let allow = std::env::var(ALLOW_PROVIDER_API_CALLS_ENV).ok();
    resolve_autonomous_provider_call_authorization(refresh.as_deref(), allow.as_deref())
}

// ---------------------------------------------------------------------------
// C.5 — Single effective-binding applicability
// ---------------------------------------------------------------------------

/// Why the currently-configured assignment/binding could not be resolved to
/// exactly one autonomous-eligible `(symbol, strategy_id, timeframe)` for
/// this operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousBindingRejection {
    AssignmentIdentityMismatch,
    RuntimeBindingIdentityMismatch,
    BlankTargetSymbol,
    BlankStrategyId,
    MissingTimeframeBinding,
    UnsupportedTimeframe,
    MultiSymbolAssignmentNotExactlyBound,
}

/// The one exactly-bound `(symbol, strategy_id, timeframe)` this driver may
/// operate on for the current tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSingleBinding {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe: mqk_md::Timeframe,
}

/// Resolve exactly one autonomous-eligible binding, or a stable typed
/// rejection. Requires: the operation row's recorded
/// `assignment_identity`/`runtime_binding_identity` to agree with the
/// freshly-computed identities (proves no operator config change slipped in
/// since the operation was created/recovered); a non-blank strategy id,
/// target symbol, and timeframe binding; and exactly one configured
/// assignment that matches the resolved binding's symbol/strategy exactly —
/// per the bundle's Phase C restriction, a same-strategy/different-symbol or
/// any other multi-symbol assignment is unsupported, never silently
/// narrowed to "the first assignment".
pub fn resolve_single_effective_binding(
    operation: &mqk_db::AutonomousDailyOperationRecord,
    assignment_config: &MultiSymbolRuntimeConfig,
    assignment_identity: &str,
    runtime_binding: &EffectiveRuntimeBinding,
    runtime_binding_identity: &str,
) -> Result<ResolvedSingleBinding, AutonomousBindingRejection> {
    if operation.assignment_identity != assignment_identity {
        return Err(AutonomousBindingRejection::AssignmentIdentityMismatch);
    }
    if operation.runtime_binding_identity != runtime_binding_identity {
        return Err(AutonomousBindingRejection::RuntimeBindingIdentityMismatch);
    }

    let target_symbol = runtime_binding
        .effective_runtime_target_symbol
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(AutonomousBindingRejection::BlankTargetSymbol)?;
    let strategy_id = runtime_binding
        .effective_runtime_strategy_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(AutonomousBindingRejection::BlankStrategyId)?;
    runtime_binding
        .effective_runtime_timeframe_secs
        .ok_or(AutonomousBindingRejection::MissingTimeframeBinding)?;

    if assignment_config.symbols.len() != 1 {
        return Err(AutonomousBindingRejection::MultiSymbolAssignmentNotExactlyBound);
    }
    let only = &assignment_config.symbols[0];
    if !only.symbol.trim().eq_ignore_ascii_case(target_symbol)
        || only.strategy_id.trim() != strategy_id
    {
        return Err(AutonomousBindingRejection::MultiSymbolAssignmentNotExactlyBound);
    }
    let timeframe = mqk_md::Timeframe::parse(only.timeframe.trim())
        .map_err(|_| AutonomousBindingRejection::UnsupportedTimeframe)?;

    Ok(ResolvedSingleBinding {
        symbol: target_symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe,
    })
}

// ---------------------------------------------------------------------------
// REPAIR 1 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01)
// — injected strict-readiness evaluator seam, so the driver never has to be
// handed a stale, pre-computed `AssignmentReadiness` snapshot for both the
// pre-poll and post-poll checks. The production adapter reuses the exact
// same Bundle 2 evaluator (`daily_data_readiness::evaluate_readiness_with_binding`)
// every route and the runtime start gate already share — never a second,
// independently-derived readiness computation.
// ---------------------------------------------------------------------------

/// Evaluate strict Bundle 2 daily-data readiness for one resolved binding, at
/// a caller-supplied instant. Implementations must not read the wall clock,
/// must not call a provider or broker, and must not re-derive a runtime
/// bootstrap/binding of their own — `now_utc` and `binding` are always
/// supplied by the caller.
#[async_trait::async_trait]
pub trait AutonomousAssignmentReadinessEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        operation: &mqk_db::AutonomousDailyOperationRecord,
        binding: &ResolvedSingleBinding,
        now_utc: DateTime<Utc>,
    ) -> anyhow::Result<AssignmentReadiness>;
}

/// Production adapter: delegates to
/// [`daily_data_readiness::evaluate_readiness_with_binding`] — the identical
/// evaluation path every readiness route and the runtime start gate already
/// use — using an assignment config and runtime binding the caller resolved
/// once for this driver invocation (never a second env-driven bootstrap; see
/// the lifecycle-safety contract on
/// [`daily_data_readiness::evaluate_daily_data_readiness_from_env`]).
pub struct ProductionAutonomousAssignmentReadinessEvaluator {
    pool: PgPool,
    assignment_config: MultiSymbolRuntimeConfig,
    runtime_binding: EffectiveRuntimeBinding,
    context: DailyDataReadinessContext,
}

impl ProductionAutonomousAssignmentReadinessEvaluator {
    pub fn new(
        pool: PgPool,
        assignment_config: MultiSymbolRuntimeConfig,
        runtime_binding: EffectiveRuntimeBinding,
        context: DailyDataReadinessContext,
    ) -> Self {
        Self {
            pool,
            assignment_config,
            runtime_binding,
            context,
        }
    }
}

#[async_trait::async_trait]
impl AutonomousAssignmentReadinessEvaluator for ProductionAutonomousAssignmentReadinessEvaluator {
    async fn evaluate(
        &self,
        _operation: &mqk_db::AutonomousDailyOperationRecord,
        binding: &ResolvedSingleBinding,
        now_utc: DateTime<Utc>,
    ) -> anyhow::Result<AssignmentReadiness> {
        let report = daily_data_readiness::evaluate_readiness_with_binding(
            Some(&self.pool),
            &self.assignment_config,
            &self.runtime_binding,
            &self.context,
            now_utc,
        )
        .await;

        report
            .assignments
            .into_iter()
            .find(|a| {
                a.assignment_symbol
                    .trim()
                    .eq_ignore_ascii_case(&binding.symbol)
                    && a.assignment_timeframe.trim() == binding.timeframe.as_str()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no readiness assignment found matching resolved binding {}/{}",
                    binding.symbol,
                    binding.timeframe.as_str()
                )
            })
    }
}

/// REPAIR 2: pre-poll eligibility, derived from typed reason codes only —
/// never substring matching. A missing expected bar (optionally alongside
/// `insufficient_history`, when that blocker is present *because* the
/// expected tail bar itself has not arrived) is poll-remediable; any other
/// blocker, or the absence of a computable expected timestamp outside a
/// `"ready"` verdict, is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrePollEligibility {
    /// Bundle 2 already reports `"ready"` with no expected-bar concept for
    /// this timeframe (e.g. H1/M15, which never compute an expected window) —
    /// not blocked, but there is nothing to poll for this tick.
    NoExpectedBarConcept,
    /// The exact expected bar identity is known and every other precondition
    /// already holds; the only remaining question is whether it is present.
    Known { expected_end_ts: i64 },
    /// A non-remediable blocker (or a db_unavailable/query_failed readiness
    /// state) makes polling ineligible this tick — zero provider calls.
    NonRemediable { reason_code: &'static str },
}

fn classify_pre_poll_eligibility(readiness: &AssignmentReadiness) -> PrePollEligibility {
    let Some(expected_end_ts) = readiness.expected_latest_bar_ts else {
        if readiness.is_ready() {
            return PrePollEligibility::NoExpectedBarConcept;
        }
        let reason_code = match readiness.readiness_state {
            "db_unavailable" => "database_unavailable",
            "query_failed" => "query_failed",
            _ => readiness
                .blockers
                .first()
                .copied()
                .unwrap_or("expected_latest_bar_not_loaded"),
        };
        return PrePollEligibility::NonRemediable { reason_code };
    };

    for &blocker in &readiness.blockers {
        let remediable = blocker == REASON_EXPECTED_LATEST_BAR_MISSING
            || (blocker == REASON_INSUFFICIENT_HISTORY
                && readiness
                    .actual_latest_bar_ts
                    .map(|actual| actual < expected_end_ts)
                    .unwrap_or(true));
        if !remediable {
            return PrePollEligibility::NonRemediable {
                reason_code: blocker,
            };
        }
    }

    PrePollEligibility::Known { expected_end_ts }
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01
// Durable-evidence-inconsistency reason codes
// ---------------------------------------------------------------------------

/// `operation.last_completed_bar_ts == expected_ts`, but no `md_bars` row
/// exists for the exact expected bar identity.
pub const REASON_OBSERVED_BAR_MISSING_FROM_MD_BARS: &str = "observed_bar_missing_from_md_bars";
/// The exact expected bar row exists but `is_complete` is `false`.
pub const REASON_OBSERVED_BAR_INCOMPLETE: &str = "observed_bar_incomplete";
/// The exact expected bar row exists but its `provider_id` does not match
/// the canonical registry-resolved provider for this binding.
pub const REASON_OBSERVED_BAR_PROVIDER_MISMATCH: &str = "observed_bar_provider_mismatch";
/// The exact expected bar row exists but its `provider_symbol` does not
/// match the canonical registry-resolved provider symbol for this binding.
pub const REASON_OBSERVED_BAR_PROVIDER_SYMBOL_MISMATCH: &str =
    "observed_bar_provider_symbol_mismatch";

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01
// Explicit driver mode — the coordinator must choose; never inferred.
// ---------------------------------------------------------------------------

/// Explicit driver mode for one tick. Never inferred from the wall clock,
/// operation state, provider presence, pending-bar presence, or whether a
/// native-strategy bootstrap happens to be present — the caller (Phase D's
/// coordinator, or a test) must choose explicitly.
///
/// `PrepareDataOnly` owns binding/registry validation, readiness evaluation,
/// provider polling, and durable bar observation only. It never creates a
/// dispatch claim, deposits a pending strategy bar, or invokes native
/// strategy code. `BarObserved` in this mode proves only that the exact
/// expected bar is durably observed and ready — never that strategy dispatch
/// occurred.
///
/// `RunningDispatch` may perform the same observation reconciliation, but
/// additionally proves runtime-dispatch eligibility
/// ([`AutonomousStrategyDispatchRuntimeTruth`]) before creating a dispatch
/// claim or invoking native strategy code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousCompletedBarDriverMode {
    PrepareDataOnly,
    RunningDispatch,
}

/// REPAIR 8: `PrepareDataOnly` may operate only in these nonterminal
/// pre-runtime operation states. It must never operate while the operation
/// is actually `running`, winding down (`stopping`/`stop_retrying`),
/// manually blocked, degraded, or terminal (`completed*`) — those states are
/// either running-runtime's domain or not pollable at all. Conservative by
/// design: only the states the binding contract names are allowed; anything
/// else (including recovery/degraded states not named in the contract) is
/// refused rather than assumed safe.
fn prepare_data_only_state_eligible(state: &str) -> bool {
    matches!(
        state,
        mqk_db::STATE_AWAITING_PREOPEN
            | mqk_db::STATE_PREPARING_DATA
            | mqk_db::STATE_AWAITING_OPEN
            | mqk_db::STATE_PREFLIGHT_BLOCKED
            | mqk_db::STATE_START_RETRYING
    )
}

// ---------------------------------------------------------------------------
// REPAIR 4 — runtime-dispatch eligibility seam
// ---------------------------------------------------------------------------

/// Read-only runtime-dispatch eligibility truth for `RunningDispatch` mode,
/// reported by `AppState::autonomous_strategy_dispatch_runtime_truth`.
/// Production implementations must perform no runtime start/stop, no
/// bootstrap creation, no mutation of pending bars, no re-bootstrap, and no
/// provider/broker call — a pure read of already-established state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousStrategyDispatchRuntimeTruth {
    Active { run_id: Uuid },
    NoLocallyOwnedRun,
    NativeStrategyBootstrapMissing,
    NativeStrategyBootstrapDormant,
    NativeStrategyBootstrapFailed,
}

/// REPAIR 5: stable reason codes for [`AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady`].
pub const REASON_OPERATION_NOT_RUNNING: &str = "operation_not_running";
pub const REASON_OPERATION_RUN_ID_MISSING: &str = "operation_run_id_missing";
pub const REASON_LOCAL_RUNTIME_NOT_ACTIVE: &str = "local_runtime_not_active";
pub const REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH: &str = "local_runtime_run_id_mismatch";
pub const REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING: &str = "native_strategy_bootstrap_missing";
pub const REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT: &str = "native_strategy_bootstrap_dormant";
pub const REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED: &str = "native_strategy_bootstrap_failed";

/// REPAIR 3/4: prove every runtime-dispatch eligibility precondition before a
/// `RunningDispatch`-mode caller may create a dispatch claim. Typed checks
/// only — never a debug-string parse. `operation.state == running` alone
/// already excludes every state REPAIR 8 names as disallowed for dispatch
/// (awaiting_preopen, preparing_data, awaiting_open, preflight_blocked,
/// start_retrying, recovery_retrying, stopping, stop_retrying,
/// controller_degraded, evidence_degraded, calendar_unavailable,
/// manual_intervention_required, completed*) — none of them equal
/// `mqk_db::STATE_RUNNING`.
async fn prove_running_dispatch_eligibility(
    input: &AutonomousCompletedBarDriverInput<'_>,
) -> Result<(), AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;
    if operation.state != mqk_db::STATE_RUNNING {
        return Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_OPERATION_NOT_RUNNING,
            },
        );
    }
    let Some(operation_run_id) = operation.run_id else {
        return Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_OPERATION_RUN_ID_MISSING,
            },
        );
    };
    match input
        .state
        .autonomous_strategy_dispatch_runtime_truth()
        .await
    {
        AutonomousStrategyDispatchRuntimeTruth::Active { run_id } if run_id == operation_run_id => {
            Ok(())
        }
        AutonomousStrategyDispatchRuntimeTruth::Active { .. } => Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH,
            },
        ),
        AutonomousStrategyDispatchRuntimeTruth::NoLocallyOwnedRun => Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_LOCAL_RUNTIME_NOT_ACTIVE,
            },
        ),
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapMissing => Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
            },
        ),
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapDormant => Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT,
            },
        ),
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapFailed => Err(
            AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
                reason_code: REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED,
            },
        ),
    }
}

// ---------------------------------------------------------------------------
// C.14 — Driver outcome
// ---------------------------------------------------------------------------

/// Bounded, stable outcome of one driver tick. Reason codes and detail
/// strings are never parsed as authority by any caller.
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomousCompletedBarDriverOutcome {
    /// The operation is not in a state where autonomous polling applies
    /// (e.g. a terminal/manual-intervention state).
    NotApplicable {
        reason_code: &'static str,
    },
    /// `now_utc` is before `preopen_start_utc` or at/after
    /// `effective_operation_close_utc` — the driver is not active.
    OutsideOperationWindow,
    /// The operation row is a legacy row (predates
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-BOUNDARY-MODEL-REPAIR-01) with
    /// null exchange-calendar fields. Never substitutes the effective
    /// operation window for missing exchange truth.
    ExchangeSessionTruthMissing,
    AuthorizationDisabled,
    AuthorizationInvalid {
        reason_code: &'static str,
        detail: String,
    },
    BindingBlocked {
        rejection: AutonomousBindingRejection,
    },
    RegistryBlocked {
        rejection: LatestBarRegistryAdmissionRejection,
    },
    /// The strict Bundle 2 daily-data-readiness evaluation for this
    /// assignment is not `"ready"`.
    ReadinessBlocked {
        blockers: Vec<&'static str>,
    },
    /// The provider does not support the capability/timeframe needed.
    Unsupported {
        detail: String,
    },
    /// A completed bar for the current expected timestamp has already been
    /// observed — no poll attempted this tick.
    PollNotDue,
    /// A poll was attempted and succeeded, but the returned bar is not newer
    /// than the already-observed evidence.
    PollSucceededNoNewBar,
    /// A poll was attempted and the provider reported no completed bar yet.
    NoNewCompletedBar,
    /// A poll failed with a transient (retryable) provider error.
    PollFailedTransient {
        detail: String,
    },
    /// A poll failed with a non-transient provider error, or the returned
    /// bar failed provenance validation.
    PollFailedTerminal {
        detail: String,
    },
    /// A genuinely new completed bar was durably recorded as observed, but
    /// dispatch was not reached this call (reserved for callers that only
    /// want observation, not dispatch, in one tick).
    BarObserved {
        bar_end_ts: i64,
    },
    /// This exact bar identity was already durably dispatched previously.
    AlreadyDispatched {
        evaluation_id: Option<Uuid>,
    },
    /// A dispatch claim exists for this bar identity but is not provably
    /// complete (`claimed`, `uncertain`, or `failed`). No automatic
    /// redispatch.
    DispatchClaimUnresolved {
        status: String,
    },
    /// The canonical strategy dispatch call was invoked and confirmed
    /// success; the claim was marked completed and evidence advanced.
    DispatchCompleted {
        bar_end_ts: i64,
    },
    /// A durable evidence write did not find the expected operation row —
    /// never silently ignored.
    EvidencePersistenceFailed {
        detail: String,
    },
    /// REPAIR 4: the provider call succeeded and the returned bar was
    /// ingested, but its `end_ts` is older than the exact expected
    /// timestamp — the provider is lagging behind the exchange. No
    /// observation, no dispatch claim.
    ProviderLaggingExpectedBar {
        returned_bar_ts: i64,
        expected_end_ts: i64,
    },
    /// REPAIR 4: the provider call succeeded and the returned bar was
    /// ingested, but its `end_ts` is newer than the exact expected
    /// timestamp. No observation, no dispatch claim.
    UnexpectedOrFutureBar {
        returned_bar_ts: i64,
        expected_end_ts: i64,
    },
    /// REPAIR 5: the exact expected bar was located or ingested, but the
    /// mandatory post-poll strict-readiness re-evaluation is not ready (or
    /// no longer agrees the candidate bar is the expected/actual latest
    /// bar). No dispatch claim, no strategy call, no outbox work.
    ReadinessBlockedAfterPoll {
        blockers: Vec<&'static str>,
    },
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
    /// durable operation row already records `last_completed_bar_ts ==
    /// expected_ts`, but the exact canonical `md_bars` row backing that
    /// observation is missing, incomplete, or carries mismatched provider
    /// provenance. This is durable-evidence corruption (or external
    /// deletion), not a normal missing-tail-bar condition: zero provider
    /// calls, zero provider resolution, no dispatch claim, no strategy
    /// call.
    ObservedBarEvidenceInconsistent {
        expected_end_ts: i64,
        reason_code: &'static str,
    },
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: an
    /// attempt to record `expected_ts` as observed was refused because the
    /// operation's durable evidence already records a *later* bar. The
    /// older expected bar is never observed or dispatched.
    ObservedBarSequenceInconsistent {
        expected_end_ts: i64,
        current_last_completed_bar_ts: Option<i64>,
    },
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: a
    /// provider call was genuinely due (the exact expected bar is absent
    /// locally and authorization permits a poll), but lazily resolving the
    /// provider (registry lookup, credential loading, client construction)
    /// failed. No provider network call was made; no provider-poll
    /// attempt counter was incremented.
    ProviderSetupBlocked {
        rejection: AutonomousDriverSetupRejection,
    },
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01
    /// REPAIR 5: `RunningDispatch` mode could not prove runtime-dispatch
    /// eligibility before a dispatch claim would have been created. Zero
    /// dispatch claims, zero pending-bar deposits, zero strategy calls, zero
    /// OMS work. Never classified as a provider failure and never counted
    /// toward any provider-poll counter.
    RuntimeDispatchNotReady {
        reason_code: &'static str,
    },
}

/// Fixed, bounded poll-retry cooldown. Phase D owns full typed
/// transient/terminal backoff policy for runtime-start retries; this is a
/// minimal, honest bound preventing busy-polling on a failed provider call
/// within Phase C's scope.
const POLL_RETRY_COOLDOWN_SECS: i64 = 60;

/// Input for [`tick_autonomous_completed_bar_driver`]. Every field is
/// caller-supplied; the driver never reads the wall clock, never reads env,
/// and never re-derives a session plan, assignment, or binding on its own.
pub struct AutonomousCompletedBarDriverInput<'a> {
    pub state: &'a AppState,
    pub pool: &'a PgPool,
    pub operation: &'a mqk_db::AutonomousDailyOperationRecord,
    pub assignment_config: &'a MultiSymbolRuntimeConfig,
    pub assignment_identity: &'a str,
    pub runtime_binding: &'a EffectiveRuntimeBinding,
    pub runtime_binding_identity: &'a str,
    pub now_utc: DateTime<Utc>,
    pub authorization: AutonomousProviderCallAuthorization,
    pub instruments: &'a [TrackedInstrument],
    pub provider_id: &'a str,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: a
    /// lazy provider-resolution seam, invoked at most once per tick and
    /// only once every non-provider precondition has passed and a poll is
    /// genuinely about to happen — never on the local exact-bar path, and
    /// never when `authorization` is `Disabled`/`Invalid`. Replaces the
    /// previously mandatory already-built `&dyn MarketDataProvider`, which
    /// forced provider-registry construction and credential loading on
    /// every tick regardless of whether a provider call was needed.
    pub provider_resolver: &'a dyn AutonomousLatestBarProviderResolver,
    /// REPAIR 1: an injected evaluator seam, called at least twice per tick
    /// that reaches the bar stage — once (pre-poll) to determine polling
    /// eligibility, and once more (post-poll) from current DB truth to
    /// authorize dispatch. Never a single stale snapshot reused for both
    /// decisions.
    pub readiness_evaluator: &'a dyn AutonomousAssignmentReadinessEvaluator,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01:
    /// explicit, caller-chosen driver mode. Never defaulted, never inferred.
    pub mode: AutonomousCompletedBarDriverMode,
}

/// Perform one autonomous completed-bar driver tick. Returns a stable typed
/// outcome; returns `Err` only for an unexpected DB connectivity failure
/// (which necessarily prevents dispatch — see C.11's "a DB failure before a
/// provider call causes zero provider calls").
pub async fn tick_autonomous_completed_bar_driver(
    input: AutonomousCompletedBarDriverInput<'_>,
) -> anyhow::Result<AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;

    if mqk_db::is_terminal_operation_state(&operation.state)
        || operation.state == mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED
    {
        return Ok(AutonomousCompletedBarDriverOutcome::NotApplicable {
            reason_code: "operation_not_in_pollable_state",
        });
    }

    if input.now_utc < operation.preopen_start_utc
        || input.now_utc >= operation.effective_operation_close_utc
    {
        return Ok(AutonomousCompletedBarDriverOutcome::OutsideOperationWindow);
    }

    // REPAIR 8: `PrepareDataOnly` is a pre-runtime concern only. Once the
    // operation has left the states the binding contract names for
    // preparation (e.g. it is actually `running`, winding down, manually
    // blocked, or terminal), preparation-mode ticks must refuse rather than
    // silently continue polling/observing on the coordinator's behalf.
    if input.mode == AutonomousCompletedBarDriverMode::PrepareDataOnly
        && !prepare_data_only_state_eligible(&operation.state)
    {
        return Ok(AutonomousCompletedBarDriverOutcome::NotApplicable {
            reason_code: "operation_not_in_preparation_state",
        });
    }

    let (Some(exchange_open), Some(exchange_close), Some(_early_close), Some(_prev_date)) = (
        operation.exchange_session_open_utc,
        operation.exchange_session_close_utc,
        operation.exchange_is_early_close,
        operation.previous_trading_date,
    ) else {
        return Ok(AutonomousCompletedBarDriverOutcome::ExchangeSessionTruthMissing);
    };
    let _ = (exchange_open, exchange_close); // exchange truth proven present; consumers of the
                                             // exact values are the readiness/session-plan layers,
                                             // not this gate.

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01:
    // `authorization` is no longer checked here. It authorizes provider
    // *calls* only, and must never block resolving a binding, reading the
    // instrument registry, evaluating readiness, or using canonical data
    // already stored in `md_bars`. See `reconcile_observed_expected_bar`
    // and `poll_missing_expected_bar` for exactly where it is enforced.

    let binding = match resolve_single_effective_binding(
        operation,
        input.assignment_config,
        input.assignment_identity,
        input.runtime_binding,
        input.runtime_binding_identity,
    ) {
        Ok(binding) => binding,
        Err(rejection) => {
            return Ok(AutonomousCompletedBarDriverOutcome::BindingBlocked { rejection });
        }
    };

    let target: ResolvedLatestBarPollTarget = match resolve_latest_bar_poll_target(
        input.instruments,
        input.provider_id,
        &binding.symbol,
        binding.timeframe,
    ) {
        Ok(target) => target,
        Err(rejection) => {
            return Ok(AutonomousCompletedBarDriverOutcome::RegistryBlocked { rejection });
        }
    };

    // REPAIR 1/2 — first (pre-poll) strict-readiness evaluation: gates
    // *eligibility to poll*, not dispatch. A missing expected bar alone
    // never blocks polling.
    let pre_poll_readiness = input
        .readiness_evaluator
        .evaluate(operation, &binding, input.now_utc)
        .await?;

    let expected_ts = match classify_pre_poll_eligibility(&pre_poll_readiness) {
        PrePollEligibility::NoExpectedBarConcept => {
            return Ok(AutonomousCompletedBarDriverOutcome::PollNotDue);
        }
        PrePollEligibility::NonRemediable { .. } => {
            return Ok(AutonomousCompletedBarDriverOutcome::ReadinessBlocked {
                blockers: pre_poll_readiness.blockers.clone(),
            });
        }
        PrePollEligibility::Known { expected_end_ts } => expected_end_ts,
    };

    reconcile_observed_expected_bar(&input, &binding, &target, expected_ts).await
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
/// single helper responsible for the entire observed-bar section of one
/// driver tick once the exact expected bar identity (`expected_ts`) is
/// known. Required order:
///
/// 1. Look up the exact canonical bar already in `md_bars` (REPAIR 3,
///    kept) — a single-row equality query, zero provider calls.
/// 2. If it exists with matching provenance (complete, matching
///    `provider_id`/`provider_symbol`): use it. Zero provider calls, zero
///    provider resolution, zero credential reads — regardless of whether
///    `authorization` is `Authorized`, `Disabled`, or `Invalid`.
/// 3. Otherwise (absent, or present with mismatched provenance): if the
///    operation's own durable evidence (`last_completed_bar_ts`) already
///    claims this exact bar was observed, that is durable-evidence
///    corruption, not a normal missing-tail-bar condition — fail closed
///    ([`AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent`]),
///    zero provider calls.
/// 4. Otherwise this is a normal missing-bar tick — delegate to
///    [`poll_missing_expected_bar`], where `authorization` gates whether a
///    provider may be resolved and polled at all.
async fn reconcile_observed_expected_bar(
    input: &AutonomousCompletedBarDriverInput<'_>,
    binding: &ResolvedSingleBinding,
    target: &ResolvedLatestBarPollTarget,
    expected_ts: i64,
) -> anyhow::Result<AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;

    // REPAIR 3 — exact expected-bar lookup: avoid a provider call entirely
    // when the exact expected canonical bar already exists in `md_bars`.
    // Zero provider-poll counters are touched on this path (REPAIR 6).
    let exact_local = mqk_db::md::fetch_exact_bar_with_provenance(
        input.pool,
        &binding.symbol,
        binding.timeframe.as_str(),
        expected_ts,
    )
    .await?;

    if let Some(row) = &exact_local {
        if row.is_complete
            && row.provider_id.eq_ignore_ascii_case(&target.provider_id)
            && row
                .provider_symbol
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&target.provider_symbol))
                .unwrap_or(false)
        {
            // Local path: the exact expected bar is already trusted
            // canonical data. Zero provider calls regardless of
            // `authorization`.
            return observe_and_dispatch_if_ready(input, binding, expected_ts).await;
        }
    }

    // The exact expected bar is not usable locally (absent, or present with
    // mismatched provenance). If durable evidence already claims it was
    // observed, this is evidence corruption — never automatically re-poll
    // and pretend the prior observation never happened.
    if operation.last_completed_bar_ts == Some(expected_ts) {
        let reason_code = match &exact_local {
            None => REASON_OBSERVED_BAR_MISSING_FROM_MD_BARS,
            Some(row) if !row.is_complete => REASON_OBSERVED_BAR_INCOMPLETE,
            Some(row) if !row.provider_id.eq_ignore_ascii_case(&target.provider_id) => {
                REASON_OBSERVED_BAR_PROVIDER_MISMATCH
            }
            Some(_) => REASON_OBSERVED_BAR_PROVIDER_SYMBOL_MISMATCH,
        };
        return Ok(
            AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
                expected_end_ts: expected_ts,
                reason_code,
            },
        );
    }

    poll_missing_expected_bar(input, binding, target, expected_ts).await
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
/// missing-bar path — reached only when the exact expected bar is not
/// already trusted canonical data. `authorization` and the lazy provider
/// resolver are consulted here, and only here.
async fn poll_missing_expected_bar(
    input: &AutonomousCompletedBarDriverInput<'_>,
    binding: &ResolvedSingleBinding,
    target: &ResolvedLatestBarPollTarget,
    expected_ts: i64,
) -> anyhow::Result<AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;

    match &input.authorization {
        AutonomousProviderCallAuthorization::Disabled => {
            return Ok(AutonomousCompletedBarDriverOutcome::AuthorizationDisabled);
        }
        AutonomousProviderCallAuthorization::Invalid {
            reason_code,
            detail,
        } => {
            return Ok(AutonomousCompletedBarDriverOutcome::AuthorizationInvalid {
                reason_code,
                detail: detail.clone(),
            });
        }
        AutonomousProviderCallAuthorization::Authorized => {}
    }

    // C.7 — poll cadence: a bounded cooldown must elapse since the last
    // provider-poll attempt for this operation before trying again.
    if let Some(last_poll) = operation.last_provider_poll_utc {
        if (input.now_utc - last_poll).num_seconds() < POLL_RETRY_COOLDOWN_SECS {
            return Ok(AutonomousCompletedBarDriverOutcome::PollNotDue);
        }
    }

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01: the
    // provider is resolved (registry lookup, credential reads, client
    // construction) only here — every non-provider precondition has
    // passed and a poll is genuinely about to happen. A construction
    // failure never increments any provider-poll counter: no provider
    // request was ever made.
    let provider = match input.provider_resolver.resolve(input.provider_id) {
        Ok(provider) => provider,
        Err(rejection) => {
            return Ok(AutonomousCompletedBarDriverOutcome::ProviderSetupBlocked { rejection });
        }
    };

    let data_refresh_state_polling = "polling";
    match mqk_db::record_provider_poll_started(
        input.pool,
        operation.operation_id,
        input.now_utc,
        data_refresh_state_polling,
    )
    .await?
    {
        mqk_db::RecordProviderPollOutcome::Recorded { .. } => {}
        mqk_db::RecordProviderPollOutcome::NotFound => {
            return Ok(
                AutonomousCompletedBarDriverOutcome::EvidencePersistenceFailed {
                    detail: "operation row not found while recording provider-poll start"
                        .to_string(),
                },
            );
        }
    }

    // REPAIR 4 — the provider's result must equal the exact expected bar to
    // be treated as observed/dispatchable.
    let poll_outcome = poll_and_ingest_latest_closed_bar(
        input.pool,
        LatestBarPollSeamInput {
            provider: provider.as_ref(),
            target,
            now_utc: input.now_utc,
            ingest_mode: "autonomous_daily_operation_driver",
            expected_bar_constraint: Some(ExpectedLatestBarConstraint {
                exact_end_ts: expected_ts,
            }),
        },
    )
    .await;

    match poll_outcome {
        LatestBarPollOutcome::Unsupported { detail } => {
            mqk_db::record_provider_poll_failed(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "unsupported",
                &detail,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::Unsupported { detail })
        }
        LatestBarPollOutcome::NoCompletedBarAvailable => {
            mqk_db::record_provider_poll_succeeded(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "no_completed_bar_available",
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::NoNewCompletedBar)
        }
        LatestBarPollOutcome::ProviderTemporarilyUnavailable { detail } => {
            mqk_db::record_provider_poll_failed(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "provider_temporarily_unavailable",
                &detail,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::PollFailedTransient { detail })
        }
        LatestBarPollOutcome::ProviderRejected { detail } => {
            mqk_db::record_provider_poll_failed(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "provider_rejected",
                &detail,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::PollFailedTerminal { detail })
        }
        LatestBarPollOutcome::ProvenanceRejected { detail, .. } => {
            mqk_db::record_provider_poll_failed(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "provenance_rejected",
                &detail,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::PollFailedTerminal { detail })
        }
        LatestBarPollOutcome::DatabaseFailure { detail, .. } => {
            mqk_db::record_provider_poll_failed(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "database_failure",
                &detail,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::EvidencePersistenceFailed { detail })
        }
        // REPAIR 6: the provider call itself succeeded — success count
        // increments — but the returned bar does not match the exact
        // expected identity, so it is never treated as observed/dispatched.
        LatestBarPollOutcome::ProviderLaggingExpectedBar {
            returned_bar_ts,
            expected_end_ts,
        } => {
            mqk_db::record_provider_poll_succeeded(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "provider_lagging_expected_bar",
            )
            .await?;
            Ok(
                AutonomousCompletedBarDriverOutcome::ProviderLaggingExpectedBar {
                    returned_bar_ts,
                    expected_end_ts,
                },
            )
        }
        LatestBarPollOutcome::UnexpectedOrFutureBar {
            returned_bar_ts,
            expected_end_ts,
        } => {
            mqk_db::record_provider_poll_succeeded(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "unexpected_or_future_bar",
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::UnexpectedOrFutureBar {
                returned_bar_ts,
                expected_end_ts,
            })
        }
        LatestBarPollOutcome::InsertedNewBar { end_ts, .. }
        | LatestBarPollOutcome::AlreadyStored { end_ts, .. } => {
            mqk_db::record_provider_poll_succeeded(
                input.pool,
                operation.operation_id,
                input.now_utc,
                "bar_received",
            )
            .await?;
            debug_assert_eq!(
                end_ts, expected_ts,
                "poll_and_ingest_latest_closed_bar must enforce the exact expected-bar constraint"
            );
            observe_and_dispatch_if_ready(input, binding, end_ts).await
        }
    }
}

/// REPAIR 3/5 (and AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-OBSERVED-BAR-RECOVERY-01
/// DEFECT 2): record the exact expected bar as observed (idempotent), then
/// re-evaluate strict readiness (the mandatory *second* evaluation) from
/// current DB truth before authorizing dispatch. Reached both when the exact
/// bar was already present locally (no poll) and when a poll just ingested
/// it.
///
/// `AlreadyObserved` is no longer treated as terminal: it proves only that
/// the bar was observed at some point, never that readiness later passed or
/// that a dispatch claim exists/completed. Both `Recorded` (first
/// observation — including the crash-recovery case where a prior process
/// observed the bar but crashed before claiming its dispatch, so a fresh
/// `AppState`/evaluator/operation snapshot reaches this function fresh) and
/// `AlreadyObserved` (readiness may have been blocked on an earlier tick, or
/// the claim was never reached) continue identically into the mandatory
/// readiness re-evaluation and claim reconciliation below. `StaleBarIgnored`
/// means the operation's durable evidence already records a strictly newer
/// bar than `bar_end_ts` — the older bar is never observed or dispatched.
async fn observe_and_dispatch_if_ready(
    input: &AutonomousCompletedBarDriverInput<'_>,
    binding: &ResolvedSingleBinding,
    bar_end_ts: i64,
) -> anyhow::Result<AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;

    match mqk_db::record_completed_bar_observed(
        input.pool,
        operation.operation_id,
        bar_end_ts,
        input.now_utc,
    )
    .await?
    {
        mqk_db::RecordCompletedBarObservedOutcome::NotFound => Ok(
            AutonomousCompletedBarDriverOutcome::EvidencePersistenceFailed {
                detail: "operation row not found while recording bar observation".to_string(),
            },
        ),
        mqk_db::RecordCompletedBarObservedOutcome::StaleBarIgnored {
            current_last_completed_bar_ts,
        } => Ok(
            AutonomousCompletedBarDriverOutcome::ObservedBarSequenceInconsistent {
                expected_end_ts: bar_end_ts,
                current_last_completed_bar_ts,
            },
        ),
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { .. }
        | mqk_db::RecordCompletedBarObservedOutcome::AlreadyObserved { .. } => {
            // REPAIR 5 — mandatory post-poll (second) strict-readiness
            // re-evaluation from current DB truth. A successful ingest (or
            // an already-observed exact local bar) alone never authorizes
            // dispatch — this re-evaluation runs on every eligible tick,
            // including a tick where it was blocked before and only now
            // becomes ready.
            let post_poll_readiness = input
                .readiness_evaluator
                .evaluate(operation, binding, input.now_utc)
                .await?;

            let ready_for_this_bar = post_poll_readiness.is_ready()
                && post_poll_readiness.expected_latest_bar_ts == Some(bar_end_ts)
                && post_poll_readiness.actual_latest_bar_ts == Some(bar_end_ts);

            if !ready_for_this_bar {
                return Ok(
                    AutonomousCompletedBarDriverOutcome::ReadinessBlockedAfterPoll {
                        blockers: post_poll_readiness.blockers.clone(),
                    },
                );
            }

            // REPAIR 6: branch by explicit mode. `PrepareDataOnly` stops
            // here — the exact bar is durably observed and ready, but that
            // is not the same thing as strategy dispatch having occurred.
            // `RunningDispatch` must additionally prove runtime-dispatch
            // eligibility before a claim is ever created.
            match input.mode {
                AutonomousCompletedBarDriverMode::PrepareDataOnly => {
                    Ok(AutonomousCompletedBarDriverOutcome::BarObserved { bar_end_ts })
                }
                AutonomousCompletedBarDriverMode::RunningDispatch => {
                    if let Err(not_ready) = prove_running_dispatch_eligibility(input).await {
                        return Ok(not_ready);
                    }
                    claim_and_dispatch_observed_bar(input, binding, bar_end_ts).await
                }
            }
        }
    }
}

/// C.10-C.13: claim the durable dispatch identity for `bar_end_ts`, and only
/// on a fresh claim, deposit + consume the canonical
/// `AppState::tick_strategy_dispatch_for_symbol` path — the exact same
/// production dispatch path used by the existing autonomous bar ticker and
/// manual signal route, never a parallel strategy implementation. Never
/// redispatches an already-completed or unresolved claim.
async fn claim_and_dispatch_observed_bar(
    input: &AutonomousCompletedBarDriverInput<'_>,
    binding: &ResolvedSingleBinding,
    bar_end_ts: i64,
) -> anyhow::Result<AutonomousCompletedBarDriverOutcome> {
    let operation = input.operation;

    let claim = mqk_db::claim_autonomous_daily_bar_dispatch(
        input.pool,
        operation.operation_id,
        &binding.symbol,
        binding.timeframe.as_str(),
        bar_end_ts,
        input.now_utc,
    )
    .await?;

    match claim {
        mqk_db::BarDispatchClaimOutcome::AlreadyCompleted { evaluation_id } => {
            return Ok(AutonomousCompletedBarDriverOutcome::AlreadyDispatched { evaluation_id });
        }
        mqk_db::BarDispatchClaimOutcome::Unresolved { status } => {
            return Ok(AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved { status });
        }
        mqk_db::BarDispatchClaimOutcome::Claimed => {}
    }

    input
        .state
        .deposit_strategy_bar_input(StrategyBarInput {
            now_tick: operation.bars_observed.max(0) as u64,
            end_ts: bar_end_ts,
            limit_price: None,
            qty: 1,
        })
        .await;

    let dispatch_result = input
        .state
        .tick_strategy_dispatch_for_symbol(&binding.symbol, binding.timeframe.as_str())
        .await;

    match dispatch_result {
        Some(_result) => {
            mqk_db::complete_autonomous_daily_bar_dispatch(
                input.pool,
                operation.operation_id,
                &binding.symbol,
                binding.timeframe.as_str(),
                bar_end_ts,
                input.now_utc,
                None,
            )
            .await?;
            Ok(AutonomousCompletedBarDriverOutcome::DispatchCompleted { bar_end_ts })
        }
        None => {
            mqk_db::fail_autonomous_daily_bar_dispatch(
                input.pool,
                operation.operation_id,
                &binding.symbol,
                binding.timeframe.as_str(),
                bar_end_ts,
                "canonical strategy dispatch returned no result despite a fresh claim and \
                 passing pre-dispatch readiness gates",
            )
            .await?;
            Ok(
                AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
                    status: mqk_db::DISPATCH_STATUS_FAILED.to_string(),
                },
            )
        }
    }
}

// ---------------------------------------------------------------------------
// C.15 — bounded task/liveness foundation (not started by Phase C)
// ---------------------------------------------------------------------------

/// Bounded liveness truth for a future task runner. Phase D owns actually
/// spawning, supervising, and restarting this task; Phase C only defines the
/// truth values it would report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousCompletedBarDriverTaskLiveness {
    NotStarted,
    Running,
    Waiting,
    Blocked,
    Stopped,
    Failed,
}

/// A cancellation-respecting, bounded-cadence tick loop shape for Phase D to
/// adopt. Deliberately not `pub fn spawn(...)` here — Phase C does not start
/// this task from `main.rs`, the session controller, or anywhere else; this
/// type exists so Phase D can drive ticks without duplicating the
/// cancellation/cadence plumbing.
pub struct AutonomousCompletedBarDriverTaskConfig {
    pub tick_cadence: std::time::Duration,
}

impl AutonomousCompletedBarDriverTaskConfig {
    pub fn new(tick_cadence: std::time::Duration) -> Self {
        Self { tick_cadence }
    }
}

/// Run one bounded-cadence tick loop, calling `on_tick` once per cadence
/// interval until `cancel` resolves. `on_tick` is caller-supplied so this
/// loop performs no provider/DB/runtime action of its own — it is pure
/// cadence plumbing. Never spawned automatically by this patch.
pub async fn run_bounded_cadence_task<F, Fut>(
    config: AutonomousCompletedBarDriverTaskConfig,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    mut on_tick: F,
) -> AutonomousCompletedBarDriverTaskLiveness
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut interval = tokio::time::interval(config.tick_cadence);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                on_tick().await;
            }
            changed = cancel.changed() => {
                match changed {
                    Ok(()) if *cancel.borrow() => {
                        return AutonomousCompletedBarDriverTaskLiveness::Stopped;
                    }
                    Ok(()) => {}
                    // Sender dropped: no further cancellation signal will ever
                    // arrive. Stop rather than spin on an always-ready future.
                    Err(_) => return AutonomousCompletedBarDriverTaskLiveness::Stopped,
                }
            }
        }
    }
}
