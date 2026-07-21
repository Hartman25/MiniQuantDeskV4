//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-
//! FINALIZATION-CAS: the strict evidence classifier and durable finalization
//! write seam authorized by the E1 outcome contract
//! (`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`)
//! and built on top of E2A's accepted coverage-anchor and run-lineage
//! authorities
//! (`docs/specs/autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.md`).
//!
//! Architecture: an async evidence-gathering pass performs every database
//! read up front into one [`AutonomousDailyOutcomeEvidenceSnapshot`]; a
//! separate, pure [`classify_autonomous_daily_outcome`] function applies the
//! E1 contract's exact global precedence order over that snapshot with zero
//! I/O. No process-local diagnostic counter (`AppState`'s in-memory
//! `bar_tick_dispatch_count`, `last_bar_signal_qty`, or any completed-bar-
//! task liveness cache) is ever read by any function in this module --
//! structurally impossible, since none of those types are even in scope
//! here.
//!
//! This module implements no API route and no GUI surface, and sends no
//! notification itself.
//! `classify_and_finalize_autonomous_daily_operation` is the one production
//! entry point AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-
//! FINALIZATION-INTEGRATION-AND-NOTIFICATION wires into the coordinator's
//! `AwaitingOutcomeFinalization`/`evidence_degraded` handling
//! (`autonomous_daily_coordinator.rs`); the coordinator supplies
//! `AutonomousDailyFinalizationContext`/`AutonomousDailyFinalizationPolicyInputs`
//! from `AppState`/env exactly as `ensure_coverage_authority` already does
//! for coverage-anchor binding, and `session_controller.rs`'s
//! `log_coordinator_outcome` owns sending the E1 §12 notifications, gated on
//! this module's own durable `newly_applied`/`Finalized`-vs-`AlreadyFinalized`
//! truth -- never on process-local memory. `persist_autonomous_daily_finalization_blocker`
//! (E3.4) is the one additional narrow seam E3 uses when current policy
//! resolution itself fails before evidence gathering can even begin.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;
use sqlx::PgPool;
use uuid::Uuid;

use super::autonomous_daily_coverage_authority::{
    check_coverage_authority, construct_coverage_bound_detail,
    coverage_construction_inputs_from_operation, resolve_current_coverage_policy_inputs,
    CoverageAuthorityCheck, CoverageBoundDetail,
};
use super::autonomous_daily_operation::{
    derive_assignment_identity, derive_runtime_binding_identity,
};
use super::autonomous_retry_policy::{
    blocker_signature, AutonomousBlockerIdentity, AutonomousCoordinatorReason,
};
use super::market_calendar::MarketCalendarProvider;
use super::MultiSymbolRuntimeConfig;

use mqk_db::{
    AutonomousDailyBarDispatchRecord, AutonomousDailyOperationRecord, InboxRow, OutboxRow,
};

// ---------------------------------------------------------------------------
// E2B.1 -- closed outcome types
// ---------------------------------------------------------------------------

pub const REASON_ACTIVITY_FILL_CONFIRMED: &str = mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED;
pub const REASON_ACTIVITY_ORDER_SUBMITTED: &str = mqk_db::OUTCOME_ACTIVITY_ORDER_SUBMITTED;
pub const REASON_ACTIVITY_DECISION_ACCEPTED: &str = mqk_db::OUTCOME_ACTIVITY_DECISION_ACCEPTED;
pub const REASON_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL: &str =
    mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL;

pub const REASON_UNKNOWN_ASSIGNMENT_IDENTITY_UNAVAILABLE: &str =
    "unknown_assignment_identity_unavailable";
pub const REASON_UNKNOWN_RUN_LINEAGE_UNAVAILABLE: &str = "unknown_run_lineage_unavailable";
pub const REASON_UNKNOWN_INCOMPLETE_BAR_COVERAGE: &str = "unknown_incomplete_bar_coverage";
pub const REASON_UNKNOWN_UNRESOLVED_DISPATCH_CLAIM: &str = "unknown_unresolved_dispatch_claim";
pub const REASON_UNKNOWN_MISSING_EVALUATION_EVIDENCE: &str = "unknown_missing_evaluation_evidence";
pub const REASON_UNKNOWN_ORDER_EVIDENCE_CONFLICT: &str = "unknown_order_evidence_conflict";
pub const REASON_UNKNOWN_DATABASE_UNAVAILABLE: &str = "unknown_database_unavailable";
pub const REASON_UNKNOWN_RUNTIME_STOP_UNPROVEN: &str = "unknown_runtime_stop_unproven";

/// Closed terminal-activity/no-trade reason set. Never a fourth,
/// free-form, or generic variant -- exactly the four codes the E1 contract
/// §10 authorizes for this patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousDailyTerminalReason {
    ActivityFillConfirmed,
    ActivityOrderSubmitted,
    ActivityDecisionAccepted,
    NoTradeStrategyEvaluatedNoSignal,
}

impl AutonomousDailyTerminalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActivityFillConfirmed => REASON_ACTIVITY_FILL_CONFIRMED,
            Self::ActivityOrderSubmitted => REASON_ACTIVITY_ORDER_SUBMITTED,
            Self::ActivityDecisionAccepted => REASON_ACTIVITY_DECISION_ACCEPTED,
            Self::NoTradeStrategyEvaluatedNoSignal => REASON_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        }
    }

    /// The exact terminal DB state this reason finalizes into.
    pub fn target_state(self) -> &'static str {
        match self {
            Self::NoTradeStrategyEvaluatedNoSignal => mqk_db::STATE_COMPLETED_NO_TRADE,
            Self::ActivityFillConfirmed
            | Self::ActivityOrderSubmitted
            | Self::ActivityDecisionAccepted => mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        }
    }
}

/// Closed nonterminal evidence-gap reason set (§7/§10 of the E1 contract).
/// `no_trade_all_signals_blocked`, `no_trade_no_bar_expected`, and
/// `unknown_insufficient_bar_evidence` are deliberately absent -- not
/// authorized for this patch (E1 §9, §6 Correction 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousDailyUnknownReason {
    AssignmentIdentityUnavailable,
    RunLineageUnavailable,
    IncompleteBarCoverage,
    UnresolvedDispatchClaim,
    MissingEvaluationEvidence,
    OrderEvidenceConflict,
    DatabaseUnavailable,
    RuntimeStopUnproven,
}

impl AutonomousDailyUnknownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssignmentIdentityUnavailable => REASON_UNKNOWN_ASSIGNMENT_IDENTITY_UNAVAILABLE,
            Self::RunLineageUnavailable => REASON_UNKNOWN_RUN_LINEAGE_UNAVAILABLE,
            Self::IncompleteBarCoverage => REASON_UNKNOWN_INCOMPLETE_BAR_COVERAGE,
            Self::UnresolvedDispatchClaim => REASON_UNKNOWN_UNRESOLVED_DISPATCH_CLAIM,
            Self::MissingEvaluationEvidence => REASON_UNKNOWN_MISSING_EVALUATION_EVIDENCE,
            Self::OrderEvidenceConflict => REASON_UNKNOWN_ORDER_EVIDENCE_CONFLICT,
            Self::DatabaseUnavailable => REASON_UNKNOWN_DATABASE_UNAVAILABLE,
            Self::RuntimeStopUnproven => REASON_UNKNOWN_RUNTIME_STOP_UNPROVEN,
        }
    }
}

/// Bounded, non-free-form evidence counts persisted alongside a
/// classification's audit detail. Never a raw SQL/provider/panic string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutonomousDailyOutcomeEvidenceSummary {
    pub expected_bar_count: i64,
    pub claim_count: i64,
    pub evaluation_count: i64,
    pub outbox_count: i64,
    pub fill_count: i64,
}

/// The one typed classification model the pure classifier may produce.
/// Generic `completed` is not a representable variant -- structurally
/// impossible for the automatic classifier to emit.
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomousDailyOutcomeClassification {
    CompletedWithActivity {
        reason_code: AutonomousDailyTerminalReason,
        evidence: AutonomousDailyOutcomeEvidenceSummary,
    },
    CompletedNoTrade {
        reason_code: AutonomousDailyTerminalReason,
        evidence: AutonomousDailyOutcomeEvidenceSummary,
    },
    EvidenceBlocked {
        reason_code: AutonomousDailyUnknownReason,
        evidence: AutonomousDailyOutcomeEvidenceSummary,
    },
}

// ---------------------------------------------------------------------------
// E2B.2 -- finalization eligibility
// ---------------------------------------------------------------------------

/// Process-local runtime-ownership fact the caller supplies -- the E3
/// coordinator integration obtains this from `AppState::locally_owned_run_id()`
/// compared against `operation.run_id`; this module represents it as an
/// explicit input so the classifier itself never reaches into daemon-global
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutonomousDailyFinalizationContext {
    pub matching_local_runtime_active: bool,
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
// UNCERTAINTY-CLOSURE (REPAIR 5/6): the narrow injected effect seam that lets
// a test deterministically prove commit-uncertainty and partial-evidence-
// read-failure behavior against a *real* database, without mocking any
// successful write. Every field defaults to `false`/`None`, and the
// production entry point (`classify_and_finalize_autonomous_daily_operation`)
// always passes the all-default value -- production behavior is byte-for-
// byte unchanged by this type's existence. No field ever substitutes a fake
// result for a real one on the successful-write side; each field either (a)
// performs one additional *real* database write before the real write under
// test, so that write's own CAS/re-read logic runs unmodified against
// genuinely different DB state, or (b) discards this attempt's *observation*
// of an already-real, already-committed result, forcing the same
// authoritative-re-read path a genuinely lost acknowledgment would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutonomousDailyFinalizationEffectSeam {
    /// When the real terminal finalization CAS returns `Ok(Applied(_))`
    /// (the transaction has genuinely committed), discard that result and
    /// treat the write as uncertain instead -- modeling "commit applied,
    /// acknowledgment lost." The subsequent authoritative re-read must find
    /// the same, already-committed terminal truth.
    pub force_commit_acknowledgment_lost: bool,
    /// A real, separate database write performed immediately before the
    /// real terminal finalization CAS, so that CAS's own `(expected_state,
    /// expected_state_version)` guard naturally fails against genuinely
    /// different DB state -- never a mocked CAS result.
    pub pre_commit_effect: AutonomousDailyFinalizationInjectedPreCommitEffect,
    /// When true, `gather_autonomous_daily_outcome_evidence` fails with a
    /// synthetic `Err` immediately after successfully resolving identity,
    /// lineage, coverage, and every expected bar's claim/evaluation
    /// evidence -- modeling a real, *later* evidence read failing, never a
    /// failure of the initial operation-row load itself (a complete outage
    /// is `store_46`'s scenario, unaffected by this field).
    pub force_evidence_read_failure_after_claims: bool,
    /// When true, the best-effort `evidence_degraded` blocker write (issued
    /// after an evidence-read failure) is skipped entirely rather than
    /// attempted -- modeling a database that is also unavailable for the
    /// blocker-write step. The subsequent re-read is real and correctly
    /// observes that no write took place.
    pub force_blocker_persistence_unavailable: bool,
}

/// A real, separate database mutation to perform immediately before the real
/// terminal finalization CAS under test, so that CAS's own guard naturally
/// mismatches against genuinely different, already-committed DB state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutonomousDailyFinalizationInjectedPreCommitEffect {
    #[default]
    None,
    /// Real transition of the same operation, at its current (still-stale-
    /// to-the-caller) `(state, state_version)`, to `stop_retrying` -- a
    /// legal edge from both `stopping` and (as a same-state no-op guard)
    /// already `stop_retrying`. Models "CAS did not apply": the real
    /// terminal CAS below naturally observes a stale expected version.
    StaleVersionBeforeCommit,
    /// Real terminal finalization of the same operation, at its current
    /// (still-stale-to-the-caller) `(state, state_version)`, to a
    /// *different* valid terminal reason than the one under test. Models a
    /// genuinely conflicting concurrent writer: the real terminal CAS below
    /// naturally fails against an already-terminal row with different
    /// truth.
    ConflictingTerminalWriteBeforeCommit(AutonomousDailyTerminalReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousDailyFinalizationEligibility {
    /// `state ∈ {stopping, stop_retrying}`, no matching local runtime, and
    /// `stopped_at_utc` is present.
    Eligible,
    /// `state` is not `stopping`/`stop_retrying` (includes already-terminal
    /// and every other nonterminal state) -- not yet eligible, no
    /// classification attempt of any kind.
    NotEligible,
    /// `state ∈ {stopping, stop_retrying}` but either a matching local
    /// runtime is still active or `stopped_at_utc` is not yet set -- not yet
    /// eligible; per the E1 contract §7 this narrow residual case is *not*
    /// itself a trigger to persist `unknown_runtime_stop_unproven` -- it
    /// means "not yet eligible," not "evidence gap," so no durable write of
    /// any kind occurs for this case either.
    RuntimeStopUnproven,
}

/// Pure eligibility check (E1 contract §3.2, conditions 2-4; condition 1 is
/// satisfied by the caller having already loaded `operation`; condition 5 is
/// implied by conditions 2-3 together per the contract's own note).
pub fn check_finalization_eligibility(
    operation: &AutonomousDailyOperationRecord,
    context: &AutonomousDailyFinalizationContext,
) -> AutonomousDailyFinalizationEligibility {
    if !matches!(
        operation.state.as_str(),
        mqk_db::STATE_STOPPING | mqk_db::STATE_STOP_RETRYING
    ) {
        return AutonomousDailyFinalizationEligibility::NotEligible;
    }
    if context.matching_local_runtime_active {
        return AutonomousDailyFinalizationEligibility::RuntimeStopUnproven;
    }
    if operation.stopped_at_utc.is_none() {
        return AutonomousDailyFinalizationEligibility::RuntimeStopUnproven;
    }
    AutonomousDailyFinalizationEligibility::Eligible
}

// ---------------------------------------------------------------------------
// E2B.5 -- exact expected dispatch-bar set (pure)
// ---------------------------------------------------------------------------

/// Derive the exact intended dispatch-bar set from an already-bound,
/// immutable [`CoverageBoundDetail`] (E1 contract §6 item 2, steps 4-10):
/// `first_dispatchable_bar_end_ts` plus every later canonical current-session
/// grid identity whose own expectation instant
/// (`bar_end_ts + timeframe_secs + effective_grace_seconds`) is strictly
/// greater than the first bar's own expectation instant and strictly less
/// than `effective_operation_close_utc`. Reuses only
/// [`crate::daily_data_readiness::intraday_grid_starts`] -- no second
/// calendar/timeframe algorithm. Pure: no I/O, no wall clock, no
/// `started_at_utc` lower bound, and no narrowing for a late start or a
/// recovery gap -- both are evaluated as missing coverage by the caller
/// comparing this set against durable claims, never by shrinking this set.
pub fn derive_expected_bar_set(detail: &CoverageBoundDetail) -> Vec<i64> {
    let current_grid = crate::daily_data_readiness::intraday_grid_starts(
        detail.exchange_session_open_utc,
        detail.exchange_session_close_utc,
        detail.timeframe_secs,
    );
    let first_expectation_instant = detail.first_dispatchable_bar_end_ts
        + detail.timeframe_secs
        + detail.effective_grace_seconds;
    let close_ts = detail.effective_operation_close_utc.timestamp();

    let mut later: Vec<i64> = current_grid
        .into_iter()
        .filter(|&slot_start| {
            let expectation = slot_start + detail.timeframe_secs + detail.effective_grace_seconds;
            expectation > first_expectation_instant && expectation < close_ts
        })
        .collect();
    later.sort_unstable();
    later.dedup();

    let mut set = Vec::with_capacity(later.len() + 1);
    set.push(detail.first_dispatchable_bar_end_ts);
    set.extend(later);
    set
}

// ---------------------------------------------------------------------------
// E2B.3 -- durable evidence snapshot
// ---------------------------------------------------------------------------

/// Per-expected-bar evidence: the durable claim (if any) and the durable
/// evaluation it points to (if the claim is `completed` with a non-null
/// `evaluation_id` that itself resolved to a real row). Both fields are
/// gathered as raw, unvalidated durable facts -- every cross-check (run
/// lineage membership, strategy/symbol/timeframe/decision-stage/bar-identity
/// agreement, duplicate-evaluation-id detection) is performed by the pure
/// classifier, never precomputed here, so a snapshot built by hand in a unit
/// test exercises the identical validation path a DB-backed snapshot does.
#[derive(Debug, Clone)]
pub struct ExpectedBarClaimEvidence {
    pub bar_end_ts: i64,
    pub claim: Option<AutonomousDailyBarDispatchRecord>,
    pub evaluation: Option<mqk_db::StrategySignalEvaluationRecord>,
}

/// Every database-read fact the pure classifier needs, gathered once before
/// classification begins. No process-local diagnostic field exists on this
/// type by construction.
#[derive(Debug, Clone)]
pub struct AutonomousDailyOutcomeEvidenceSnapshot {
    pub operation_id: Uuid,
    pub current_run_id: Option<Uuid>,

    /// `true` iff the current process's freshly-resolved
    /// `assignment_identity`/`runtime_binding_identity` exactly match the
    /// operation's persisted values.
    pub identity_match: bool,
    /// `Some(lineage)` iff the full run-id lineage read/validated cleanly
    /// (E2A `fetch_and_validate_autonomous_daily_operation_run_lineage`);
    /// `None` on any validation failure.
    pub lineage: Option<Vec<Uuid>>,
    /// `Some(detail)` iff a durable coverage anchor exists and the current
    /// process's freshly-resolved policy is semantically compatible with it
    /// (E2A `CoverageAuthorityCheck::Compatible`); `None` for every other
    /// case (not bound, unreadable, invalid, conflicting, or unresolvable
    /// current policy).
    pub coverage: Option<CoverageBoundDetail>,
    /// The current process's freshly-resolved strategy id, used to validate
    /// each completed claim's evaluation is strategy-compatible (E1 §6b
    /// item 2 evaluation cross-check).
    pub expected_strategy_id: Option<String>,

    /// The exact expected dispatch-bar set derived from `coverage`; empty
    /// when `coverage` is `None`.
    pub expected_bars: Vec<i64>,
    /// One entry per `expected_bars` element, in the same order.
    pub claims: Vec<ExpectedBarClaimEvidence>,
    /// Every `sys_autonomous_daily_bar_dispatches` row ever created for this
    /// operation, any status, any bar identity -- used to detect an extra/
    /// future/inconsistent claim outside the expected set.
    pub total_claim_count: i64,

    pub aggregate_bars_observed: i64,
    pub aggregate_bars_dispatched: i64,
    pub aggregate_last_completed_bar_ts: Option<i64>,
    pub aggregate_last_dispatched_bar_ts: Option<i64>,

    /// Every `oms_outbox` row across the complete validated run lineage.
    pub outbox_rows: Vec<OutboxRow>,
    /// Every `oms_inbox` row across the complete validated run lineage.
    pub inbox_rows: Vec<InboxRow>,
}

fn evidence_summary(
    snapshot: &AutonomousDailyOutcomeEvidenceSnapshot,
) -> AutonomousDailyOutcomeEvidenceSummary {
    AutonomousDailyOutcomeEvidenceSummary {
        expected_bar_count: snapshot.expected_bars.len() as i64,
        claim_count: snapshot.claims.iter().filter(|c| c.claim.is_some()).count() as i64,
        evaluation_count: snapshot
            .claims
            .iter()
            .filter(|c| c.evaluation.is_some())
            .count() as i64,
        outbox_count: snapshot.outbox_rows.len() as i64,
        fill_count: snapshot
            .inbox_rows
            .iter()
            .filter(|r| matches!(r.event_kind.as_str(), "fill" | "partial_fill"))
            .count() as i64,
    }
}

// ---------------------------------------------------------------------------
// E2B.3/E2B.4/E2B.6/E2B.7/E2B.9/E2B.10 -- async evidence gathering
// ---------------------------------------------------------------------------

/// Every already-resolved input the evidence gatherer and finalizer need to
/// independently re-derive the current process's coverage/identity policy --
/// mirrors exactly the parameters
/// [`super::autonomous_daily_coordinator::ensure_coverage_authority`] already
/// threads through [`resolve_current_coverage_policy_inputs`]/
/// [`construct_coverage_bound_detail`]. No `AppState` dependency: a
/// narrower, source-aligned signature per the E2B mission.
pub struct AutonomousDailyFinalizationPolicyInputs<'a> {
    pub calendar_provider: &'a dyn MarketCalendarProvider,
    pub config: &'a MultiSymbolRuntimeConfig,
    pub binding: &'a EffectiveRuntimeBinding,
    pub strategy_registry: &'a PluginRegistry,
}

/// Gather every database-read fact [`classify_autonomous_daily_outcome`]
/// needs into one durable snapshot. All reads happen here; nothing below
/// this function touches the database again for the same classification
/// attempt. A propagated `Err` here is a genuine partial-read-failure
/// (E1 contract §9) -- the caller treats it as `unknown_database_unavailable`.
pub async fn gather_autonomous_daily_outcome_evidence(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
    effect_seam: AutonomousDailyFinalizationEffectSeam,
) -> anyhow::Result<AutonomousDailyOutcomeEvidenceSnapshot> {
    let assignment_identity = derive_assignment_identity(policy_inputs.config);
    let runtime_binding_identity = derive_runtime_binding_identity(policy_inputs.binding);
    let identity_match = assignment_identity == operation.assignment_identity
        && runtime_binding_identity == operation.runtime_binding_identity;
    let expected_strategy_id = policy_inputs.binding.effective_runtime_strategy_id.clone();

    let lineage =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(pool, operation)
            .await?
            .ok();

    let coverage = resolve_coverage_for_gather(
        pool,
        operation,
        &assignment_identity,
        &runtime_binding_identity,
        policy_inputs,
    )
    .await?;

    let expected_bars = coverage
        .as_ref()
        .map(derive_expected_bar_set)
        .unwrap_or_default();

    let mut claims = Vec::with_capacity(expected_bars.len());
    if let Some(detail) = &coverage {
        for &bar_end_ts in &expected_bars {
            let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
                pool,
                operation.operation_id,
                &detail.local_symbol,
                &detail.timeframe,
                bar_end_ts,
            )
            .await?;

            let evaluation = match &claim {
                Some(c)
                    if c.status == mqk_db::DISPATCH_STATUS_COMPLETED
                        && c.evaluation_id.is_some() =>
                {
                    mqk_db::fetch_strategy_signal_evaluation(pool, c.evaluation_id.unwrap()).await?
                }
                _ => None,
            };

            claims.push(ExpectedBarClaimEvidence {
                bar_end_ts,
                claim,
                evaluation,
            });
        }
    }

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
    // UNCERTAINTY-CLOSURE (REPAIR 6): test-only injected failure point for a
    // real, *later* evidence read -- identity/lineage/coverage/every
    // expected bar's claim+evaluation have already been read successfully
    // above; production never sets this field, so this is a no-op except
    // under an explicit test-support call.
    if effect_seam.force_evidence_read_failure_after_claims {
        anyhow::bail!(
            "gather_autonomous_daily_outcome_evidence: test-injected partial evidence read \
             failure after claims for operation {}",
            operation.operation_id
        );
    }

    let total_claim_count =
        mqk_db::count_autonomous_daily_bar_dispatch_claims(pool, operation.operation_id).await?;

    let mut outbox_rows = Vec::new();
    let mut inbox_rows = Vec::new();
    if let Some(lineage) = &lineage {
        for &run_id in lineage {
            outbox_rows.extend(mqk_db::outbox_load_all_for_run(pool, run_id).await?);
            inbox_rows.extend(mqk_db::inbox_load_all_for_run(pool, run_id).await?);
        }
    }

    Ok(AutonomousDailyOutcomeEvidenceSnapshot {
        operation_id: operation.operation_id,
        current_run_id: operation.run_id,
        identity_match,
        lineage,
        coverage,
        expected_strategy_id,
        expected_bars,
        claims,
        total_claim_count,
        aggregate_bars_observed: operation.bars_observed,
        aggregate_bars_dispatched: operation.bars_dispatched,
        aggregate_last_completed_bar_ts: operation.last_completed_bar_ts,
        aggregate_last_dispatched_bar_ts: operation.last_dispatched_bar_ts,
        outbox_rows,
        inbox_rows,
    })
}

/// Resolve and validate the durable coverage anchor for gathering purposes.
/// A policy-resolution or construction failure is a legitimate "coverage
/// unavailable" outcome (`None`), never a propagated I/O error --
/// [`check_coverage_authority`]'s own DB read is the only fallible I/O step
/// here, and its error propagates as a genuine partial-read failure.
async fn resolve_coverage_for_gather(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
) -> anyhow::Result<Option<CoverageBoundDetail>> {
    let policy = match resolve_current_coverage_policy_inputs(
        policy_inputs.config,
        policy_inputs.binding,
        policy_inputs.strategy_registry,
    ) {
        Ok(policy) => policy,
        Err(_) => return Ok(None),
    };

    let Some(inputs) = coverage_construction_inputs_from_operation(
        operation,
        assignment_identity,
        runtime_binding_identity,
        &policy,
    ) else {
        return Ok(None);
    };

    let fresh = match construct_coverage_bound_detail(policy_inputs.calendar_provider, &inputs) {
        Ok(fresh) => fresh,
        Err(_) => return Ok(None),
    };

    match check_coverage_authority(pool, operation.operation_id, &fresh).await? {
        CoverageAuthorityCheck::Compatible(detail) => Ok(Some(detail)),
        CoverageAuthorityCheck::NotBound
        | CoverageAuthorityCheck::Unreadable
        | CoverageAuthorityCheck::Invalid
        | CoverageAuthorityCheck::Conflict => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// E2B.13 -- pure global precedence classifier
// ---------------------------------------------------------------------------

fn blocked(
    reason: AutonomousDailyUnknownReason,
    snapshot: &AutonomousDailyOutcomeEvidenceSnapshot,
) -> AutonomousDailyOutcomeClassification {
    AutonomousDailyOutcomeClassification::EvidenceBlocked {
        reason_code: reason,
        evidence: evidence_summary(snapshot),
    }
}

/// Apply the E1 contract's exact global precedence order (§8, restated by
/// §13 for E2B) over an already-gathered snapshot. Pure: no I/O, no wall
/// clock. Never produces generic `completed`. Never reads a process-local
/// diagnostic counter -- structurally impossible, since `snapshot` carries
/// none.
pub fn classify_autonomous_daily_outcome(
    snapshot: &AutonomousDailyOutcomeEvidenceSnapshot,
) -> AutonomousDailyOutcomeClassification {
    // Step 2: assignment/runtime-binding identity.
    if !snapshot.identity_match {
        return blocked(
            AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
            snapshot,
        );
    }

    // Step 3: full run lineage.
    let Some(lineage) = &snapshot.lineage else {
        return blocked(
            AutonomousDailyUnknownReason::RunLineageUnavailable,
            snapshot,
        );
    };

    // Step 4: durable coverage anchor. AUTONOMOUS-DAILY-PAPER-OPERATIONS-
    // 01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-UNCERTAINTY-CLOSURE (REPAIR 4):
    // a missing/unusable anchor is always `unknown_incomplete_bar_coverage`,
    // regardless of whether the run lineage is empty. The prior empty-
    // lineage special case (routing to `unknown_missing_evaluation_evidence`
    // for a "never reached running" operation) is removed: the precedence
    // order is strictly identity -> lineage -> durable coverage authority ->
    // expected-bar coverage -> claims -> evaluations, and coverage is
    // decided purely on its own presence/validity, never on a downstream
    // signal (lineage emptiness) that belongs to an earlier step.
    // `unknown_missing_evaluation_evidence` is reachable only once coverage
    // and expected-bar truth are both established (steps 5-7 below).
    let Some(coverage) = &snapshot.coverage else {
        return blocked(
            AutonomousDailyUnknownReason::IncompleteBarCoverage,
            snapshot,
        );
    };

    // Step 5: complete expected-bar coverage + aggregate consistency.
    if snapshot.expected_bars.is_empty() {
        return blocked(
            AutonomousDailyUnknownReason::IncompleteBarCoverage,
            snapshot,
        );
    }
    let final_expected = *snapshot
        .expected_bars
        .last()
        .expect("checked nonempty above");

    let matched_claim_count = snapshot.claims.iter().filter(|c| c.claim.is_some()).count();
    let all_expected_claimed = snapshot.claims.len() == snapshot.expected_bars.len()
        && matched_claim_count == snapshot.claims.len();
    let extra_claims_exist = snapshot.total_claim_count > matched_claim_count as i64;
    let aggregate_consistent = snapshot.aggregate_bars_observed
        == snapshot.expected_bars.len() as i64
        && snapshot.aggregate_bars_dispatched == snapshot.expected_bars.len() as i64
        && snapshot.aggregate_last_completed_bar_ts == Some(final_expected)
        && snapshot.aggregate_last_dispatched_bar_ts == Some(final_expected);

    if !all_expected_claimed || extra_claims_exist || !aggregate_consistent {
        return blocked(
            AutonomousDailyUnknownReason::IncompleteBarCoverage,
            snapshot,
        );
    }

    // Step 6: zero unresolved/contradictory claims.
    for c in &snapshot.claims {
        let claim = c
            .claim
            .as_ref()
            .expect("all_expected_claimed checked above");
        if claim.status != mqk_db::DISPATCH_STATUS_COMPLETED {
            return blocked(
                AutonomousDailyUnknownReason::UnresolvedDispatchClaim,
                snapshot,
            );
        }
        if claim.evaluation_id.is_none() {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
    }

    // Step 7: exact evaluation evidence, per claim.
    let mut seen_evaluation_ids: HashSet<Uuid> = HashSet::with_capacity(snapshot.claims.len());
    for c in &snapshot.claims {
        let claim = c
            .claim
            .as_ref()
            .expect("all_expected_claimed checked above");
        let evaluation_id = claim.evaluation_id.expect("checked non-null above");

        if !seen_evaluation_ids.insert(evaluation_id) {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }

        let Some(evaluation) = &c.evaluation else {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        };
        if evaluation.evaluation_id != evaluation_id {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        let in_lineage = evaluation
            .run_id
            .map(|rid| lineage.contains(&rid))
            .unwrap_or(false);
        if !in_lineage {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        if Some(evaluation.strategy_id.as_str()) != snapshot.expected_strategy_id.as_deref() {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        if evaluation.symbol.trim().to_uppercase() != coverage.local_symbol {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        if evaluation.timeframe != coverage.timeframe {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        if evaluation.decision_stage != "strategy_evaluated" {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
        let expected_bar_dt = DateTime::<Utc>::from_timestamp(c.bar_end_ts, 0);
        if evaluation.latest_bar_ts_utc != expected_bar_dt {
            return blocked(
                AutonomousDailyUnknownReason::MissingEvaluationEvidence,
                snapshot,
            );
        }
    }

    // Step 8/9: activity tier, strongest first.
    let any_fill = snapshot
        .inbox_rows
        .iter()
        .any(|r| matches!(r.event_kind.as_str(), "fill" | "partial_fill"));
    if any_fill {
        return AutonomousDailyOutcomeClassification::CompletedWithActivity {
            reason_code: AutonomousDailyTerminalReason::ActivityFillConfirmed,
            evidence: evidence_summary(snapshot),
        };
    }

    let any_broker_reach =
        snapshot.outbox_rows.iter().any(|r| {
            matches!(r.status.as_str(), "SENT" | "ACKED") || r.dispatching_at_utc.is_some()
        }) || snapshot.inbox_rows.iter().any(|r| {
            matches!(
                r.event_kind.as_str(),
                "ack" | "cancel_ack" | "replace_ack" | "reject"
            )
        });
    if any_broker_reach {
        return AutonomousDailyOutcomeClassification::CompletedWithActivity {
            reason_code: AutonomousDailyTerminalReason::ActivityOrderSubmitted,
            evidence: evidence_summary(snapshot),
        };
    }

    if !snapshot.outbox_rows.is_empty() {
        return AutonomousDailyOutcomeClassification::CompletedWithActivity {
            reason_code: AutonomousDailyTerminalReason::ActivityDecisionAccepted,
            evidence: evidence_summary(snapshot),
        };
    }

    // Step 8 (order-evidence conflict) / Step 10 (no-trade).
    let any_nonzero_signal = snapshot.claims.iter().any(|c| {
        c.evaluation
            .as_ref()
            .map(|e| e.signal_generated || e.signal_qty.unwrap_or(0) != 0)
            .unwrap_or(false)
    });
    if any_nonzero_signal {
        return blocked(
            AutonomousDailyUnknownReason::OrderEvidenceConflict,
            snapshot,
        );
    }

    AutonomousDailyOutcomeClassification::CompletedNoTrade {
        reason_code: AutonomousDailyTerminalReason::NoTradeStrategyEvaluatedNoSignal,
        evidence: evidence_summary(snapshot),
    }
}

// ---------------------------------------------------------------------------
// E2B.14/E2B.15/E2B.16 -- terminal finalization CAS, with commit-uncertainty
// re-read
// ---------------------------------------------------------------------------

fn bounded_text(s: &str, max_len: usize) -> String {
    if s.chars().count() > max_len {
        s.chars().take(max_len).collect()
    } else {
        s.to_string()
    }
}

fn bounded_detail(s: &str) -> String {
    bounded_text(s, 4000)
}

/// Result of the high-level finalization entry point (E2B.20).
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomousDailyFinalizationOutcome {
    /// Not yet eligible per §3.2 -- no classification attempt, no write of
    /// any kind.
    NotEligible,
    /// The operation was already terminal (or became terminal via a
    /// concurrent writer observed by this attempt's re-read) with exactly
    /// the classification this attempt would have produced -- read-only.
    AlreadyFinalized(AutonomousDailyOperationRecord),
    /// The terminal finalization CAS applied (or was confirmed applied via
    /// an authoritative re-read after an uncertain write).
    Finalized {
        outcome: String,
        record: AutonomousDailyOperationRecord,
    },
    /// The classifier could not resolve a terminal outcome; the durable
    /// `evidence_degraded` blocker was applied (or confirmed applied, or
    /// refreshed in place on an exact-reason replay).
    ///
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
    /// INTEGRATION-AND-NOTIFICATION: `newly_applied` is `true` only when
    /// *this* invocation durably advanced truth -- a fresh
    /// `stopping`/`stop_retrying -> evidence_degraded` transition that
    /// applied for real, or an in-place refresh whose reason/signature
    /// genuinely changed from what was already persisted. It is `false` for
    /// an exact-reason replay (`AlreadyApplied`) and for the
    /// authoritative-re-read fallback after an ambiguous write, since that
    /// path can never prove *this* attempt was the one that advanced durable
    /// truth. E3's coordinator notification dedup is gated on this field --
    /// never on process-local memory.
    EvidenceDegraded {
        reason_code: AutonomousDailyUnknownReason,
        record: AutonomousDailyOperationRecord,
        newly_applied: bool,
    },
    /// A previously `evidence_degraded` (post-stop) operation's evidence now
    /// resolves cleanly; the operation took the existing
    /// `evidence_degraded -> stopping` edge. A later invocation from
    /// `stopping` may perform the terminal CAS -- this attempt does not
    /// finalize directly.
    RecoveredToStopping(AutonomousDailyOperationRecord),
    /// Complete outage (the operation row itself could not be loaded), or a
    /// partial evidence-read failure whose best-effort blocker write could
    /// not itself be confirmed durable. No write was claimed persisted.
    DatabaseUnavailable,
    /// The operation is already terminal at a different state and/or a
    /// different outcome than this attempt's classification would produce.
    /// Never rewritten.
    Conflict,
}

fn blocker_signature_for_unknown(
    operation: &AutonomousDailyOperationRecord,
    reason: AutonomousDailyUnknownReason,
) -> super::autonomous_retry_policy::AutonomousBlockerSignature {
    blocker_signature(
        &AutonomousCoordinatorReason::UnclassifiedFailClosed {
            fault_class: reason.as_str(),
        },
        &AutonomousBlockerIdentity {
            operation_id: Some(operation.operation_id),
            assignment_identity: Some(&operation.assignment_identity),
            runtime_binding_identity: Some(&operation.runtime_binding_identity),
            ..Default::default()
        },
    )
}

/// Attempt the terminal finalization CAS; on any ambiguous write result
/// (CAS-false, store error), perform one authoritative re-read by
/// `operation_id` before deciding the final outcome -- never claims success
/// on the write result alone. `effect_seam` is the REPAIR 5 test-only
/// injected effect; production always passes the all-default value, which
/// changes nothing below.
async fn finalize_with_commit_uncertainty(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason: AutonomousDailyTerminalReason,
    now_utc: DateTime<Utc>,
    effect_seam: AutonomousDailyFinalizationEffectSeam,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let target_state = reason.target_state().to_string();
    let outcome_code = reason.as_str().to_string();

    // REPAIR 5: a real, separate database write performed before the real
    // CAS under test, so that CAS's own guard naturally mismatches against
    // genuinely different, already-committed DB state -- never a mocked CAS
    // result. Production (`AutonomousDailyFinalizationInjectedPreCommitEffect::None`)
    // performs no extra write here.
    match effect_seam.pre_commit_effect {
        AutonomousDailyFinalizationInjectedPreCommitEffect::None => {}
        AutonomousDailyFinalizationInjectedPreCommitEffect::StaleVersionBeforeCommit => {
            let _ = mqk_db::transition_autonomous_daily_operation(
                pool,
                &mqk_db::TransitionAutonomousDailyOperationArgs {
                    operation_id: operation.operation_id,
                    expected_state: operation.state.clone(),
                    expected_state_version: operation.state_version,
                    new_state: mqk_db::STATE_STOP_RETRYING.to_string(),
                    reason_code: None,
                    blocker_signature: None,
                    occurred_at_utc: now_utc,
                    run_id: None,
                    bounded_detail: bounded_detail(
                        "test-injected: real pre-commit version bump (stale-CAS proof)",
                    ),
                },
            )
            .await;
        }
        AutonomousDailyFinalizationInjectedPreCommitEffect::ConflictingTerminalWriteBeforeCommit(
            conflicting_reason,
        ) => {
            let conflicting_args = mqk_db::FinalizeAutonomousDailyOperationArgs {
                operation_id: operation.operation_id,
                expected_state: operation.state.clone(),
                expected_state_version: operation.state_version,
                target_state: conflicting_reason.target_state().to_string(),
                outcome: conflicting_reason.as_str().to_string(),
                finalized_at_utc: now_utc,
                bounded_detail: bounded_detail(
                    "test-injected: real conflicting concurrent-writer finalize",
                ),
            };
            let _ = mqk_db::finalize_autonomous_daily_operation(pool, &conflicting_args).await;
        }
    }

    let args = mqk_db::FinalizeAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        target_state: target_state.clone(),
        outcome: outcome_code.clone(),
        finalized_at_utc: now_utc,
        bounded_detail: bounded_detail(&format!(
            "autonomous daily outcome finalized: {outcome_code}"
        )),
    };

    let mut write_result = mqk_db::finalize_autonomous_daily_operation(pool, &args).await;

    // REPAIR 5: the real write above has already committed for real; this
    // discards this attempt's observation of that `Ok(Applied(_))` result,
    // modeling "commit applied, acknowledgment lost." Production never sets
    // this field.
    if effect_seam.force_commit_acknowledgment_lost {
        if let Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(_)) = &write_result {
            write_result = Err(anyhow::anyhow!(
                "test-injected: finalize commit acknowledgment lost after real commit for \
                 operation {}",
                operation.operation_id
            ));
        }
    }

    match write_result {
        Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record)) => {
            Ok(AutonomousDailyFinalizationOutcome::Finalized {
                outcome: outcome_code,
                record,
            })
        }
        Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::AlreadyApplied(record)) => {
            Ok(AutonomousDailyFinalizationOutcome::AlreadyFinalized(record))
        }
        Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::ConflictingTerminalTruth(_)) => {
            Ok(AutonomousDailyFinalizationOutcome::Conflict)
        }
        Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::IllegalTarget) => {
            anyhow::bail!(
                "finalize_with_commit_uncertainty: illegal finalization target for operation {}",
                operation.operation_id
            )
        }
        Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::NotFound)
        | Ok(mqk_db::FinalizeAutonomousDailyOperationOutcome::StaleState { .. })
        | Err(_) => {
            reread_confirm_finalized(
                pool,
                operation.operation_id,
                &target_state,
                &outcome_code,
                operation.state_version,
            )
            .await
        }
    }
}

/// Authoritative re-read after an ambiguous terminal-CAS write result.
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
/// UNCERTAINTY-CLOSURE (REPAIR 3): confirmation requires the exact
/// authorized target/outcome pair, `finalized_at_utc IS NOT NULL`,
/// `state_version` strictly greater than the *original* expected version
/// this attempt's own CAS used (a matching state/outcome at an unadvanced
/// version is not confirmed finalization -- it did not come from this or any
/// other real commit), and both `state_reason_code`/`state_blocker_signature`
/// null (complete terminal truth, mirroring the store-level
/// `is_complete_automatic_terminal_truth` check).
async fn reread_confirm_finalized(
    pool: &PgPool,
    operation_id: Uuid,
    expected_target_state: &str,
    expected_outcome: &str,
    expected_state_version: i64,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let record = match mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id).await {
        Ok(Some(record)) => record,
        Ok(None) | Err(_) => return Ok(AutonomousDailyFinalizationOutcome::DatabaseUnavailable),
    };

    if record.state == expected_target_state
        && record.outcome.as_deref() == Some(expected_outcome)
        && record.finalized_at_utc.is_some()
        && record.state_version > expected_state_version
        && record.state_reason_code.is_none()
        && record.state_blocker_signature.is_none()
    {
        return Ok(AutonomousDailyFinalizationOutcome::Finalized {
            outcome: expected_outcome.to_string(),
            record,
        });
    }

    if mqk_db::is_terminal_operation_state(&record.state) {
        return Ok(AutonomousDailyFinalizationOutcome::Conflict);
    }

    // Genuinely not yet finalized (e.g. a stale expected version raced with
    // another writer that has not yet reached a terminal state) -- this
    // attempt claims nothing; a future coordinator tick retries from
    // scratch.
    anyhow::bail!(
        "finalize_with_commit_uncertainty: operation {operation_id} not yet finalized after \
         re-read; retry on a future attempt"
    )
}

// ---------------------------------------------------------------------------
// E2B.17/E2B.18 -- evidence-degraded CAS + recovery-to-stopping
// ---------------------------------------------------------------------------

/// Persist (or confirm, or refresh) the nonterminal `evidence_degraded`
/// blocker for `reason`. `operation.state` must be `stopping`,
/// `stop_retrying`, or already `evidence_degraded` (a same-state refresh).
async fn apply_evidence_degraded_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason: AutonomousDailyUnknownReason,
    now_utc: DateTime<Utc>,
    effect_seam: AutonomousDailyFinalizationEffectSeam,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let signature = blocker_signature_for_unknown(operation, reason);
    let reason_code = signature.reason_code.to_string();
    let detail = bounded_detail(&format!(
        "autonomous daily outcome finalization evidence blocker: {reason_code}"
    ));

    // REPAIR 6: model a database that is also unavailable for the blocker-
    // write step itself -- skip the real write entirely rather than attempt
    // and discard it, so the re-read below is genuinely real and genuinely
    // observes that no write took place (never a fabricated "blocker
    // written" claim). Production never sets this field.
    if effect_seam.force_blocker_persistence_unavailable {
        return reread_confirm_evidence_degraded(pool, operation.operation_id, reason).await;
    }

    if operation.state == mqk_db::STATE_EVIDENCE_DEGRADED {
        let refresh_args = mqk_db::RefreshAutonomousDailyOperationBlockerArgs {
            operation_id: operation.operation_id,
            expected_state: operation.state.clone(),
            expected_state_version: operation.state_version,
            reason_code: bounded_text(&reason_code, 128),
            blocker_signature: signature
                .stable_context
                .clone()
                .map(|s| bounded_text(&s, 128)),
            occurred_at_utc: now_utc,
            bounded_detail: detail,
        };
        return match mqk_db::refresh_autonomous_daily_operation_blocker(pool, &refresh_args).await {
            Ok(mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::Applied(record)) => {
                Ok(AutonomousDailyFinalizationOutcome::EvidenceDegraded {
                    reason_code: reason,
                    record,
                    newly_applied: true,
                })
            }
            Ok(mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::AlreadyApplied(record)) => {
                Ok(AutonomousDailyFinalizationOutcome::EvidenceDegraded {
                    reason_code: reason,
                    record,
                    newly_applied: false,
                })
            }
            Ok(_) | Err(_) => {
                reread_confirm_evidence_degraded(pool, operation.operation_id, reason).await
            }
        };
    }

    let transition_args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_EVIDENCE_DEGRADED.to_string(),
        reason_code: Some(bounded_text(&reason_code, 128)),
        blocker_signature: signature
            .stable_context
            .clone()
            .map(|s| bounded_text(&s, 128)),
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: detail,
    };

    match mqk_db::transition_autonomous_daily_operation(pool, &transition_args).await {
        Ok(mqk_db::AutonomousDailyTransitionOutcome::Applied(record)) => {
            Ok(AutonomousDailyFinalizationOutcome::EvidenceDegraded {
                reason_code: reason,
                record,
                newly_applied: true,
            })
        }
        Ok(mqk_db::AutonomousDailyTransitionOutcome::AlreadyApplied(record)) => {
            Ok(AutonomousDailyFinalizationOutcome::EvidenceDegraded {
                reason_code: reason,
                record,
                newly_applied: false,
            })
        }
        Ok(_) | Err(_) => {
            reread_confirm_evidence_degraded(pool, operation.operation_id, reason).await
        }
    }
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
/// GATE-REPAIR-01: the shared eligibility gate this wrapper enforces before
/// ever calling [`apply_evidence_degraded_blocker`]. This is the same
/// eligibility truth [`check_finalization_eligibility`]/
/// `classify_and_finalize_autonomous_daily_operation_with_effect_seam`'s own
/// `STATE_EVIDENCE_DEGRADED` branch already apply on the successful-policy-
/// resolution path -- restated here so a policy-resolution-*failure* caller
/// can never bypass it. Persistence is permitted only when: no matching
/// local runtime is active, `stopped_at_utc` is durably present, and
/// `operation.state` is `stopping`, `stop_retrying`, or an already
/// `evidence_degraded` post-stop row being refreshed in place.
fn finalization_blocker_persistence_eligible(
    operation: &AutonomousDailyOperationRecord,
    context: &AutonomousDailyFinalizationContext,
) -> bool {
    if context.matching_local_runtime_active {
        return false;
    }
    if operation.stopped_at_utc.is_none() {
        return false;
    }
    matches!(
        operation.state.as_str(),
        mqk_db::STATE_STOPPING | mqk_db::STATE_STOP_RETRYING | mqk_db::STATE_EVIDENCE_DEGRADED
    )
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
/// INTEGRATION-AND-NOTIFICATION: the narrow, E2B-owned coordinator seam for
/// E3.4's policy-resolution-failure case. When the coordinator cannot build
/// [`AutonomousDailyFinalizationPolicyInputs`] at all (current
/// assignment/config/runtime-binding resolution itself failed), there is no
/// way to call [`classify_and_finalize_autonomous_daily_operation`] --
/// evidence gathering requires a real `config`/`binding`/`strategy_registry`
/// to even attempt. This thin wrapper reuses
/// [`apply_evidence_degraded_blocker`] verbatim (the same CAS/refresh/
/// authoritative-re-read machinery every other evidence blocker in this
/// module uses) so the coordinator never becomes a second, independent
/// blocker writer. Never call this when policy inputs resolved successfully
/// -- that case must go through the normal gather/classify pipeline.
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
/// GATE-REPAIR-01: `context` is required and checked via
/// [`finalization_blocker_persistence_eligible`] before any write is
/// attempted. This is deliberate defense-in-depth on top of the coordinator's
/// own early return in `handle_outcome_finalization` -- this seam must never
/// become a second way to bypass finalization eligibility, even if a future
/// caller forgets the coordinator-level check. An ineligible call returns
/// [`AutonomousDailyFinalizationOutcome::NotEligible`] with zero DB writes.
pub async fn persist_autonomous_daily_finalization_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
    reason: AutonomousDailyUnknownReason,
    context: AutonomousDailyFinalizationContext,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    if !finalization_blocker_persistence_eligible(operation, &context) {
        return Ok(AutonomousDailyFinalizationOutcome::NotEligible);
    }
    apply_evidence_degraded_blocker(
        pool,
        operation,
        reason,
        now_utc,
        AutonomousDailyFinalizationEffectSeam::default(),
    )
    .await
}

/// Authoritative re-read after an ambiguous evidence-degraded blocker write.
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
/// INTEGRATION-AND-NOTIFICATION: this path can never prove *this* attempt
/// was the one that durably advanced truth (the write result itself was
/// ambiguous) -- `newly_applied` is always `false` here, never `true`, per
/// the binding rule on [`AutonomousDailyFinalizationOutcome::EvidenceDegraded`].
async fn reread_confirm_evidence_degraded(
    pool: &PgPool,
    operation_id: Uuid,
    reason: AutonomousDailyUnknownReason,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    match mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id).await {
        Ok(Some(record))
            if record.state == mqk_db::STATE_EVIDENCE_DEGRADED
                && record.state_reason_code.as_deref() == Some(reason.as_str())
                && record.outcome.is_none()
                && record.finalized_at_utc.is_none() =>
        {
            Ok(AutonomousDailyFinalizationOutcome::EvidenceDegraded {
                reason_code: reason,
                record,
                newly_applied: false,
            })
        }
        _ => Ok(AutonomousDailyFinalizationOutcome::DatabaseUnavailable),
    }
}

/// Take the existing `evidence_degraded -> stopping` legal edge once
/// evidence now resolves cleanly (E2B.18). Never finalizes directly.
async fn recover_evidence_degraded_to_stopping(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_STOPPING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: bounded_detail(
            "autonomous daily outcome finalization evidence recovered; returning to stopping",
        ),
    };

    match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record)
        | mqk_db::AutonomousDailyTransitionOutcome::AlreadyApplied(record) => Ok(
            AutonomousDailyFinalizationOutcome::RecoveredToStopping(record),
        ),
        mqk_db::AutonomousDailyTransitionOutcome::StaleState { .. }
        | mqk_db::AutonomousDailyTransitionOutcome::NotFound
        | mqk_db::AutonomousDailyTransitionOutcome::IllegalTransition => {
            match mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation.operation_id)
                .await?
            {
                Some(record) if record.state == mqk_db::STATE_STOPPING => Ok(
                    AutonomousDailyFinalizationOutcome::RecoveredToStopping(record),
                ),
                _ => Ok(AutonomousDailyFinalizationOutcome::DatabaseUnavailable),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// E2B.20 -- high-level entry point
// ---------------------------------------------------------------------------

/// Classify and (if eligible and resolvable) finalize one autonomous daily
/// operation. Called from the coordinator's `handle_outcome_finalization`
/// helper (`autonomous_daily_coordinator.rs`, AUTONOMOUS-DAILY-PAPER-
/// OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION)
/// once per finalization-eligible tick. Always uses the all-default (no-op)
/// injected effect seam -- production behavior is unaffected by
/// [`AutonomousDailyFinalizationEffectSeam`]'s existence.
pub async fn classify_and_finalize_autonomous_daily_operation(
    pool: &PgPool,
    operation_id: Uuid,
    now_utc: DateTime<Utc>,
    context: AutonomousDailyFinalizationContext,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        pool,
        operation_id,
        now_utc,
        context,
        policy_inputs,
        AutonomousDailyFinalizationEffectSeam::default(),
    )
    .await
}

/// Test-support entry point, identical to
/// [`classify_and_finalize_autonomous_daily_operation`] except for one added
/// parameter: the REPAIR 5/6 narrow injected effect seam that lets a test
/// deterministically prove commit-uncertainty and partial-evidence-read-
/// failure behavior against a real database. The production entry point
/// above always calls this with [`AutonomousDailyFinalizationEffectSeam::default`],
/// so production behavior is identical to before this parameter existed.
pub async fn classify_and_finalize_autonomous_daily_operation_with_effect_seam(
    pool: &PgPool,
    operation_id: Uuid,
    now_utc: DateTime<Utc>,
    context: AutonomousDailyFinalizationContext,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
    effect_seam: AutonomousDailyFinalizationEffectSeam,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let operation = match mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id).await {
        Ok(Some(operation)) => operation,
        Ok(None) => anyhow::bail!(
            "classify_and_finalize_autonomous_daily_operation: no operation row for {operation_id}"
        ),
        Err(_) => return Ok(AutonomousDailyFinalizationOutcome::DatabaseUnavailable),
    };

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
    // UNCERTAINTY-CLOSURE (REPAIR 2): generic `completed` is read-only
    // manual/administrative terminal truth -- never rewritten or
    // reclassified, and never subjected to the automatic-row completeness
    // check below (it carries no `outcome` in the closed set by
    // construction).
    if operation.state == mqk_db::STATE_COMPLETED {
        return Ok(AutonomousDailyFinalizationOutcome::AlreadyFinalized(
            operation,
        ));
    }

    if mqk_db::is_terminal_operation_state(&operation.state) {
        // `completed_no_trade` / `completed_with_activity`: only a
        // *complete* durable terminal truth (exact authorized pair,
        // `finalized_at_utc` present, no residual reason/blocker fields) may
        // be reported as already finalized -- a malformed automatic
        // terminal row (e.g. hand-corrupted, or left mid-write by a crash
        // this patch's own CAS never permits in practice) is a `Conflict`,
        // never silently accepted as truth.
        return if mqk_db::is_complete_automatic_terminal_truth(&operation) {
            Ok(AutonomousDailyFinalizationOutcome::AlreadyFinalized(
                operation,
            ))
        } else {
            Ok(AutonomousDailyFinalizationOutcome::Conflict)
        };
    }

    if operation.state == mqk_db::STATE_EVIDENCE_DEGRADED {
        if context.matching_local_runtime_active || operation.stopped_at_utc.is_none() {
            return Ok(AutonomousDailyFinalizationOutcome::NotEligible);
        }
        return run_gather_classify_and_persist(
            pool,
            &operation,
            policy_inputs,
            now_utc,
            effect_seam,
        )
        .await;
    }

    match check_finalization_eligibility(&operation, &context) {
        AutonomousDailyFinalizationEligibility::NotEligible
        | AutonomousDailyFinalizationEligibility::RuntimeStopUnproven => {
            Ok(AutonomousDailyFinalizationOutcome::NotEligible)
        }
        AutonomousDailyFinalizationEligibility::Eligible => {
            run_gather_classify_and_persist(pool, &operation, policy_inputs, now_utc, effect_seam)
                .await
        }
    }
}

/// Shared gather+classify+persist pipeline for both the `stopping`/
/// `stop_retrying` (finalize-or-degrade) and `evidence_degraded`
/// (recover-or-refresh) entry states.
async fn run_gather_classify_and_persist(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
    now_utc: DateTime<Utc>,
    effect_seam: AutonomousDailyFinalizationEffectSeam,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    let snapshot =
        match gather_autonomous_daily_outcome_evidence(pool, operation, policy_inputs, effect_seam)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return apply_evidence_degraded_blocker(
                    pool,
                    operation,
                    AutonomousDailyUnknownReason::DatabaseUnavailable,
                    now_utc,
                    effect_seam,
                )
                .await;
            }
        };

    match classify_autonomous_daily_outcome(&snapshot) {
        AutonomousDailyOutcomeClassification::CompletedWithActivity { reason_code, .. }
        | AutonomousDailyOutcomeClassification::CompletedNoTrade { reason_code, .. } => {
            if operation.state == mqk_db::STATE_EVIDENCE_DEGRADED {
                recover_evidence_degraded_to_stopping(pool, operation, now_utc).await
            } else {
                finalize_with_commit_uncertainty(pool, operation, reason_code, now_utc, effect_seam)
                    .await
            }
        }
        AutonomousDailyOutcomeClassification::EvidenceBlocked { reason_code, .. } => {
            apply_evidence_degraded_blocker(pool, operation, reason_code, now_utc, effect_seam)
                .await
        }
    }
}
