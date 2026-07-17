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
// an already-built provider trait object (so tests can inject fakes without
// touching the filesystem or a provider factory). This wrapper is the thin
// production seam that loads and validates both from disk before the tick
// function ever runs — reusing the exact same `mqk_md::instrument_registry`
// / `mqk_md::provider_registry` functions the manual poll-once route and
// historical provider sync already use (C.8), never ad hoc parsing. A
// rejection here happens strictly before any provider call.

/// Why the driver's registry/provider setup could not be resolved. Every
/// variant is reached before any provider network call.
#[derive(Debug, Clone)]
pub enum AutonomousDriverSetupRejection {
    InstrumentRegistryUnavailable(String),
    InstrumentRegistryInvalid(String),
    ProviderRegistryUnavailable(String),
    ProviderUnknownOrDisabled(String),
    ProviderConstructionFailed(String),
}

/// Load and validate the instrument registry, and resolve/build the market
/// data provider, from paths and an already-resolved `provider_id`. Zero
/// provider calls are made by this function itself — provider construction
/// only prepares a client, it never calls `fetch_latest_closed_bar`.
pub fn load_driver_instruments_and_provider(
    instrument_registry_path: &str,
    provider_registry_path: &str,
    provider_id: &str,
) -> Result<(Vec<TrackedInstrument>, mqk_md::MarketDataProviderBox), AutonomousDriverSetupRejection>
{
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        instrument_registry_path,
    ))
    .map_err(|e| AutonomousDriverSetupRejection::InstrumentRegistryUnavailable(e.to_string()))?;
    mqk_md::instrument_registry::validate_registry(&instruments)
        .map_err(|e| AutonomousDriverSetupRejection::InstrumentRegistryInvalid(e.to_string()))?;

    let providers = mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
        provider_registry_path,
    ))
    .map_err(|e| AutonomousDriverSetupRejection::ProviderRegistryUnavailable(e.to_string()))?;
    let config = mqk_md::provider_registry::find_provider(&providers, provider_id)
        .filter(|c| c.enabled)
        .ok_or_else(|| {
            AutonomousDriverSetupRejection::ProviderUnknownOrDisabled(provider_id.to_string())
        })?;
    let provider =
        mqk_md::build_market_data_provider_from_config(config, |name| std::env::var(name).ok())
            .map_err(|e| {
                AutonomousDriverSetupRejection::ProviderConstructionFailed(e.to_string())
            })?;

    Ok((instruments, provider))
}

// ---------------------------------------------------------------------------
// C.2/C.3 — Two-part autonomous provider-call authorization
// ---------------------------------------------------------------------------

pub const AUTONOMOUS_DATA_REFRESH_ENABLED_ENV: &str = "MQK_AUTONOMOUS_DATA_REFRESH_ENABLED";
pub const ALLOW_PROVIDER_API_CALLS_ENV: &str = "MQK_ALLOW_PROVIDER_API_CALLS";

/// Frozen logical gate: `autonomous_data_refresh_enabled == true AND
/// allow_provider_api_calls == true`. PAPER mode alone is never sufficient
/// authorization to make a provider call.
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
    pub provider: &'a dyn mqk_md::MarketDataProvider,
    /// REPAIR 1: an injected evaluator seam, called at least twice per tick
    /// that reaches the bar stage — once (pre-poll) to determine polling
    /// eligibility, and once more (post-poll) from current DB truth to
    /// authorize dispatch. Never a single stale snapshot reused for both
    /// decisions.
    pub readiness_evaluator: &'a dyn AutonomousAssignmentReadinessEvaluator,
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

    // C.7 — poll cadence: due iff the exact expected bar has not already
    // been observed, and (if a prior attempt for this exact expected bar
    // exists) a bounded cooldown has elapsed.
    if operation.last_completed_bar_ts == Some(expected_ts) {
        return Ok(AutonomousCompletedBarDriverOutcome::PollNotDue);
    }

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
    if let Some(row) = exact_local {
        if row.is_complete
            && row.provider_id.eq_ignore_ascii_case(&target.provider_id)
            && row
                .provider_symbol
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&target.provider_symbol))
                .unwrap_or(false)
        {
            return observe_and_dispatch_if_ready(&input, &binding, expected_ts).await;
        }
    }

    if let Some(last_poll) = operation.last_provider_poll_utc {
        if (input.now_utc - last_poll).num_seconds() < POLL_RETRY_COOLDOWN_SECS {
            return Ok(AutonomousCompletedBarDriverOutcome::PollNotDue);
        }
    }

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
            provider: input.provider,
            target: &target,
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
            observe_and_dispatch_if_ready(&input, &binding, end_ts).await
        }
    }
}

/// REPAIR 3/5: record the exact expected bar as observed (idempotent), then
/// re-evaluate strict readiness (the mandatory *second* evaluation) from
/// current DB truth before authorizing dispatch. Reached both when the exact
/// bar was already present locally (no poll) and when a poll just ingested
/// it.
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
        mqk_db::RecordCompletedBarObservedOutcome::AlreadyObserved { .. }
        | mqk_db::RecordCompletedBarObservedOutcome::StaleBarIgnored { .. } => {
            Ok(AutonomousCompletedBarDriverOutcome::PollSucceededNoNewBar)
        }
        mqk_db::RecordCompletedBarObservedOutcome::Recorded { .. } => {
            // REPAIR 5 — mandatory post-poll (second) strict-readiness
            // re-evaluation from current DB truth. A successful ingest alone
            // never authorizes dispatch.
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

            claim_and_dispatch_observed_bar(input, binding, bar_end_ts).await
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
