//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D1: typed autonomous coordinator
//! policy — closed reason model, exhaustive retry classification,
//! deterministic blocker signatures, and bounded backoff.
//!
//! Foundation only (Phase D1). Nothing in this module is called by
//! `session_controller.rs`, `attempt_auto_start`, `attempt_auto_stop`, any
//! task-wiring, or any durable-operation transition in this patch. The
//! frozen retry-class contract this module implements is
//! `docs/specs/autonomous_daily_paper_operations_01a_current_truth_and_contract.md`
//! §15 (`AutonomousRetryClass`) and the 30s/60s/120s/300s-cap backoff
//! schedule.
//!
//! ## Source-grounded fault-class classification table (D1.4, D1D1-POLICY-CLOSURE-REPAIR-01)
//!
//! Every `RuntimeLifecycleError::fault_class()` string reachable from
//! `AppState::start_execution_runtime` (`state/lifecycle.rs`, including
//! `create_or_reuse_run_for_start`, `db_pool`, and
//! `ProductionRuntimeStartEffects::start_runtime_effects`), as audited for
//! this patch. The conversion below matches on `RuntimeLifecycleError`'s
//! *variant first*, then on `fault_class()` — the same static fault-class
//! string can carry a different meaning depending on the variant that
//! produced it (see the `runtime.start_refused.service_unavailable` rows
//! below), so a combined `ServiceUnavailable | Forbidden | Conflict` match
//! arm would silently conflate two distinct facts:
//!
//! | fault_class (or `Internal` context label) | source variant | coordinator reason | retry class |
//! |---|---|---|---|
//! | `runtime.start_refused.service_unavailable` (`db_pool`, DB absent) | `ServiceUnavailable` | `DatabaseNotConfiguredOrInvalid` | Manual |
//! | `runtime.start_refused.service_unavailable` (`start_runtime_effects`'s `orchestrator.tick()`, DB-backed runtime leadership lease temporarily unavailable — `"RUNTIME_LEASE"` in the effects error is converted to `RuntimeStartEffectsErrorKind::Conflict` by `start_runtime_effects`, never surfaced as a string here) | `Conflict` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `runtime.start_refused.service_unavailable` (not currently reachable as `Forbidden`) | `Forbidden` | `UnclassifiedFailClosed` (conservative fail-closed; no source site produces this combination today) | Manual |
//! | `start active-run lookup failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start latest-run lookup failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start insert_run failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start strategy_registry lookup failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start arm_run failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start begin_run failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start initial heartbeat failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `post-initial-tick heartbeat refresh failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start initial tick failed` (never transient — may wrap orchestrator, broker, continuity, risk, or other runtime failure, not proven to be a temporary DB operation from the static label alone) | `Internal` | `UnclassifiedFailClosed` | Manual |
//! | `runtime.control_refusal.integrity_disarmed` | `Forbidden` | `IntegrityHalted` | Manual |
//! | `runtime.start_refused.reconcile_dirty` (covers both `"dirty"`/`"stale"` source status strings) | `Forbidden` | `ReconcileDirty` | Manual |
//! | `runtime.start_refused.native_strategy_bootstrap_failed` | `Forbidden` | `NativeStrategyBootstrapFailed` | Manual |
//! | `runtime.start_refused.strategy_bootstrap_dormant` | `Forbidden` | `NativeStrategyBootstrapDormant` | Manual |
//! | `runtime.start_refused.readiness_evidence_persist_failed` | `ServiceUnavailable` | `ReadinessEvidencePersistFailed` | Manual |
//! | `runtime.start_refused.readiness_run_link_persist_failed` | `ServiceUnavailable` | `ReadinessRunLinkPersistFailed` | Manual |
//! | `runtime.truth_mismatch.durable_active_without_local_owner` | `Conflict` | `DurableActiveRunWithoutLocalOwner` | Manual |
//! | every other fault class (deployment-mode, capital/artifact/parity/economics gates, WS-continuity-unproven, `daily_data_readiness_blocked`, `strategy_registry_missing`/`disabled`, `market_data_not_fresh`, `fixed_window_override_invalid`, `already_owned`, `halted_lifecycle`, `durable_run_active`, any other `Internal` context, and any unrecognized label on a known variant) | any | `UnclassifiedFailClosed { fault_class }` | Manual |
//!
//! Notes on the conservative fallback (never a guess in the transient or
//! wait-for-condition direction):
//! - `daily_data_readiness_blocked` wraps many distinct sub-reasons
//!   (assignment/strategy/symbol/timeframe mismatch, `db_unavailable`,
//!   `query_failed`, `calendar_unavailable`, provider/instrument identity,
//!   ...), but `RuntimeLifecycleError` carries only one static
//!   `fault_class` for all of them — the specific sub-reason lives only
//!   inside the free-form `message` string (`report.top_level_blocker`
//!   embedded via `format!`). Per this module's no-string-authority rule
//!   (D1.8) the lifecycle-error conversion never parses that message, so
//!   this fault class conservatively fails closed here. A future phase that
//!   passes the typed `daily_data_readiness` report directly (not through
//!   `RuntimeLifecycleError`) may classify it more precisely without
//!   violating this rule.
//! - `paper_alpaca_ws_continuity_unproven` / `capital_ws_continuity_unproven`
//!   cover `ColdStartUnproven`, `GapDetected`, and any other non-proven
//!   continuity state alike — `RuntimeLifecycleError` does not carry the
//!   underlying `AlpacaWsContinuityState`. Conservatively fails closed
//!   rather than guessing `WsGapDetected` or `WsReconnecting`. A caller with
//!   access to the typed continuity value should use
//!   [`coordinator_reason_from_ws_continuity`] instead, which distinguishes
//!   `GapDetected` precisely.
//! - `strategy_registry_missing` / `strategy_registry_disabled` are a
//!   distinct DB-registry-truth check from the `NativeStrategyBootstrap`
//!   plugin-registry outcome (`NativeStrategyBootstrapMissing` names a
//!   different fact — the in-memory bootstrap field being absent, read by
//!   the completed-bar driver). Conservatively fails closed rather than
//!   conflating the two.
//! - No fault class audited on this path is reachable as
//!   `RetryableTransient` or `WaitForCondition` today except the eight
//!   `Internal` DB-operation labels above and the `Conflict`-variant
//!   `runtime.start_refused.service_unavailable` (runtime-lease-unavailable)
//!   case — the `WaitForCondition` reasons and `WsReconnecting` are reserved
//!   for the daily coordinator's own typed facts (see the typed-fact
//!   adapters below), not yet emitted by any gate on the
//!   `start_execution_runtime` path.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};

use super::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverOutcome, AutonomousStrategyDispatchRuntimeTruth,
    REASON_LOCAL_RUNTIME_NOT_ACTIVE, REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH,
    REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT, REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED,
    REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
};
use super::autonomous_daily_operation::{
    AutonomousDailyPlanReason, AutonomousDailySessionPlanResolution,
};
use super::{AlpacaWsContinuityState, RuntimeLifecycleError};

use mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome;

// ---------------------------------------------------------------------------
// D1.2 — Typed coordinator reasons
// ---------------------------------------------------------------------------

/// Closed autonomous-coordinator blocker/reason model. Never a broad
/// free-text variant (`Other(String)`/`Unknown(String)`) — the one
/// data-carrying variant, [`UnclassifiedFailClosed`], is bounded to a single
/// static `fault_class` label and always classifies
/// [`AutonomousRetryClass::ManualInterventionRequired`].
///
/// [`UnclassifiedFailClosed`]: AutonomousCoordinatorReason::UnclassifiedFailClosed
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AutonomousCoordinatorReason {
    AwaitingPreopen,
    AwaitingSessionOpen,
    PublicationGraceActive,
    LatestCompletedBarPending,
    ArmPending,

    WsReconnecting,
    ProviderTemporarilyUnavailable,
    TemporaryDatabaseOperationFailure,
    RuntimeEndedWithoutHalt,

    IntegrityHalted,
    KillSwitchActive,
    DurableArmDisarmed,
    WsGapDetected,
    ReconcileDirty,
    ReconcileStale,
    AssignmentMissing,
    StrategyBindingMismatch,
    SymbolBindingMismatch,
    TimeframeBindingMismatch,
    PromotionNotActive,
    ProviderRegistryInvalid,
    InstrumentRegistryInvalid,
    ProviderIdentityMismatch,
    ProviderTimestampConventionUnverified,
    UnsupportedTimeframe,
    ReadinessEvidencePersistFailed,
    ReadinessRunLinkPersistFailed,
    RiskConfigurationInvalid,
    OperationIdentityConflict,
    DatabaseNotConfiguredOrInvalid,
    NativeStrategyBootstrapMissing,
    NativeStrategyBootstrapDormant,
    NativeStrategyBootstrapFailed,
    DurableActiveRunWithoutLocalOwner,
    RuntimeRunIdMismatch,
    DispatchClaimUnresolved,
    ObservedBarEvidenceInconsistent,

    SessionClosed,
    OperationCompleted,
    NonTradingDay,

    /// Conservative fail-closed fallback for a static `fault_class` the
    /// source audit could not yet assign a more precise variant. Always
    /// classifies as [`AutonomousRetryClass::ManualInterventionRequired`] —
    /// never transient, never wait-for-condition, never terminal.
    UnclassifiedFailClosed {
        fault_class: &'static str,
    },
}

// ---------------------------------------------------------------------------
// D1.3 — Retry class + exhaustive classifier
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousRetryClass {
    WaitForCondition,
    RetryableTransient,
    ManualInterventionRequired,
    SessionTerminal,
}

// NO-STRING-AUTHORITY-GUARD-SCOPE-BEGIN
//
// Everything from here to the matching END marker is the D1.8-guarded
// classification surface: it must never render an error to a string (no
// stringifying format macro on the error value) and must never rely on
// substring/prefix/regex matching to select a retry class. Only exact
// `match` against typed variants or static string constants is permitted.
// See `no_string_authority_guard_*` in the test module below, which scans
// this exact scope for the literal forbidden patterns.

/// Exhaustive: every [`AutonomousCoordinatorReason`] variant is listed in
/// exactly one arm below — the compiler rejects an unmatched variant, so no
/// reason can silently fall through unclassified.
pub fn classify_autonomous_reason(reason: &AutonomousCoordinatorReason) -> AutonomousRetryClass {
    use AutonomousCoordinatorReason::*;
    match reason {
        AwaitingPreopen
        | AwaitingSessionOpen
        | PublicationGraceActive
        | LatestCompletedBarPending
        | ArmPending => AutonomousRetryClass::WaitForCondition,

        WsReconnecting
        | ProviderTemporarilyUnavailable
        | TemporaryDatabaseOperationFailure
        | RuntimeEndedWithoutHalt => AutonomousRetryClass::RetryableTransient,

        IntegrityHalted
        | KillSwitchActive
        | DurableArmDisarmed
        | WsGapDetected
        | ReconcileDirty
        | ReconcileStale
        | AssignmentMissing
        | StrategyBindingMismatch
        | SymbolBindingMismatch
        | TimeframeBindingMismatch
        | PromotionNotActive
        | ProviderRegistryInvalid
        | InstrumentRegistryInvalid
        | ProviderIdentityMismatch
        | ProviderTimestampConventionUnverified
        | UnsupportedTimeframe
        | ReadinessEvidencePersistFailed
        | ReadinessRunLinkPersistFailed
        | RiskConfigurationInvalid
        | OperationIdentityConflict
        | DatabaseNotConfiguredOrInvalid
        | NativeStrategyBootstrapMissing
        | NativeStrategyBootstrapDormant
        | NativeStrategyBootstrapFailed
        | DurableActiveRunWithoutLocalOwner
        | RuntimeRunIdMismatch
        | DispatchClaimUnresolved
        | ObservedBarEvidenceInconsistent
        | UnclassifiedFailClosed { .. } => AutonomousRetryClass::ManualInterventionRequired,

        SessionClosed | OperationCompleted | NonTradingDay => AutonomousRetryClass::SessionTerminal,
    }
}

// ---------------------------------------------------------------------------
// D1.4 — Exact lifecycle-error -> coordinator-reason conversion
// ---------------------------------------------------------------------------

/// Convert a `RuntimeLifecycleError` from the canonical
/// `AppState::start_execution_runtime` gate chain into a typed coordinator
/// reason.
///
/// Matches on the `RuntimeLifecycleError` variant *first*, then on
/// `fault_class()` by exact string equality against the source-audited
/// constants documented in this module's classification table. Never
/// renders `error` through a stringifying format macro, never relies on
/// substring/prefix/regex matching. Every fault class not explicitly listed
/// for its variant fails closed to
/// [`AutonomousCoordinatorReason::UnclassifiedFailClosed`].
///
/// D1D1-POLICY-CLOSURE-REPAIR-01: variant-sensitive by design. The exact
/// static string `"runtime.start_refused.service_unavailable"` is produced
/// by two source sites with different meanings — `db_pool()` (no runtime DB
/// configured at all, surfaced as `ServiceUnavailable`) and
/// `start_runtime_effects`'s `orchestrator.tick()` (a DB-backed runtime
/// leadership lease temporarily unavailable against an already-configured
/// DB, surfaced as `Conflict` via `RuntimeStartEffectsErrorKind::Conflict`).
/// A combined match arm across `ServiceUnavailable | Forbidden | Conflict`
/// would have classified both identically; matching per-variant first keeps
/// them distinct without ever parsing the rendered message for
/// `"RUNTIME_LEASE"` (that parsing already happens once, upstream, in
/// `start_runtime_effects` itself — this function only ever inspects the
/// already-typed variant and the static `fault_class` string).
pub fn coordinator_reason_from_runtime_lifecycle_error(
    error: &RuntimeLifecycleError,
) -> AutonomousCoordinatorReason {
    match error {
        RuntimeLifecycleError::Internal { fault_class, .. } => match *fault_class {
            // D1D1-POLICY-CLOSURE-REPAIR-01: every exact `Internal` context
            // label proven, by source audit, to represent a DB operation
            // that failed after a valid DB handle already existed (never a
            // missing-configuration case, which is `ServiceUnavailable`
            // from `db_pool()` instead). `"start initial tick failed"` is
            // deliberately excluded — it can wrap orchestrator, broker,
            // continuity, risk, or other runtime failure and is not proven
            // transient from the static label alone.
            "start active-run lookup failed"
            | "start latest-run lookup failed"
            | "start insert_run failed"
            | "start strategy_registry lookup failed"
            | "start arm_run failed"
            | "start begin_run failed"
            | "start initial heartbeat failed"
            | "post-initial-tick heartbeat refresh failed" => {
                AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
            }
            other => AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class: other },
        },
        RuntimeLifecycleError::ServiceUnavailable { fault_class, .. } => match *fault_class {
            "runtime.start_refused.service_unavailable" => {
                AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid
            }
            "runtime.start_refused.readiness_evidence_persist_failed" => {
                AutonomousCoordinatorReason::ReadinessEvidencePersistFailed
            }
            "runtime.start_refused.readiness_run_link_persist_failed" => {
                AutonomousCoordinatorReason::ReadinessRunLinkPersistFailed
            }
            other => AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class: other },
        },
        RuntimeLifecycleError::Forbidden { fault_class, .. } => match *fault_class {
            "runtime.control_refusal.integrity_disarmed" => {
                AutonomousCoordinatorReason::IntegrityHalted
            }
            "runtime.start_refused.reconcile_dirty" => AutonomousCoordinatorReason::ReconcileDirty,
            "runtime.start_refused.native_strategy_bootstrap_failed" => {
                AutonomousCoordinatorReason::NativeStrategyBootstrapFailed
            }
            "runtime.start_refused.strategy_bootstrap_dormant" => {
                AutonomousCoordinatorReason::NativeStrategyBootstrapDormant
            }
            // D1D1-POLICY-CLOSURE-REPAIR-01: `"runtime.start_refused.service_unavailable"`
            // is not currently produced as `Forbidden` by any audited source
            // site. No case for it here — the catchall below fails closed
            // to `UnclassifiedFailClosed` rather than guessing it means the
            // same thing as the `ServiceUnavailable` or `Conflict` cases.
            other => AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class: other },
        },
        RuntimeLifecycleError::Conflict { fault_class, .. } => match *fault_class {
            "runtime.truth_mismatch.durable_active_without_local_owner" => {
                AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner
            }
            // D1D1-POLICY-CLOSURE-REPAIR-01: this exact fault class, when
            // surfaced as `Conflict`, is produced only by
            // `start_runtime_effects`'s `orchestrator.tick()` runtime-lease
            // branch — an operational condition against an already-
            // configured DB, not missing DB configuration. Accepted as
            // transient because the distinction comes exclusively from the
            // `RuntimeLifecycleError` variant, never from parsing the
            // message for `"RUNTIME_LEASE"`.
            "runtime.start_refused.service_unavailable" => {
                AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
            }
            other => AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class: other },
        },
    }
}

// ---------------------------------------------------------------------------
// D1.5 — Typed fact adapters
// ---------------------------------------------------------------------------

/// Map an already-typed Alpaca WS continuity fact to a coordinator reason,
/// or `None` when the state is not itself a blocker (`NotApplicable`/`Live`).
///
/// `ColdStartUnproven` never masquerades as `WsReconnecting`: no fact in the
/// current [`AlpacaWsContinuityState`] enum proves an active reconnect is in
/// progress, so an unproven cold start conservatively fails closed rather
/// than guessing. `GapDetected` is the one case the closed reason set names
/// precisely (`broker_rules.md`: terminal for the session, never inferred
/// from message text).
pub fn coordinator_reason_from_ws_continuity(
    state: &AlpacaWsContinuityState,
) -> Option<AutonomousCoordinatorReason> {
    match state {
        AlpacaWsContinuityState::NotApplicable | AlpacaWsContinuityState::Live { .. } => None,
        AlpacaWsContinuityState::ColdStartUnproven => {
            Some(AutonomousCoordinatorReason::UnclassifiedFailClosed {
                fault_class: "alpaca_ws_continuity_cold_start_unproven",
            })
        }
        AlpacaWsContinuityState::GapDetected { .. } => {
            Some(AutonomousCoordinatorReason::WsGapDetected)
        }
    }
}

/// Map a resolved autonomous daily session-plan to a coordinator reason.
/// `Applicable` is never a blocker. `NotApplicable { Weekend |
/// ExchangeHoliday }` is the one case the closed reason set names precisely
/// (`NonTradingDay`, session-terminal). Every `Blocked` reason
/// (`CalendarUnavailable`, `CalendarOutOfRange`, `CalendarInvalid`,
/// `FixedWindowOverrideInvalid`, `SessionPlanInvalid`) has no precise
/// counterpart in the closed reason set today and fails closed to
/// `UnclassifiedFailClosed`, keyed on the source's own stable
/// `AutonomousDailyPlanReason::as_str()` label.
pub fn coordinator_reason_from_session_plan_resolution(
    resolution: &AutonomousDailySessionPlanResolution,
) -> Option<AutonomousCoordinatorReason> {
    match resolution {
        AutonomousDailySessionPlanResolution::Applicable(_) => None,
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            match reason_code {
                AutonomousDailyPlanReason::Weekend | AutonomousDailyPlanReason::ExchangeHoliday => {
                    Some(AutonomousCoordinatorReason::NonTradingDay)
                }
                other => Some(AutonomousCoordinatorReason::UnclassifiedFailClosed {
                    fault_class: other.as_str(),
                }),
            }
        }
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            Some(AutonomousCoordinatorReason::UnclassifiedFailClosed {
                fault_class: reason_code.as_str(),
            })
        }
    }
}

/// Map a completed-bar-driver tick outcome to a coordinator reason where the
/// mapping is unambiguous from the outcome alone. Every other outcome
/// (`PollNotDue`, `BarObserved`, `DispatchCompleted`,
/// `OutsideOperationWindow`, readiness/binding/registry blockers whose
/// detail lives only in a `Vec` of blocker strings, ...) returns `None`
/// rather than guessing — a context-dependent value is left unclassified
/// here; classifying those precisely requires the daily coordinator's own
/// operation-state context, out of D1 scope.
pub fn coordinator_reason_from_completed_bar_driver_outcome(
    outcome: &AutonomousCompletedBarDriverOutcome,
) -> Option<AutonomousCoordinatorReason> {
    match outcome {
        AutonomousCompletedBarDriverOutcome::PollFailedTransient { .. } => {
            Some(AutonomousCoordinatorReason::ProviderTemporarilyUnavailable)
        }
        AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved { status: _ } => {
            Some(AutonomousCoordinatorReason::DispatchClaimUnresolved)
        }
        AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent { .. } => {
            Some(AutonomousCoordinatorReason::ObservedBarEvidenceInconsistent)
        }
        AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady { reason_code } => {
            match *reason_code {
                REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING => {
                    Some(AutonomousCoordinatorReason::NativeStrategyBootstrapMissing)
                }
                REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT => {
                    Some(AutonomousCoordinatorReason::NativeStrategyBootstrapDormant)
                }
                REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED => {
                    Some(AutonomousCoordinatorReason::NativeStrategyBootstrapFailed)
                }
                REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH => {
                    Some(AutonomousCoordinatorReason::RuntimeRunIdMismatch)
                }
                REASON_LOCAL_RUNTIME_NOT_ACTIVE => {
                    Some(AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Map a durable daily-operation create/recover outcome to a coordinator
/// reason. `Created`/`Recovered` are never blockers; `IdentityConflict` maps
/// exactly to `OperationIdentityConflict` (§13 Correction 2 of the binding
/// contract).
pub fn coordinator_reason_from_create_or_recover_outcome(
    outcome: &CreateOrRecoverAutonomousDailyOperationOutcome,
) -> Option<AutonomousCoordinatorReason> {
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(_)
        | CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(_) => None,
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
            Some(AutonomousCoordinatorReason::OperationIdentityConflict)
        }
    }
}

/// D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 3: map an already-typed
/// runtime-dispatch eligibility fact
/// ([`AutonomousStrategyDispatchRuntimeTruth`], reported by
/// `AppState::autonomous_strategy_dispatch_runtime_truth`) to a coordinator
/// reason, or `None` when the truth proves the expected run is actively and
/// correctly owned.
///
/// This adapter is intended only for a caller that already holds a durable
/// `running` operation with a known, required `run_id` — the same
/// precondition `prove_running_dispatch_eligibility` in
/// `autonomous_completed_bar_driver.rs` establishes before calling
/// `autonomous_strategy_dispatch_runtime_truth` at all. A pre-runtime caller
/// (no operation yet, or an operation not yet `running`) must not invoke
/// this adapter — `AutonomousStrategyDispatchRuntimeTruth` carries no
/// operation-state fact of its own, so this function never infers or
/// re-derives operation state; it only classifies the already-resolved
/// runtime-ownership truth against the caller-supplied `expected_run_id`.
///
/// Typed match only: no debug-string rendering, no reason-string parsing.
pub fn coordinator_reason_from_strategy_dispatch_runtime_truth(
    truth: &AutonomousStrategyDispatchRuntimeTruth,
    expected_run_id: uuid::Uuid,
) -> Option<AutonomousCoordinatorReason> {
    match truth {
        AutonomousStrategyDispatchRuntimeTruth::Active { run_id } if *run_id == expected_run_id => {
            None
        }
        AutonomousStrategyDispatchRuntimeTruth::Active { .. } => {
            Some(AutonomousCoordinatorReason::RuntimeRunIdMismatch)
        }
        AutonomousStrategyDispatchRuntimeTruth::NoLocallyOwnedRun => {
            Some(AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner)
        }
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapMissing => {
            Some(AutonomousCoordinatorReason::NativeStrategyBootstrapMissing)
        }
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapDormant => {
            Some(AutonomousCoordinatorReason::NativeStrategyBootstrapDormant)
        }
        AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapFailed => {
            Some(AutonomousCoordinatorReason::NativeStrategyBootstrapFailed)
        }
    }
}

// NO-STRING-AUTHORITY-GUARD-SCOPE-END

// ---------------------------------------------------------------------------
// D1.6 — Deterministic blocker signature
// ---------------------------------------------------------------------------

/// Deterministic, bounded identity for one blocker occurrence. Same typed
/// reason + same stable identity inputs -> same signature; a changed reason
/// or a changed stable identity input -> a different signature. Free-form
/// error detail, timestamps, secrets, and full environment/filesystem error
/// text never enter the signature.
///
/// D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 4: `stable_context`, when present,
/// is always exactly [`BLOCKER_SIGNATURE_RENDERED_LEN`] ASCII bytes
/// (`"sha256:"` plus 64 lowercase hex chars) — a SHA-256 digest over a
/// versioned, tagged, length-delimited canonical encoding of every present
/// identity field (see [`blocker_signature`]), never the raw concatenated
/// identity strings. This replaces the prior design (`|`-joined raw values,
/// truncated at a fixed character cap), which could let a long earlier
/// field silently absorb or mask a changed later field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AutonomousBlockerSignature {
    pub reason_code: &'static str,
    pub stable_context: Option<String>,
}

/// Stable identity inputs a signature may bind. All fields are optional
/// because not every reason has every identity available (e.g.
/// `NonTradingDay` has no operation yet). Only present fields contribute to
/// `stable_context`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutonomousBlockerIdentity<'a> {
    pub operation_id: Option<uuid::Uuid>,
    pub assignment_identity: Option<&'a str>,
    pub runtime_binding_identity: Option<&'a str>,
    pub provider_id: Option<&'a str>,
    pub symbol: Option<&'a str>,
    pub timeframe: Option<&'a str>,
}

/// Canonical-encoding version tag. Bumping this (e.g. to `v3`) on any future
/// change to field order, field set, or encoding scheme guarantees old and
/// new signatures never collide by construction.
const BLOCKER_SIGNATURE_CANONICAL_VERSION: &str = "mqk.autonomous-blocker-signature.v2";

/// Fixed rendered length of a present `stable_context`: `"sha256:"` (7
/// bytes) plus 64 lowercase hex digits.
pub const BLOCKER_SIGNATURE_RENDERED_LEN: usize = 7 + 64;

/// Append one length-delimited canonical field to `canonical`. The
/// `<byte-length>:` prefix makes field boundaries unambiguous — a value
/// containing the field separator, another field's name, or any other
/// adversarial content cannot be confused with a boundary, and a changed
/// value that happens to be a prefix/suffix of another cannot produce the
/// same canonical bytes (proven by the differing length prefix or the
/// differing byte immediately following the declared length).
fn append_canonical_field(canonical: &mut String, field_name: &str, value: &str) {
    canonical.push_str(field_name);
    canonical.push('=');
    canonical.push_str(&value.len().to_string());
    canonical.push(':');
    canonical.push_str(value);
    canonical.push('\n');
}

/// D1.6 (D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 4): compute a deterministic,
/// bounded, collision-resistant blocker signature for one typed reason plus
/// its stable identity inputs.
///
/// Every supplied identity field contributes its full value (never
/// truncated) to a versioned, tagged, length-delimited canonical byte
/// string, which is then hashed with SHA-256. The rendered
/// `stable_context` — `sha256:<64 lowercase hex chars>` — never exposes the
/// raw identity text, carries no timestamp, no random UUID, no free-form
/// error detail, no environment dump, and no secret-bearing field (the
/// bounded [`AutonomousBlockerIdentity`] struct has no field for any of
/// those). `reason_code` remains a separate field on
/// [`AutonomousBlockerSignature`], so a changed reason still changes the
/// overall signature even when the identity context is identical.
pub fn blocker_signature(
    reason: &AutonomousCoordinatorReason,
    identity: &AutonomousBlockerIdentity<'_>,
) -> AutonomousBlockerSignature {
    let reason_code = reason_code_str(reason);

    let mut canonical = String::new();
    canonical.push_str(BLOCKER_SIGNATURE_CANONICAL_VERSION);
    canonical.push('\n');

    let mut any_field_present = false;
    if let Some(operation_id) = identity.operation_id {
        append_canonical_field(&mut canonical, "operation_id", &operation_id.to_string());
        any_field_present = true;
    }
    if let Some(v) = identity.assignment_identity {
        append_canonical_field(&mut canonical, "assignment_identity", v);
        any_field_present = true;
    }
    if let Some(v) = identity.runtime_binding_identity {
        append_canonical_field(&mut canonical, "runtime_binding_identity", v);
        any_field_present = true;
    }
    if let Some(v) = identity.provider_id {
        append_canonical_field(&mut canonical, "provider_id", v);
        any_field_present = true;
    }
    if let Some(v) = identity.symbol {
        append_canonical_field(&mut canonical, "symbol", v);
        any_field_present = true;
    }
    if let Some(v) = identity.timeframe {
        append_canonical_field(&mut canonical, "timeframe", v);
        any_field_present = true;
    }

    let stable_context = if any_field_present {
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        Some(format!("sha256:{}", hex::encode(hasher.finalize())))
    } else {
        None
    };

    AutonomousBlockerSignature {
        reason_code,
        stable_context,
    }
}

/// Stable string label for each coordinator reason, used only as the
/// signature's `reason_code` — never parsed back, never used to select a
/// retry class (that is [`classify_autonomous_reason`]'s exclusive job).
fn reason_code_str(reason: &AutonomousCoordinatorReason) -> &'static str {
    use AutonomousCoordinatorReason::*;
    match reason {
        AwaitingPreopen => "awaiting_preopen",
        AwaitingSessionOpen => "awaiting_session_open",
        PublicationGraceActive => "publication_grace_active",
        LatestCompletedBarPending => "latest_completed_bar_pending",
        ArmPending => "arm_pending",
        WsReconnecting => "ws_reconnecting",
        ProviderTemporarilyUnavailable => "provider_temporarily_unavailable",
        TemporaryDatabaseOperationFailure => "temporary_database_operation_failure",
        RuntimeEndedWithoutHalt => "runtime_ended_without_halt",
        IntegrityHalted => "integrity_halted",
        KillSwitchActive => "kill_switch_active",
        DurableArmDisarmed => "durable_arm_disarmed",
        WsGapDetected => "ws_gap_detected",
        ReconcileDirty => "reconcile_dirty",
        ReconcileStale => "reconcile_stale",
        AssignmentMissing => "assignment_missing",
        StrategyBindingMismatch => "strategy_binding_mismatch",
        SymbolBindingMismatch => "symbol_binding_mismatch",
        TimeframeBindingMismatch => "timeframe_binding_mismatch",
        PromotionNotActive => "promotion_not_active",
        ProviderRegistryInvalid => "provider_registry_invalid",
        InstrumentRegistryInvalid => "instrument_registry_invalid",
        ProviderIdentityMismatch => "provider_identity_mismatch",
        ProviderTimestampConventionUnverified => "provider_timestamp_convention_unverified",
        UnsupportedTimeframe => "unsupported_timeframe",
        ReadinessEvidencePersistFailed => "readiness_evidence_persist_failed",
        ReadinessRunLinkPersistFailed => "readiness_run_link_persist_failed",
        RiskConfigurationInvalid => "risk_configuration_invalid",
        OperationIdentityConflict => "operation_identity_conflict",
        DatabaseNotConfiguredOrInvalid => "database_not_configured_or_invalid",
        NativeStrategyBootstrapMissing => "native_strategy_bootstrap_missing",
        NativeStrategyBootstrapDormant => "native_strategy_bootstrap_dormant",
        NativeStrategyBootstrapFailed => "native_strategy_bootstrap_failed",
        DurableActiveRunWithoutLocalOwner => "durable_active_run_without_local_owner",
        RuntimeRunIdMismatch => "runtime_run_id_mismatch",
        DispatchClaimUnresolved => "dispatch_claim_unresolved",
        ObservedBarEvidenceInconsistent => "observed_bar_evidence_inconsistent",
        SessionClosed => "session_closed",
        OperationCompleted => "operation_completed",
        NonTradingDay => "non_trading_day",
        UnclassifiedFailClosed { fault_class } => fault_class,
    }
}

// ---------------------------------------------------------------------------
// D1.7 — Bounded backoff
// ---------------------------------------------------------------------------

/// 30s / 60s / 120s / 300s-cap schedule, frozen by
/// `docs/specs/autonomous_daily_paper_operations_01a_current_truth_and_contract.md`
/// §15. `attempt_number` is one-based: attempt 1 is the delay before the
/// first retry after an initial refusal. `0` is normalized to `1` — this
/// function never produces an unbounded or zero-delay loop.
pub fn retry_delay_for_attempt(attempt_number: u64) -> std::time::Duration {
    let attempt = attempt_number.max(1);
    let secs = match attempt {
        1 => 30,
        2 => 60,
        3 => 120,
        _ => 300,
    };
    std::time::Duration::from_secs(secs)
}

/// Compute the next retry instant from an injected `now_utc` — never reads
/// the wall clock itself.
pub fn next_retry_at(now_utc: DateTime<Utc>, attempt_number: u64) -> DateTime<Utc> {
    let delay = retry_delay_for_attempt(attempt_number);
    now_utc
        + ChronoDuration::from_std(delay)
            .expect("retry_delay_for_attempt's bound (<=300s) always fits in chrono::Duration")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // -----------------------------------------------------------------
    // Retry class exhaustiveness (proof group: retry classes, #11-#16)
    // -----------------------------------------------------------------

    fn all_reasons_by_class() -> (
        Vec<AutonomousCoordinatorReason>,
        Vec<AutonomousCoordinatorReason>,
        Vec<AutonomousCoordinatorReason>,
        Vec<AutonomousCoordinatorReason>,
    ) {
        use AutonomousCoordinatorReason::*;
        let wait = vec![
            AwaitingPreopen,
            AwaitingSessionOpen,
            PublicationGraceActive,
            LatestCompletedBarPending,
            ArmPending,
        ];
        let transient = vec![
            WsReconnecting,
            ProviderTemporarilyUnavailable,
            TemporaryDatabaseOperationFailure,
            RuntimeEndedWithoutHalt,
        ];
        let manual = vec![
            IntegrityHalted,
            KillSwitchActive,
            DurableArmDisarmed,
            WsGapDetected,
            ReconcileDirty,
            ReconcileStale,
            AssignmentMissing,
            StrategyBindingMismatch,
            SymbolBindingMismatch,
            TimeframeBindingMismatch,
            PromotionNotActive,
            ProviderRegistryInvalid,
            InstrumentRegistryInvalid,
            ProviderIdentityMismatch,
            ProviderTimestampConventionUnverified,
            UnsupportedTimeframe,
            ReadinessEvidencePersistFailed,
            ReadinessRunLinkPersistFailed,
            RiskConfigurationInvalid,
            OperationIdentityConflict,
            DatabaseNotConfiguredOrInvalid,
            NativeStrategyBootstrapMissing,
            NativeStrategyBootstrapDormant,
            NativeStrategyBootstrapFailed,
            DurableActiveRunWithoutLocalOwner,
            RuntimeRunIdMismatch,
            DispatchClaimUnresolved,
            ObservedBarEvidenceInconsistent,
            UnclassifiedFailClosed {
                fault_class: "some_unmapped_fault_class",
            },
        ];
        let terminal = vec![SessionClosed, OperationCompleted, NonTradingDay];
        (wait, transient, manual, terminal)
    }

    #[test]
    fn every_wait_reason_maps_to_wait_for_condition() {
        let (wait, _, _, _) = all_reasons_by_class();
        for reason in wait {
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::WaitForCondition,
                "{reason:?} must classify as WaitForCondition"
            );
        }
    }

    #[test]
    fn every_transient_reason_maps_to_retryable_transient() {
        let (_, transient, _, _) = all_reasons_by_class();
        for reason in transient {
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::RetryableTransient,
                "{reason:?} must classify as RetryableTransient"
            );
        }
    }

    #[test]
    fn every_manual_reason_maps_to_manual_intervention_required() {
        let (_, _, manual, _) = all_reasons_by_class();
        for reason in manual {
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::ManualInterventionRequired,
                "{reason:?} must classify as ManualInterventionRequired"
            );
        }
    }

    #[test]
    fn every_terminal_reason_maps_to_session_terminal() {
        let (_, _, _, terminal) = all_reasons_by_class();
        for reason in terminal {
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::SessionTerminal,
                "{reason:?} must classify as SessionTerminal"
            );
        }
    }

    #[test]
    fn no_reason_is_omitted_from_the_classifier() {
        // classify_autonomous_reason is a total (non-wildcard) match over
        // AutonomousCoordinatorReason: this test asserts the sum of the four
        // per-class lists above covers every constructible reason the
        // fixtures below exercise, and that none panics.
        let (wait, transient, manual, terminal) = all_reasons_by_class();
        let total = wait.len() + transient.len() + manual.len() + terminal.len();
        assert!(
            total >= 39,
            "expected at least 39 named reason instances, got {total}"
        );
        for reason in wait
            .into_iter()
            .chain(transient)
            .chain(manual)
            .chain(terminal)
        {
            let _ = classify_autonomous_reason(&reason);
        }
    }

    #[test]
    fn conservative_fallback_maps_to_manual() {
        let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
            fault_class: "anything_unmapped",
        };
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    // -----------------------------------------------------------------
    // Runtime lifecycle-error conversion (proof group #17-#28)
    // -----------------------------------------------------------------

    #[test]
    fn integrity_disarmed_maps_manual() {
        let err = RuntimeLifecycleError::forbidden(
            "runtime.control_refusal.integrity_disarmed",
            "integrity_armed",
            "GATE_REFUSED: integrity disarmed or halted; arm integrity first",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::IntegrityHalted
        );
        assert_eq!(
            classify_autonomous_reason(&coordinator_reason_from_runtime_lifecycle_error(&err)),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn reconcile_dirty_maps_manual() {
        let err = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.reconcile_dirty",
            "reconcile_truth",
            "dirty",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::ReconcileDirty
        );
    }

    #[test]
    fn fixed_window_invalid_maps_manual_via_conservative_fallback() {
        let err = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.fixed_window_override_invalid",
            "fixed_window_override",
            "invalid",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn daily_readiness_blocked_maps_manual_via_conservative_fallback() {
        let err = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.daily_data_readiness_blocked",
            "daily_data_readiness",
            "evaluation_id=... start_allowed=false top_level_blocker=...",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn readiness_evidence_persist_failed_maps_manual() {
        let err = RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.readiness_evidence_persist_failed",
            "persist failed",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::ReadinessEvidencePersistFailed
        );
    }

    #[test]
    fn readiness_run_link_persist_failed_maps_manual() {
        let err = RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.readiness_run_link_persist_failed",
            "link persist failed",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::ReadinessRunLinkPersistFailed
        );
    }

    #[test]
    fn strategy_registry_missing_or_disabled_maps_manual_via_conservative_fallback() {
        for fault_class in [
            "runtime.start_refused.strategy_registry_missing",
            "runtime.start_refused.strategy_registry_disabled",
        ] {
            let err = RuntimeLifecycleError::forbidden(fault_class, "strategy_registry", "x");
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            assert!(
                matches!(reason, AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }),
                "{fault_class} must fail closed to the conservative fallback, not a guessed exact variant"
            );
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::ManualInterventionRequired
            );
        }
    }

    #[test]
    fn durable_active_without_local_owner_maps_manual() {
        let err = RuntimeLifecycleError::conflict(
            "runtime.truth_mismatch.durable_active_without_local_owner",
            "durable active run exists without local ownership",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner
        );
    }

    #[test]
    fn every_currently_emitted_start_path_fault_class_is_represented() {
        // Exact fault classes audited on AppState::start_execution_runtime's
        // reachable path (lifecycle.rs + create_or_reuse_run_for_start +
        // db_pool). Every one must classify without panicking, and every
        // gate-specific class not given an exact mapping above must fail
        // closed to the conservative fallback (never silently pass as
        // transient or wait-for-condition).
        let forbidden_and_conflict_fault_classes = [
            "runtime.start_refused.deployment_mode_unproven",
            "runtime.start_refused.capital_requires_operator_token",
            "runtime.control_refusal.already_owned",
            "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
            "runtime.start_refused.capital_ws_continuity_unproven",
            "runtime.start_refused.artifact_not_deployable",
            "runtime.start_refused.artifact_intake_invalid",
            "runtime.start_refused.artifact_intake_unavailable",
            "runtime.start_refused.parity_evidence_artifact_mismatch",
            "runtime.start_refused.parity_evidence_not_present",
            "runtime.start_refused.live_capital_requires_capital_policy",
            "runtime.start_refused.capital_policy_not_authorized",
            "runtime.start_refused.deployment_economics_not_specified",
            "runtime.start_refused.fixed_window_override_invalid",
            "runtime.start_refused.daily_data_readiness_blocked",
            "runtime.start_refused.market_data_not_fresh",
            "runtime.start_refused.halted_lifecycle",
            "runtime.start_refused.durable_run_active",
        ];
        for fault_class in forbidden_and_conflict_fault_classes {
            let err = RuntimeLifecycleError::forbidden(fault_class, "gate", "message");
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::ManualInterventionRequired,
                "{fault_class} must fail closed to Manual, not silently pass through"
            );
        }
    }

    #[test]
    fn unknown_static_fault_class_maps_manual() {
        let err = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.some_future_gate_not_yet_audited",
            "gate",
            "message",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed {
                fault_class: "runtime.start_refused.some_future_gate_not_yet_audited"
            }
        );
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn human_readable_message_changes_do_not_change_classification() {
        let err_a = RuntimeLifecycleError::forbidden(
            "runtime.control_refusal.integrity_disarmed",
            "integrity_armed",
            "message text A, completely different wording",
        );
        let err_b = RuntimeLifecycleError::forbidden(
            "runtime.control_refusal.integrity_disarmed",
            "integrity_armed",
            "an entirely different message B with different length and content",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err_a),
            coordinator_reason_from_runtime_lifecycle_error(&err_b),
        );
    }

    #[test]
    fn temporary_db_operation_fault_maps_transient_only_for_operational_not_configuration_failure()
    {
        // Operational: a DB query/insert failed after the DB was already
        // configured and reachable (Internal variant, all eight audited
        // context labels reachable from create_or_reuse_run_for_start and
        // ProductionRuntimeStartEffects::start_runtime_effects).
        for context in [
            "start active-run lookup failed",
            "start latest-run lookup failed",
            "start insert_run failed",
            "start strategy_registry lookup failed",
            "start arm_run failed",
            "start begin_run failed",
            "start initial heartbeat failed",
            "post-initial-tick heartbeat refresh failed",
        ] {
            let err = RuntimeLifecycleError::internal(context, "sqlx error: connection reset");
            assert_eq!(
                coordinator_reason_from_runtime_lifecycle_error(&err),
                AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure,
                "{context} must map to TemporaryDatabaseOperationFailure"
            );
        }

        // Configuration: DB not configured at all (db_pool()'s ServiceUnavailable).
        let config_err = RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.service_unavailable",
            "runtime DB is not configured on this daemon",
        );
        let config_reason = coordinator_reason_from_runtime_lifecycle_error(&config_err);
        assert_eq!(
            config_reason,
            AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid
        );
        assert_ne!(
            config_reason,
            AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure,
            "a configuration failure must never be classified the same as an operational one"
        );
        assert_eq!(
            classify_autonomous_reason(&config_reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
        assert_eq!(
            classify_autonomous_reason(
                &AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
            ),
            AutonomousRetryClass::RetryableTransient
        );
    }

    #[test]
    fn start_initial_tick_failed_remains_manual() {
        // REPAIR 2: unlike the eight DB-operation labels above, this
        // Internal context can wrap orchestrator, broker, continuity, risk,
        // or other runtime failure — never proven transient from the static
        // label alone.
        let err = RuntimeLifecycleError::internal(
            "start initial tick failed",
            "orchestrator error: some non-DB failure",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn unknown_internal_label_remains_manual() {
        let err =
            RuntimeLifecycleError::internal("some future internal context never audited", "detail");
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn near_match_internal_label_remains_manual() {
        // Exact-string equality only — a near-match to an audited transient
        // label must never be treated as that label.
        let err = RuntimeLifecycleError::internal("start arm_run failed extra", "detail");
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    // -----------------------------------------------------------------
    // D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 1: variant-sensitive mapping of
    // the exact same static fault_class string
    // -----------------------------------------------------------------

    #[test]
    fn service_unavailable_variant_service_unavailable_fault_class_maps_db_not_configured() {
        let err = RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.service_unavailable",
            "runtime DB is not configured on this daemon",
        );
        assert_eq!(
            coordinator_reason_from_runtime_lifecycle_error(&err),
            AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid
        );
    }

    #[test]
    fn conflict_variant_service_unavailable_fault_class_maps_temporary_db_operation_failure() {
        let err = RuntimeLifecycleError::conflict(
            "runtime.start_refused.service_unavailable",
            "runtime leader lease unavailable: RUNTIME_LEASE could not be acquired",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
        );
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::RetryableTransient
        );
    }

    #[test]
    fn forbidden_variant_service_unavailable_fault_class_fails_closed_to_manual() {
        // Not currently reachable as Forbidden by any audited source site —
        // must fail closed, never reuse the ServiceUnavailable or Conflict
        // mapping for the same string.
        let err = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.service_unavailable",
            "some_gate",
            "message",
        );
        let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
        assert!(matches!(
            reason,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn variant_sensitive_mapping_is_not_confused_by_message_wording() {
        let service_unavailable = RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.service_unavailable",
            "message A, completely different wording",
        );
        let conflict = RuntimeLifecycleError::conflict(
            "runtime.start_refused.service_unavailable",
            "an entirely different message B with different length and content",
        );
        let forbidden = RuntimeLifecycleError::forbidden(
            "runtime.start_refused.service_unavailable",
            "gate",
            "yet another wording C",
        );
        let a = coordinator_reason_from_runtime_lifecycle_error(&service_unavailable);
        let b = coordinator_reason_from_runtime_lifecycle_error(&conflict);
        let c = coordinator_reason_from_runtime_lifecycle_error(&forbidden);
        assert_eq!(
            a,
            AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid
        );
        assert_eq!(
            b,
            AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
        );
        assert!(matches!(
            c,
            AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
        ));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    // -----------------------------------------------------------------
    // D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 3: runtime-dispatch-truth adapter
    // -----------------------------------------------------------------

    #[test]
    fn dispatch_truth_active_matching_run_returns_none() {
        let run_id = uuid::Uuid::from_u128(100);
        let truth = AutonomousStrategyDispatchRuntimeTruth::Active { run_id };
        assert_eq!(
            coordinator_reason_from_strategy_dispatch_runtime_truth(&truth, run_id),
            None
        );
    }

    #[test]
    fn dispatch_truth_active_mismatched_run_maps_run_id_mismatch() {
        let truth = AutonomousStrategyDispatchRuntimeTruth::Active {
            run_id: uuid::Uuid::from_u128(1),
        };
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &truth,
            uuid::Uuid::from_u128(2),
        )
        .expect("mismatch is a blocker");
        assert_eq!(reason, AutonomousCoordinatorReason::RuntimeRunIdMismatch);
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn dispatch_truth_no_locally_owned_run_maps_durable_active_run_without_local_owner() {
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &AutonomousStrategyDispatchRuntimeTruth::NoLocallyOwnedRun,
            uuid::Uuid::from_u128(1),
        )
        .expect("no locally owned run is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner
        );
    }

    #[test]
    fn dispatch_truth_bootstrap_missing_maps_exact_reason() {
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapMissing,
            uuid::Uuid::from_u128(1),
        )
        .expect("missing bootstrap is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::NativeStrategyBootstrapMissing
        );
    }

    #[test]
    fn dispatch_truth_bootstrap_dormant_maps_exact_reason() {
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapDormant,
            uuid::Uuid::from_u128(1),
        )
        .expect("dormant bootstrap is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::NativeStrategyBootstrapDormant
        );
    }

    #[test]
    fn dispatch_truth_bootstrap_failed_maps_exact_reason() {
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapFailed,
            uuid::Uuid::from_u128(1),
        )
        .expect("failed bootstrap is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::NativeStrategyBootstrapFailed
        );
    }

    // -----------------------------------------------------------------
    // WS distinctions (proof group #29-#32)
    // -----------------------------------------------------------------

    #[test]
    fn typed_reconnecting_reason_maps_transient() {
        assert_eq!(
            classify_autonomous_reason(&AutonomousCoordinatorReason::WsReconnecting),
            AutonomousRetryClass::RetryableTransient
        );
    }

    #[test]
    fn typed_gap_detected_maps_manual() {
        let state = AlpacaWsContinuityState::GapDetected {
            last_message_id: Some("m1".to_string()),
            last_event_at: Some("2026-07-17T00:00:00Z".to_string()),
            detail: "gap".to_string(),
        };
        let reason = coordinator_reason_from_ws_continuity(&state).expect("gap is a blocker");
        assert_eq!(reason, AutonomousCoordinatorReason::WsGapDetected);
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn cold_start_unknown_does_not_masquerade_as_reconnecting() {
        let reason =
            coordinator_reason_from_ws_continuity(&AlpacaWsContinuityState::ColdStartUnproven)
                .expect("cold start is a blocker, not a pass-through");
        assert_ne!(reason, AutonomousCoordinatorReason::WsReconnecting);
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired,
            "an unproven cold start must never classify as retryable-transient"
        );
    }

    #[test]
    fn ws_continuity_not_applicable_and_live_are_not_blockers() {
        assert_eq!(
            coordinator_reason_from_ws_continuity(&AlpacaWsContinuityState::NotApplicable),
            None
        );
        assert_eq!(
            coordinator_reason_from_ws_continuity(&AlpacaWsContinuityState::Live {
                last_message_id: "m1".to_string(),
                last_event_at: "2026-07-17T00:00:00Z".to_string(),
            }),
            None
        );
    }

    // -----------------------------------------------------------------
    // Session-plan / driver / create-or-recover adapters
    // -----------------------------------------------------------------

    #[test]
    fn session_plan_not_applicable_weekend_and_holiday_map_non_trading_day() {
        for reason_code in [
            AutonomousDailyPlanReason::Weekend,
            AutonomousDailyPlanReason::ExchangeHoliday,
        ] {
            let resolution = AutonomousDailySessionPlanResolution::NotApplicable {
                market_date: "2026-07-18".to_string(),
                reason_code,
            };
            let reason = coordinator_reason_from_session_plan_resolution(&resolution)
                .expect("not-applicable is a terminal blocker");
            assert_eq!(reason, AutonomousCoordinatorReason::NonTradingDay);
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::SessionTerminal
            );
        }
    }

    #[test]
    fn session_plan_blocked_reasons_fail_closed_to_manual() {
        for reason_code in [
            AutonomousDailyPlanReason::CalendarUnavailable,
            AutonomousDailyPlanReason::CalendarOutOfRange,
            AutonomousDailyPlanReason::CalendarInvalid,
            AutonomousDailyPlanReason::FixedWindowOverrideInvalid,
            AutonomousDailyPlanReason::SessionPlanInvalid,
        ] {
            let resolution = AutonomousDailySessionPlanResolution::Blocked {
                market_date: Some("2026-07-17".to_string()),
                reason_code,
                detail: "blocked".to_string(),
            };
            let reason = coordinator_reason_from_session_plan_resolution(&resolution)
                .expect("blocked is a blocker");
            assert!(matches!(
                reason,
                AutonomousCoordinatorReason::UnclassifiedFailClosed { .. }
            ));
            assert_eq!(
                classify_autonomous_reason(&reason),
                AutonomousRetryClass::ManualInterventionRequired
            );
        }
    }

    #[test]
    fn create_or_recover_identity_conflict_maps_operation_identity_conflict() {
        let outcome = CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
            existing_operation_id: uuid::Uuid::nil(),
            expected_operation_id: uuid::Uuid::from_u128(1),
            differing_fields: vec!["assignment_identity".to_string()],
        };
        let reason = coordinator_reason_from_create_or_recover_outcome(&outcome)
            .expect("identity conflict is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::OperationIdentityConflict
        );
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn completed_bar_driver_poll_failed_transient_maps_provider_temporarily_unavailable() {
        let outcome = AutonomousCompletedBarDriverOutcome::PollFailedTransient {
            detail: "timeout".to_string(),
        };
        let reason = coordinator_reason_from_completed_bar_driver_outcome(&outcome)
            .expect("transient poll failure is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::ProviderTemporarilyUnavailable
        );
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::RetryableTransient
        );
    }

    #[test]
    fn completed_bar_driver_dispatch_claim_unresolved_maps_manual() {
        let outcome = AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
            status: "claimed".to_string(),
        };
        let reason = coordinator_reason_from_completed_bar_driver_outcome(&outcome)
            .expect("unresolved dispatch claim is a blocker");
        assert_eq!(reason, AutonomousCoordinatorReason::DispatchClaimUnresolved);
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn completed_bar_driver_observed_bar_evidence_inconsistent_maps_manual() {
        let outcome = AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
            expected_end_ts: 1_700_000_000,
            reason_code: "missing_backing_row",
        };
        let reason = coordinator_reason_from_completed_bar_driver_outcome(&outcome)
            .expect("evidence inconsistency is a blocker");
        assert_eq!(
            reason,
            AutonomousCoordinatorReason::ObservedBarEvidenceInconsistent
        );
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::ManualInterventionRequired
        );
    }

    #[test]
    fn completed_bar_driver_runtime_dispatch_not_ready_bootstrap_reasons_map_exactly() {
        let cases = [
            (
                REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
                AutonomousCoordinatorReason::NativeStrategyBootstrapMissing,
            ),
            (
                REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT,
                AutonomousCoordinatorReason::NativeStrategyBootstrapDormant,
            ),
            (
                REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED,
                AutonomousCoordinatorReason::NativeStrategyBootstrapFailed,
            ),
            (
                REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH,
                AutonomousCoordinatorReason::RuntimeRunIdMismatch,
            ),
            (
                REASON_LOCAL_RUNTIME_NOT_ACTIVE,
                AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner,
            ),
        ];
        for (reason_code, expected) in cases {
            let outcome =
                AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady { reason_code };
            let reason = coordinator_reason_from_completed_bar_driver_outcome(&outcome)
                .unwrap_or_else(|| panic!("{reason_code} must map to a coordinator reason"));
            assert_eq!(reason, expected);
        }
    }

    #[test]
    fn completed_bar_driver_context_dependent_outcomes_return_none() {
        assert_eq!(
            coordinator_reason_from_completed_bar_driver_outcome(
                &AutonomousCompletedBarDriverOutcome::OutsideOperationWindow
            ),
            None
        );
        assert_eq!(
            coordinator_reason_from_completed_bar_driver_outcome(
                &AutonomousCompletedBarDriverOutcome::PollNotDue
            ),
            None
        );
    }

    // -----------------------------------------------------------------
    // Signatures (proof group #33-#39)
    // -----------------------------------------------------------------

    #[test]
    fn identical_inputs_produce_identical_signatures() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let op_id = uuid::Uuid::from_u128(42);
        let identity = AutonomousBlockerIdentity {
            operation_id: Some(op_id),
            assignment_identity: Some("assign-1"),
            ..Default::default()
        };
        assert_eq!(
            blocker_signature(&reason, &identity),
            blocker_signature(&reason, &identity)
        );
    }

    #[test]
    fn changed_reason_changes_signature() {
        let identity = AutonomousBlockerIdentity::default();
        let sig_a = blocker_signature(&AutonomousCoordinatorReason::AssignmentMissing, &identity);
        let sig_b = blocker_signature(&AutonomousCoordinatorReason::ReconcileDirty, &identity);
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn changed_operation_id_changes_signature() {
        let reason = AutonomousCoordinatorReason::OperationIdentityConflict;
        let identity_a = AutonomousBlockerIdentity {
            operation_id: Some(uuid::Uuid::from_u128(1)),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            operation_id: Some(uuid::Uuid::from_u128(2)),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn changed_binding_identity_changes_signature_when_relevant() {
        let reason = AutonomousCoordinatorReason::StrategyBindingMismatch;
        let identity_a = AutonomousBlockerIdentity {
            runtime_binding_identity: Some("binding-a"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            runtime_binding_identity: Some("binding-b"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn changed_assignment_identity_changes_signature() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let identity_a = AutonomousBlockerIdentity {
            assignment_identity: Some("assign-a"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            assignment_identity: Some("assign-b"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn changed_provider_id_changes_signature() {
        let reason = AutonomousCoordinatorReason::ProviderRegistryInvalid;
        let identity_a = AutonomousBlockerIdentity {
            provider_id: Some("twelvedata"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            provider_id: Some("alpaca"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn changed_symbol_changes_signature() {
        let reason = AutonomousCoordinatorReason::SymbolBindingMismatch;
        let identity_a = AutonomousBlockerIdentity {
            symbol: Some("ZZSG01"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            symbol: Some("ZZSG02"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn changed_timeframe_changes_signature() {
        let reason = AutonomousCoordinatorReason::TimeframeBindingMismatch;
        let identity_a = AutonomousBlockerIdentity {
            timeframe: Some("1Min"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            timeframe: Some("5Min"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn long_assignment_identity_followed_by_different_binding_identity_still_differs() {
        // A pathologically long earlier field must never absorb or mask a
        // changed later field — the length-delimited encoding (not the
        // old truncate-at-512-chars design) proves this.
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let long = "a".repeat(10_000);
        let identity_a = AutonomousBlockerIdentity {
            assignment_identity: Some(&long),
            runtime_binding_identity: Some("binding-a"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            assignment_identity: Some(&long),
            runtime_binding_identity: Some("binding-b"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn long_assignment_identity_followed_by_different_provider_id_still_differs() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let long = "b".repeat(10_000);
        let identity_a = AutonomousBlockerIdentity {
            assignment_identity: Some(&long),
            provider_id: Some("twelvedata"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            assignment_identity: Some(&long),
            provider_id: Some("alpaca"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn canonical_field_boundaries_are_unambiguous() {
        // assignment="ab", binding="c" must differ from assignment="a",
        // binding="bc" — the length-delimited encoding makes the field
        // boundary explicit rather than inferring it from a plain
        // delimiter, which a raw "ab|c" vs "a|bc" concatenation would not
        // have distinguished if either value could itself contain "|".
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let identity_a = AutonomousBlockerIdentity {
            assignment_identity: Some("ab"),
            runtime_binding_identity: Some("c"),
            ..Default::default()
        };
        let identity_b = AutonomousBlockerIdentity {
            assignment_identity: Some("a"),
            runtime_binding_identity: Some("bc"),
            ..Default::default()
        };
        assert_ne!(
            blocker_signature(&reason, &identity_a),
            blocker_signature(&reason, &identity_b)
        );
    }

    #[test]
    fn raw_long_identity_text_is_absent_from_rendered_signature() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let long = "unmistakable-raw-identity-marker-zzsg-9f3e".repeat(50);
        let identity = AutonomousBlockerIdentity {
            assignment_identity: Some(&long),
            ..Default::default()
        };
        let sig = blocker_signature(&reason, &identity);
        let rendered = format!("{sig:?}");
        assert!(
            !rendered.contains("unmistakable-raw-identity-marker"),
            "rendered signature must never contain raw identity text"
        );
    }

    #[test]
    fn free_form_detail_changes_do_not_change_signature() {
        // The signature type carries no free-form detail field at all — the
        // constructor accepts only the bounded AutonomousBlockerIdentity, so
        // there is no way to feed a raw error-message string into it.
        // Prove that constructing from the identical typed identity twice
        // with different (discarded) call-site detail strings is still
        // deterministic.
        let reason = AutonomousCoordinatorReason::ProviderRegistryInvalid;
        let identity = AutonomousBlockerIdentity {
            provider_id: Some("twelvedata"),
            ..Default::default()
        };
        let _discarded_detail_a = "filesystem error: permission denied at /some/random/path";
        let _discarded_detail_b = "a completely different, much longer free-form error message";
        assert_eq!(
            blocker_signature(&reason, &identity),
            blocker_signature(&reason, &identity)
        );
    }

    #[test]
    fn signature_excludes_secrets() {
        // AutonomousBlockerIdentity has no field for tokens/passwords/API
        // keys at all — structurally, a secret cannot enter the signature.
        let reason = AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid;
        let identity = AutonomousBlockerIdentity::default();
        let sig = blocker_signature(&reason, &identity);
        let rendered = format!("{sig:?}");
        assert!(!rendered.to_lowercase().contains("token"));
        assert!(!rendered.to_lowercase().contains("password"));
        assert!(!rendered.to_lowercase().contains("secret"));
    }

    #[test]
    fn signature_length_is_bounded() {
        // D1D1-POLICY-CLOSURE-REPAIR-01: stable_context is now a SHA-256
        // digest render, so its length is always exactly
        // BLOCKER_SIGNATURE_RENDERED_LEN regardless of input size — bounded
        // by construction, not by truncation.
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let pathological = "x".repeat(10_000);
        let identity = AutonomousBlockerIdentity {
            assignment_identity: Some(&pathological),
            ..Default::default()
        };
        let sig = blocker_signature(&reason, &identity);
        let len = sig.stable_context.map(|s| s.chars().count()).unwrap_or(0);
        assert_eq!(
            len, BLOCKER_SIGNATURE_RENDERED_LEN,
            "stable_context must always render at the fixed documented length, got {len}"
        );
    }

    #[test]
    fn signature_length_is_fixed_for_short_identity_too() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let identity = AutonomousBlockerIdentity {
            assignment_identity: Some("x"),
            ..Default::default()
        };
        let sig = blocker_signature(&reason, &identity);
        let len = sig.stable_context.map(|s| s.chars().count()).unwrap_or(0);
        assert_eq!(len, BLOCKER_SIGNATURE_RENDERED_LEN);
    }

    #[test]
    fn stable_context_none_when_no_identity_fields_present() {
        let reason = AutonomousCoordinatorReason::NonTradingDay;
        let identity = AutonomousBlockerIdentity::default();
        let sig = blocker_signature(&reason, &identity);
        assert_eq!(sig.stable_context, None);
    }

    #[test]
    fn signature_rendered_form_matches_sha256_prefix_convention() {
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let identity = AutonomousBlockerIdentity {
            assignment_identity: Some("assign-1"),
            ..Default::default()
        };
        let sig = blocker_signature(&reason, &identity);
        let rendered = sig.stable_context.expect("identity present");
        assert!(rendered.starts_with("sha256:"));
        let hex_part = &rendered["sha256:".len()..];
        assert_eq!(hex_part.len(), 64);
        assert!(hex_part
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    // -----------------------------------------------------------------
    // Backoff (proof group #40-#46)
    // -----------------------------------------------------------------

    #[test]
    fn attempt_1_is_30_seconds() {
        assert_eq!(
            retry_delay_for_attempt(1),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn attempt_2_is_60_seconds() {
        assert_eq!(
            retry_delay_for_attempt(2),
            std::time::Duration::from_secs(60)
        );
    }

    #[test]
    fn attempt_3_is_120_seconds() {
        assert_eq!(
            retry_delay_for_attempt(3),
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn attempt_4_is_300_seconds() {
        assert_eq!(
            retry_delay_for_attempt(4),
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn attempt_50_remains_300_seconds() {
        assert_eq!(
            retry_delay_for_attempt(50),
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn zero_attempt_cannot_produce_a_rapid_retry_loop() {
        assert_eq!(
            retry_delay_for_attempt(0),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn next_retry_at_uses_the_injected_instant_exactly() {
        let now = Utc.with_ymd_and_hms(2026, 7, 17, 12, 0, 0).unwrap();
        let next = next_retry_at(now, 1);
        assert_eq!(next, now + ChronoDuration::seconds(30));

        let next3 = next_retry_at(now, 3);
        assert_eq!(next3, now + ChronoDuration::seconds(120));
    }

    // -----------------------------------------------------------------
    // No-string-authority guard (D1.8)
    // -----------------------------------------------------------------

    #[test]
    fn no_string_authority_guard_scope_contains_no_forbidden_parsing_patterns() {
        let source = include_str!("autonomous_retry_policy.rs");
        let start = source
            .find("NO-STRING-AUTHORITY-GUARD-SCOPE-BEGIN")
            .expect("guard scope start marker must exist in this file");
        let end = source
            .find("NO-STRING-AUTHORITY-GUARD-SCOPE-END")
            .expect("guard scope end marker must exist in this file");
        assert!(start < end, "guard scope markers must be in order");
        let scope = &source[start..end];
        let forbidden = [
            ".to_string()",
            ".contains(",
            ".starts_with(",
            "Regex::new",
            "regex::",
            "format!(\"{}\", error",
            "format!(\"{error}\")",
        ];
        for pattern in forbidden {
            assert!(
                !scope.contains(pattern),
                "classification scope must not use {pattern:?} to decide retry behavior"
            );
        }
    }
}
