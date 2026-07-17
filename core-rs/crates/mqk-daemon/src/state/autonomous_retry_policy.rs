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
//! ## Source-grounded fault-class classification table (D1.4)
//!
//! Every `RuntimeLifecycleError::fault_class()` string reachable from
//! `AppState::start_execution_runtime` (`state/lifecycle.rs`, including
//! `create_or_reuse_run_for_start` and `db_pool`), as audited for this
//! patch:
//!
//! | fault_class (or `Internal` context label) | source variant | coordinator reason | retry class |
//! |---|---|---|---|
//! | `runtime.start_refused.service_unavailable` (`db_pool`, DB absent) | `ServiceUnavailable` | `DatabaseNotConfiguredOrInvalid` | Manual |
//! | `start active-run lookup failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start latest-run lookup failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `start insert_run failed` | `Internal` | `TemporaryDatabaseOperationFailure` | Transient |
//! | `runtime.control_refusal.integrity_disarmed` | `Forbidden` | `IntegrityHalted` | Manual |
//! | `runtime.start_refused.reconcile_dirty` (covers both `"dirty"`/`"stale"` source status strings) | `Forbidden` | `ReconcileDirty` | Manual |
//! | `runtime.start_refused.native_strategy_bootstrap_failed` | `Forbidden` | `NativeStrategyBootstrapFailed` | Manual |
//! | `runtime.start_refused.strategy_bootstrap_dormant` | `Forbidden` | `NativeStrategyBootstrapDormant` | Manual |
//! | `runtime.start_refused.readiness_evidence_persist_failed` | `ServiceUnavailable` | `ReadinessEvidencePersistFailed` | Manual |
//! | `runtime.start_refused.readiness_run_link_persist_failed` | `ServiceUnavailable` | `ReadinessRunLinkPersistFailed` | Manual |
//! | `runtime.truth_mismatch.durable_active_without_local_owner` | `Conflict` | `DurableActiveRunWithoutLocalOwner` | Manual |
//! | every other fault class (deployment-mode, capital/artifact/parity/economics gates, WS-continuity-unproven, `daily_data_readiness_blocked`, `strategy_registry_missing`/`disabled`, `market_data_not_fresh`, `fixed_window_override_invalid`, `already_owned`, `halted_lifecycle`, `durable_run_active`, any other `Internal` context) | any | `UnclassifiedFailClosed { fault_class }` | Manual |
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
//!   `RetryableTransient` or `WaitForCondition` today except the three
//!   `Internal` DB-lookup/insert labels above — the `WaitForCondition`
//!   reasons and `WsReconnecting` are reserved for the daily coordinator's
//!   own typed facts (see the typed-fact adapters below), not yet emitted
//!   by any gate on the `start_execution_runtime` path.

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use super::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverOutcome, REASON_LOCAL_RUNTIME_NOT_ACTIVE,
    REASON_LOCAL_RUNTIME_RUN_ID_MISMATCH, REASON_NATIVE_STRATEGY_BOOTSTRAP_DORMANT,
    REASON_NATIVE_STRATEGY_BOOTSTRAP_FAILED, REASON_NATIVE_STRATEGY_BOOTSTRAP_MISSING,
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
/// Matches on the `RuntimeLifecycleError` variant, then on `fault_class()`
/// by exact string equality against the source-audited constants documented
/// in this module's classification table. Never renders `error` through a
/// stringifying format macro, never relies on substring/prefix/regex
/// matching. Every fault class not explicitly listed fails closed to
/// [`AutonomousCoordinatorReason::UnclassifiedFailClosed`].
pub fn coordinator_reason_from_runtime_lifecycle_error(
    error: &RuntimeLifecycleError,
) -> AutonomousCoordinatorReason {
    match error {
        RuntimeLifecycleError::Internal { fault_class, .. } => match *fault_class {
            "start active-run lookup failed"
            | "start latest-run lookup failed"
            | "start insert_run failed" => {
                AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
            }
            other => AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class: other },
        },
        RuntimeLifecycleError::ServiceUnavailable { fault_class, .. }
        | RuntimeLifecycleError::Forbidden { fault_class, .. }
        | RuntimeLifecycleError::Conflict { fault_class, .. } => match *fault_class {
            "runtime.start_refused.service_unavailable" => {
                AutonomousCoordinatorReason::DatabaseNotConfiguredOrInvalid
            }
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
            "runtime.start_refused.readiness_evidence_persist_failed" => {
                AutonomousCoordinatorReason::ReadinessEvidencePersistFailed
            }
            "runtime.start_refused.readiness_run_link_persist_failed" => {
                AutonomousCoordinatorReason::ReadinessRunLinkPersistFailed
            }
            "runtime.truth_mismatch.durable_active_without_local_owner" => {
                AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner
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

// NO-STRING-AUTHORITY-GUARD-SCOPE-END

// ---------------------------------------------------------------------------
// D1.6 — Deterministic blocker signature
// ---------------------------------------------------------------------------

/// Deterministic, bounded identity for one blocker occurrence. Same typed
/// reason + same stable identity inputs -> same signature; a changed reason
/// or a changed stable identity input -> a different signature. Free-form
/// error detail, timestamps, secrets, and full environment/filesystem error
/// text never enter the signature.
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

/// Bounded cap on `stable_context` length (chars) so a pathologically long
/// identity value cannot grow the signature unboundedly.
const MAX_BLOCKER_SIGNATURE_STABLE_CONTEXT_CHARS: usize = 512;

/// D1.6: compute a deterministic, bounded blocker signature for one typed
/// reason plus its stable identity inputs.
pub fn blocker_signature(
    reason: &AutonomousCoordinatorReason,
    identity: &AutonomousBlockerIdentity<'_>,
) -> AutonomousBlockerSignature {
    let reason_code = reason_code_str(reason);

    let mut parts: Vec<String> = Vec::new();
    if let Some(operation_id) = identity.operation_id {
        parts.push(format!("op={operation_id}"));
    }
    if let Some(v) = identity.assignment_identity {
        parts.push(format!("assign={v}"));
    }
    if let Some(v) = identity.runtime_binding_identity {
        parts.push(format!("bind={v}"));
    }
    if let Some(v) = identity.provider_id {
        parts.push(format!("provider={v}"));
    }
    if let Some(v) = identity.symbol {
        parts.push(format!("symbol={v}"));
    }
    if let Some(v) = identity.timeframe {
        parts.push(format!("timeframe={v}"));
    }
    let joined = parts.join("|");
    let bounded: String = joined
        .chars()
        .take(MAX_BLOCKER_SIGNATURE_STABLE_CONTEXT_CHARS)
        .collect();

    AutonomousBlockerSignature {
        reason_code,
        stable_context: if bounded.is_empty() {
            None
        } else {
            Some(bounded)
        },
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
        // configured and reachable (Internal variant, audited context
        // labels from create_or_reuse_run_for_start).
        for context in [
            "start active-run lookup failed",
            "start latest-run lookup failed",
            "start insert_run failed",
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
        let reason = AutonomousCoordinatorReason::AssignmentMissing;
        let pathological = "x".repeat(10_000);
        let identity = AutonomousBlockerIdentity {
            assignment_identity: Some(&pathological),
            ..Default::default()
        };
        let sig = blocker_signature(&reason, &identity);
        let len = sig.stable_context.map(|s| s.chars().count()).unwrap_or(0);
        assert!(
            len <= MAX_BLOCKER_SIGNATURE_STABLE_CONTEXT_CHARS,
            "stable_context must never exceed the documented bound, got {len}"
        );
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
