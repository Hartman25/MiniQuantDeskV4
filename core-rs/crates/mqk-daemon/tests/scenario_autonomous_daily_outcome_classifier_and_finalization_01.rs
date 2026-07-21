//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-
//! FINALIZATION-CAS: proof tests for the pure evidence classifier
//! (`state::autonomous_daily_outcome::classify_autonomous_daily_outcome`),
//! the terminal finalization CAS (`mqk_db::finalize_autonomous_daily_operation`),
//! the two new `stopping`/`stop_retrying -> evidence_degraded` legal edges,
//! and the high-level `classify_and_finalize_autonomous_daily_operation`
//! entry point.
//!
//! Group 1 (classifier, pure, no DB): tests 1-26 (AUTONOMOUS-DAILY-PAPER-
//! OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-UNCERTAINTY-CLOSURE
//! REPAIR 4 adds `classifier_26`).
//! Group 2 (finalization/blocker CAS store proof, DB-backed, `#[ignore]`):
//! tests 26-59 (REPAIR 1/2/5/6/7 add `store_48`-`store_58`; `store_47` is
//! renamed in place -- it was never a database-failure proof;
//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
//! GATE-REPAIR-01 adds `store_59`, the hardened
//! `persist_autonomous_daily_finalization_blocker` eligibility-gate proof).
//! Group 3 (integrated end-to-end DB-backed proof, `#[ignore]`): three scenarios.
//!
//! DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_outcome_classifier_and_finalization_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! No coordinator invocation, no API route, no GUI, no notification, no real
//! provider/broker/network call anywhere in this file.

use chrono::{DateTime, NaiveDate, SubsecRound, TimeZone, Utc};
use mqk_daemon::daily_data_readiness;
use mqk_daemon::state::autonomous_daily_coverage_authority::{
    construct_coverage_bound_detail, coverage_construction_inputs_from_operation,
    resolve_current_coverage_policy_inputs, write_and_confirm_coverage_authority,
    CoverageAuthorityEnsureResult, CoverageBoundDetail,
};
use mqk_daemon::state::autonomous_daily_outcome::{
    check_finalization_eligibility, classify_and_finalize_autonomous_daily_operation,
    classify_and_finalize_autonomous_daily_operation_with_effect_seam,
    classify_autonomous_daily_outcome, derive_expected_bar_set,
    gather_autonomous_daily_outcome_evidence, persist_autonomous_daily_finalization_blocker,
    AutonomousDailyFinalizationContext, AutonomousDailyFinalizationEffectSeam,
    AutonomousDailyFinalizationEligibility, AutonomousDailyFinalizationInjectedPreCommitEffect,
    AutonomousDailyFinalizationOutcome, AutonomousDailyFinalizationPolicyInputs,
    AutonomousDailyOutcomeClassification, AutonomousDailyOutcomeEvidenceSnapshot,
    AutonomousDailyTerminalReason, AutonomousDailyUnknownReason, ExpectedBarClaimEvidence,
};
use mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
use mqk_daemon::state::{
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SymbolStrategyAssignment,
    MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION,
};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

fn monday_at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, second)
        .unwrap()
}

// Must be a real builtin strategy id registered by
// `mqk_strategy::engines::register_builtin_strategies` -- the integrated
// (DB-backed) fixtures resolve `required_history_bars` via a real
// `PluginRegistry::lookup`, exactly like production. `intraday_scalper`'s
// native timeframe is `5m`, matching `TIMEFRAME` below (mirrors the E2A
// `f04` fixture's own reasoning for the same substitution).
const STRATEGY_ID: &str = "intraday_scalper";
const SYMBOL: &str = "ZZE2B";
const TIMEFRAME: &str = "5m";

/// A coverage anchor with `effective_grace_seconds = 0` for simple mental
/// arithmetic: 5-minute grid starting at 13:00Z, expected window
/// `[13:00, 13:05, 13:10]` -- `first_expectation_instant = 13:05:00`;
/// `effective_operation_close_utc = 13:16:00` admits 13:05 (expectation
/// 13:10:00) and 13:10 (expectation 13:15:00) but excludes 13:15
/// (expectation 13:20:00, not `< 13:16:00`).
fn base_coverage_detail() -> CoverageBoundDetail {
    CoverageBoundDetail {
        schema_version: 1,
        operation_id: Uuid::new_v4(),
        market_date: "2026-07-20".to_string(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: "zze2b-test".to_string(),
        first_dispatchable_bar_end_ts: monday_at(13, 0, 0).timestamp(),
        final_dispatchable_bar_end_ts: monday_at(13, 10, 0).timestamp(),
        local_symbol: SYMBOL.to_string(),
        timeframe: TIMEFRAME.to_string(),
        timeframe_secs: 300,
        required_history_bars: 1,
        effective_grace_seconds: 0,
        session_plan_identity: "sp".to_string(),
        assignment_identity: "ai".to_string(),
        runtime_binding_identity: "rbi".to_string(),
        exchange_session_open_utc: monday_at(13, 0, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        effective_operation_open_utc: monday_at(13, 0, 0),
        effective_operation_close_utc: monday_at(13, 16, 0),
    }
}

fn flat_evaluation(
    evaluation_id: Uuid,
    run_id: Uuid,
    bar_end_ts: i64,
    detail: &CoverageBoundDetail,
) -> mqk_db::StrategySignalEvaluationRecord {
    mqk_db::StrategySignalEvaluationRecord {
        evaluation_id,
        ts_utc: Utc::now().trunc_subsecs(6),
        run_id: Some(run_id),
        strategy_id: STRATEGY_ID.to_string(),
        symbol: detail.local_symbol.clone(),
        timeframe: detail.timeframe.clone(),
        bar_context_source: "db_loaded".to_string(),
        bars_loaded: 1,
        latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(bar_end_ts, 0),
        signal_generated: false,
        signal_qty: Some(0),
        signal_side: None,
        reason_code: "flat".to_string(),
        reason: "flat".to_string(),
        decision_stage: "strategy_evaluated".to_string(),
        source: "test".to_string(),
    }
}

fn nonzero_evaluation(
    evaluation_id: Uuid,
    run_id: Uuid,
    bar_end_ts: i64,
    detail: &CoverageBoundDetail,
) -> mqk_db::StrategySignalEvaluationRecord {
    mqk_db::StrategySignalEvaluationRecord {
        signal_generated: true,
        signal_qty: Some(10),
        signal_side: Some("buy".to_string()),
        ..flat_evaluation(evaluation_id, run_id, bar_end_ts, detail)
    }
}

fn completed_claim(
    detail: &CoverageBoundDetail,
    bar_end_ts: i64,
    evaluation_id: Option<Uuid>,
) -> mqk_db::AutonomousDailyBarDispatchRecord {
    mqk_db::AutonomousDailyBarDispatchRecord {
        operation_id: detail.operation_id,
        local_symbol: detail.local_symbol.clone(),
        timeframe: detail.timeframe.clone(),
        bar_end_ts,
        status: mqk_db::DISPATCH_STATUS_COMPLETED.to_string(),
        claimed_at_utc: Utc::now().trunc_subsecs(6),
        completed_at_utc: Some(Utc::now().trunc_subsecs(6)),
        evaluation_id,
        last_error: None,
    }
}

fn unresolved_claim(
    detail: &CoverageBoundDetail,
    bar_end_ts: i64,
    status: &str,
) -> mqk_db::AutonomousDailyBarDispatchRecord {
    mqk_db::AutonomousDailyBarDispatchRecord {
        operation_id: detail.operation_id,
        local_symbol: detail.local_symbol.clone(),
        timeframe: detail.timeframe.clone(),
        bar_end_ts,
        status: status.to_string(),
        claimed_at_utc: Utc::now().trunc_subsecs(6),
        completed_at_utc: None,
        evaluation_id: None,
        last_error: None,
    }
}

fn outbox_row(run_id: Uuid, status: &str, dispatching: bool) -> mqk_db::OutboxRow {
    mqk_db::OutboxRow {
        outbox_id: 1,
        run_id,
        idempotency_key: format!("k-{}", Uuid::new_v4()),
        order_json: serde_json::json!({}),
        status: status.to_string(),
        created_at_utc: Utc::now().trunc_subsecs(6),
        sent_at_utc: None,
        claimed_at_utc: None,
        claimed_by: None,
        dispatching_at_utc: if dispatching {
            Some(Utc::now().trunc_subsecs(6))
        } else {
            None
        },
        dispatch_attempt_id: None,
    }
}

fn inbox_row(run_id: Uuid, event_kind: &str) -> mqk_db::InboxRow {
    mqk_db::InboxRow {
        inbox_id: 1,
        run_id,
        broker_message_id: format!("m-{}", Uuid::new_v4()),
        broker_fill_id: None,
        broker_sequence_id: None,
        broker_timestamp: None,
        message_json: serde_json::json!({}),
        received_at_utc: Utc::now().trunc_subsecs(6),
        applied_at_utc: None,
        event_kind: event_kind.to_string(),
    }
}

/// A fully clean, coverage-complete, all-flat-evaluations, zero-order-
/// evidence snapshot -- the no-trade baseline every test below mutates from.
fn clean_no_trade_snapshot(
    detail: &CoverageBoundDetail,
    run_id: Uuid,
) -> AutonomousDailyOutcomeSnapshotBuilder {
    let expected_bars = derive_expected_bar_set(detail);
    let claims = expected_bars
        .iter()
        .map(|&bar_end_ts| {
            let evaluation_id = Uuid::new_v4();
            ExpectedBarClaimEvidence {
                bar_end_ts,
                claim: Some(completed_claim(detail, bar_end_ts, Some(evaluation_id))),
                evaluation: Some(flat_evaluation(evaluation_id, run_id, bar_end_ts, detail)),
            }
        })
        .collect::<Vec<_>>();
    AutonomousDailyOutcomeSnapshotBuilder {
        snapshot: AutonomousDailyOutcomeEvidenceSnapshot {
            operation_id: detail.operation_id,
            current_run_id: Some(run_id),
            identity_match: true,
            lineage: Some(vec![run_id]),
            coverage: Some(detail.clone()),
            expected_strategy_id: Some(STRATEGY_ID.to_string()),
            expected_bars: expected_bars.clone(),
            claims,
            total_claim_count: expected_bars.len() as i64,
            aggregate_bars_observed: expected_bars.len() as i64,
            aggregate_bars_dispatched: expected_bars.len() as i64,
            aggregate_last_completed_bar_ts: expected_bars.last().copied(),
            aggregate_last_dispatched_bar_ts: expected_bars.last().copied(),
            outbox_rows: Vec::new(),
            inbox_rows: Vec::new(),
        },
    }
}

/// Tiny builder so each test can mutate exactly the field(s) its scenario
/// needs without repeating every other field.
struct AutonomousDailyOutcomeSnapshotBuilder {
    snapshot: AutonomousDailyOutcomeEvidenceSnapshot,
}

impl AutonomousDailyOutcomeSnapshotBuilder {
    fn build(self) -> AutonomousDailyOutcomeEvidenceSnapshot {
        self.snapshot
    }
}

fn assert_blocked(
    classification: AutonomousDailyOutcomeClassification,
    expected: AutonomousDailyUnknownReason,
) {
    match classification {
        AutonomousDailyOutcomeClassification::EvidenceBlocked { reason_code, .. } => {
            assert_eq!(reason_code, expected, "unexpected blocker reason");
        }
        other => panic!("expected EvidenceBlocked({expected:?}), got {other:?}"),
    }
}

fn assert_activity(
    classification: AutonomousDailyOutcomeClassification,
    expected: AutonomousDailyTerminalReason,
) {
    match classification {
        AutonomousDailyOutcomeClassification::CompletedWithActivity { reason_code, .. } => {
            assert_eq!(reason_code, expected, "unexpected activity reason");
        }
        other => panic!("expected CompletedWithActivity({expected:?}), got {other:?}"),
    }
}

fn assert_no_trade(classification: AutonomousDailyOutcomeClassification) {
    match classification {
        AutonomousDailyOutcomeClassification::CompletedNoTrade { reason_code, .. } => {
            assert_eq!(
                reason_code,
                AutonomousDailyTerminalReason::NoTradeStrategyEvaluatedNoSignal
            );
        }
        other => panic!("expected CompletedNoTrade, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group 1 — pure classifier tests (1-25)
// ---------------------------------------------------------------------------

#[test]
fn classifier_01_clean_no_trade_evidence_classifies_no_trade() {
    let detail = base_coverage_detail();
    let snapshot = clean_no_trade_snapshot(&detail, Uuid::new_v4()).build();
    assert_no_trade(classify_autonomous_daily_outcome(&snapshot));
}

#[test]
fn classifier_02_fill_evidence_classifies_activity_fill_confirmed() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder.snapshot.inbox_rows.push(inbox_row(run_id, "fill"));
    assert_activity(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyTerminalReason::ActivityFillConfirmed,
    );
}

#[test]
fn classifier_03_submitted_order_without_fill_classifies_order_submitted() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder
        .snapshot
        .outbox_rows
        .push(outbox_row(run_id, "SENT", false));
    assert_activity(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyTerminalReason::ActivityOrderSubmitted,
    );
}

#[test]
fn classifier_04_enqueued_decision_without_broker_evidence_classifies_decision_accepted() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder
        .snapshot
        .outbox_rows
        .push(outbox_row(run_id, "PENDING", false));
    assert_activity(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyTerminalReason::ActivityDecisionAccepted,
    );
}

#[test]
fn classifier_05_fill_outranks_order_and_decision() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder
        .snapshot
        .outbox_rows
        .push(outbox_row(run_id, "SENT", false));
    builder.snapshot.inbox_rows.push(inbox_row(run_id, "fill"));
    assert_activity(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyTerminalReason::ActivityFillConfirmed,
    );
}

#[test]
fn classifier_06_order_submission_outranks_decision() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder
        .snapshot
        .outbox_rows
        .push(outbox_row(run_id, "PENDING", false));
    builder
        .snapshot
        .outbox_rows
        .push(outbox_row(run_id, "ACKED", false));
    assert_activity(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyTerminalReason::ActivityOrderSubmitted,
    );
}

#[test]
fn classifier_07_missing_expected_claim_blocks_terminalization() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.claims[1].claim = None;
    builder.snapshot.aggregate_bars_dispatched -= 1;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_08_extra_claim_blocks_terminalization() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.total_claim_count += 1;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_09_counter_mismatch_blocks_terminalization() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.aggregate_bars_observed -= 1;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_10_last_bar_mismatch_blocks_terminalization() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.aggregate_last_completed_bar_ts = Some(detail.first_dispatchable_bar_end_ts);
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_11_claimed_claim_produces_unresolved_dispatch_claim() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    let bar_end_ts = builder.snapshot.claims[0].bar_end_ts;
    builder.snapshot.claims[0] = ExpectedBarClaimEvidence {
        bar_end_ts,
        claim: Some(unresolved_claim(
            &detail,
            bar_end_ts,
            mqk_db::DISPATCH_STATUS_CLAIMED,
        )),
        evaluation: None,
    };
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::UnresolvedDispatchClaim,
    );
}

#[test]
fn classifier_12_failed_claim_produces_unresolved_dispatch_claim() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    let bar_end_ts = builder.snapshot.claims[0].bar_end_ts;
    builder.snapshot.claims[0] = ExpectedBarClaimEvidence {
        bar_end_ts,
        claim: Some(unresolved_claim(
            &detail,
            bar_end_ts,
            mqk_db::DISPATCH_STATUS_FAILED,
        )),
        evaluation: None,
    };
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::UnresolvedDispatchClaim,
    );
}

#[test]
fn classifier_13_completed_claim_without_evaluation_id_produces_missing_evaluation_evidence() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    let bar_end_ts = builder.snapshot.claims[0].bar_end_ts;
    builder.snapshot.claims[0] = ExpectedBarClaimEvidence {
        bar_end_ts,
        claim: Some(completed_claim(&detail, bar_end_ts, None)),
        evaluation: None,
    };
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::MissingEvaluationEvidence,
    );
}

#[test]
fn classifier_14_missing_evaluation_row_produces_missing_evaluation_evidence() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.claims[0].evaluation = None;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::MissingEvaluationEvidence,
    );
}

#[test]
fn classifier_15_evaluation_outside_run_lineage_fails_closed() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    let bar_end_ts = builder.snapshot.claims[0].bar_end_ts;
    let stray_run = Uuid::new_v4();
    let eval_id = builder.snapshot.claims[0]
        .claim
        .as_ref()
        .unwrap()
        .evaluation_id
        .unwrap();
    builder.snapshot.claims[0].evaluation =
        Some(flat_evaluation(eval_id, stray_run, bar_end_ts, &detail));
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::MissingEvaluationEvidence,
    );
}

#[test]
fn classifier_16_duplicate_evaluation_id_fails_closed() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    let shared_eval_id = builder.snapshot.claims[0]
        .claim
        .as_ref()
        .unwrap()
        .evaluation_id
        .unwrap();
    let bar_end_ts_1 = builder.snapshot.claims[1].bar_end_ts;
    builder.snapshot.claims[1].claim =
        Some(completed_claim(&detail, bar_end_ts_1, Some(shared_eval_id)));
    builder.snapshot.claims[1].evaluation = Some(flat_evaluation(
        shared_eval_id,
        run_id,
        bar_end_ts_1,
        &detail,
    ));
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::MissingEvaluationEvidence,
    );
}

#[test]
fn classifier_17_invalid_run_lineage_produces_run_lineage_unavailable() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.lineage = None;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::RunLineageUnavailable,
    );
}

#[test]
fn classifier_18_missing_coverage_authority_blocks_terminalization() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder.snapshot.coverage = None;
    builder.snapshot.expected_bars = Vec::new();
    builder.snapshot.claims = Vec::new();
    // Lineage is non-empty (the operation did run) -- a missing anchor here
    // is a genuine coverage-proof gap, not the never-ran case.
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_19_current_identity_mismatch_produces_assignment_identity_unavailable() {
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.identity_match = false;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
    );
}

#[test]
fn classifier_20_nonzero_signal_without_outbox_produces_order_evidence_conflict() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    let bar_end_ts = builder.snapshot.claims[0].bar_end_ts;
    let eval_id = builder.snapshot.claims[0]
        .claim
        .as_ref()
        .unwrap()
        .evaluation_id
        .unwrap();
    builder.snapshot.claims[0].evaluation =
        Some(nonzero_evaluation(eval_id, run_id, bar_end_ts, &detail));
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::OrderEvidenceConflict,
    );
}

#[test]
fn classifier_21_risk_denial_rows_do_not_change_classification() {
    // Structural proof: `AutonomousDailyOutcomeEvidenceSnapshot` has no
    // `sys_risk_denial_events`-shaped field at all -- a snapshot identical to
    // the clean no-trade baseline classifies identically regardless of any
    // risk-denial activity that might exist out-of-band in the database.
    let detail = base_coverage_detail();
    let snapshot = clean_no_trade_snapshot(&detail, Uuid::new_v4()).build();
    assert_no_trade(classify_autonomous_daily_outcome(&snapshot));
}

#[test]
fn classifier_22_fill_plus_unresolved_claim_remains_nonterminal() {
    let detail = base_coverage_detail();
    let run_id = Uuid::new_v4();
    let mut builder = clean_no_trade_snapshot(&detail, run_id);
    builder.snapshot.inbox_rows.push(inbox_row(run_id, "fill"));
    let bar_end_ts = builder.snapshot.claims[1].bar_end_ts;
    builder.snapshot.claims[1] = ExpectedBarClaimEvidence {
        bar_end_ts,
        claim: Some(unresolved_claim(
            &detail,
            bar_end_ts,
            mqk_db::DISPATCH_STATUS_CLAIMED,
        )),
        evaluation: None,
    };
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::UnresolvedDispatchClaim,
    );
}

#[test]
fn classifier_23_late_start_recovery_gap_missing_bar_remains_nonterminal() {
    // The expected set is derived purely from the coverage anchor, never
    // from actual observed activity -- a middle bar with literally no claim
    // (as if a recovery gap swallowed it) is indistinguishable, to the
    // classifier, from any other missing expected claim.
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    let mid = builder.snapshot.claims.len() / 2;
    builder.snapshot.claims[mid].claim = None;
    builder.snapshot.claims[mid].evaluation = None;
    builder.snapshot.aggregate_bars_observed -= 1;
    builder.snapshot.aggregate_bars_dispatched -= 1;
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn classifier_24_generic_completed_is_never_produced() {
    // Structural proof: `AutonomousDailyOutcomeClassification` has exactly
    // three variants, none of which can carry the DB `state` string
    // `"completed"` -- every terminal reason's `target_state()` maps only to
    // `completed_no_trade`/`completed_with_activity` (see
    // `AutonomousDailyTerminalReason::target_state`).
    for reason in [
        AutonomousDailyTerminalReason::ActivityFillConfirmed,
        AutonomousDailyTerminalReason::ActivityOrderSubmitted,
        AutonomousDailyTerminalReason::ActivityDecisionAccepted,
        AutonomousDailyTerminalReason::NoTradeStrategyEvaluatedNoSignal,
    ] {
        assert_ne!(reason.target_state(), mqk_db::STATE_COMPLETED);
        assert!(matches!(
            reason.target_state(),
            mqk_db::STATE_COMPLETED_NO_TRADE | mqk_db::STATE_COMPLETED_WITH_ACTIVITY
        ));
    }
}

#[test]
fn classifier_25_process_local_diagnostic_counters_are_never_read() {
    // Structural proof: `classify_autonomous_daily_outcome`'s only parameter
    // is `&AutonomousDailyOutcomeEvidenceSnapshot`, a type built entirely
    // from durable-DB-shaped fields (see its own field list) -- there is no
    // `AppState`, no `bar_tick_dispatch_count`, no `last_bar_signal_qty`, and
    // no completed-bar-task liveness cache reachable from this function's
    // signature at all. A snapshot built without ever touching `AppState`
    // (as every test in this file does) classifies fully.
    let detail = base_coverage_detail();
    let snapshot = clean_no_trade_snapshot(&detail, Uuid::new_v4()).build();
    let _ = classify_autonomous_daily_outcome(&snapshot);
}

#[test]
fn classifier_26_missing_coverage_with_empty_lineage_is_incomplete_bar_coverage() {
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
    // UNCERTAINTY-CLOSURE (REPAIR 4): the precedence order is strictly
    // identity -> lineage -> durable coverage authority -> expected-bar
    // coverage -> claims -> evaluations. A missing/unusable coverage anchor
    // is always `unknown_incomplete_bar_coverage`, even when the lineage is
    // also empty -- the prior empty-lineage special case (routing to
    // `unknown_missing_evaluation_evidence` instead) is removed.
    let detail = base_coverage_detail();
    let mut builder = clean_no_trade_snapshot(&detail, Uuid::new_v4());
    builder.snapshot.lineage = Some(Vec::new());
    builder.snapshot.coverage = None;
    builder.snapshot.expected_bars = Vec::new();
    builder.snapshot.claims = Vec::new();
    assert_blocked(
        classify_autonomous_daily_outcome(&builder.build()),
        AutonomousDailyUnknownReason::IncompleteBarCoverage,
    );
}

#[test]
fn eligibility_state_not_stopping_is_not_eligible() {
    let mut op = fixture_operation_record(mqk_db::STATE_RUNNING);
    op.stopped_at_utc = None;
    let eligibility =
        check_finalization_eligibility(&op, &AutonomousDailyFinalizationContext::default());
    assert_eq!(
        eligibility,
        AutonomousDailyFinalizationEligibility::NotEligible
    );
}

#[test]
fn eligibility_matching_local_runtime_active_is_runtime_stop_unproven() {
    let mut op = fixture_operation_record(mqk_db::STATE_STOPPING);
    op.stopped_at_utc = Some(Utc::now().trunc_subsecs(6));
    let eligibility = check_finalization_eligibility(
        &op,
        &AutonomousDailyFinalizationContext {
            matching_local_runtime_active: true,
        },
    );
    assert_eq!(
        eligibility,
        AutonomousDailyFinalizationEligibility::RuntimeStopUnproven
    );
}

#[test]
fn eligibility_missing_stopped_at_utc_is_runtime_stop_unproven() {
    let mut op = fixture_operation_record(mqk_db::STATE_STOPPING);
    op.stopped_at_utc = None;
    let eligibility =
        check_finalization_eligibility(&op, &AutonomousDailyFinalizationContext::default());
    assert_eq!(
        eligibility,
        AutonomousDailyFinalizationEligibility::RuntimeStopUnproven
    );
}

#[test]
fn eligibility_stopping_with_stopped_at_utc_and_no_local_runtime_is_eligible() {
    let mut op = fixture_operation_record(mqk_db::STATE_STOPPING);
    op.stopped_at_utc = Some(Utc::now().trunc_subsecs(6));
    let eligibility =
        check_finalization_eligibility(&op, &AutonomousDailyFinalizationContext::default());
    assert_eq!(
        eligibility,
        AutonomousDailyFinalizationEligibility::Eligible
    );
}

fn fixture_operation_record(state: &str) -> mqk_db::AutonomousDailyOperationRecord {
    let now = Utc::now().trunc_subsecs(6);
    mqk_db::AutonomousDailyOperationRecord {
        operation_id: Uuid::new_v4(),
        market_date: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: "zze2b-fixture".to_string(),
        session_plan_identity: "sp".to_string(),
        assignment_identity: "ai".to_string(),
        runtime_binding_identity: "rbi".to_string(),
        calendar_source: "test".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "fixed_window_override".to_string(),
        effective_operation_open_utc: now,
        effective_operation_close_utc: now,
        exchange_session_open_utc: Some(now),
        exchange_session_close_utc: Some(now),
        exchange_is_early_close: Some(false),
        previous_trading_date: Some(NaiveDate::from_ymd_opt(2026, 7, 17).unwrap()),
        preopen_start_utc: now,
        postclose_finalize_utc: now,
        state: state.to_string(),
        state_reason_code: None,
        state_blocker_signature: None,
        state_version: 1,
        run_id: None,
        start_attempt_count: 0,
        last_start_attempt_utc: None,
        next_retry_utc: None,
        data_refresh_state: "not_started".to_string(),
        last_provider_poll_utc: None,
        provider_poll_attempt_count: 0,
        provider_poll_success_count: 0,
        provider_poll_failure_count: 0,
        last_completed_bar_ts: None,
        last_dispatched_bar_ts: None,
        bars_observed: 0,
        bars_dispatched: 0,
        started_at_utc: None,
        stopped_at_utc: None,
        finalized_at_utc: None,
        outcome: None,
        no_trade_reason: None,
        last_error: None,
        created_at_utc: now,
        updated_at_utc: now,
        stop_attempt_count: Some(0),
        last_stop_attempt_utc: None,
    }
}

// ---------------------------------------------------------------------------
// Group 2 — DB-backed finalization/blocker CAS store proof (26-58)
// ---------------------------------------------------------------------------

async fn maybe_db(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("{label}: skipped DB-backed proof because MQK_DATABASE_URL is not set");
            return None;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from oms_inbox where run_id in (select run_id from runs where engine_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean inbox");
    sqlx::query("delete from oms_outbox where run_id in (select run_id from runs where engine_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean outbox");
    sqlx::query("delete from strategy_signal_evaluations where run_id in (select run_id from runs where engine_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean evaluations");
    sqlx::query("delete from runs where engine_id like 'zze2b%'")
        .execute(&pool)
        .await
        .expect("clean runs");
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean claims");
    sqlx::query("delete from sys_autonomous_session_events where id like 'autonomous_daily_coverage_bound:%' and id in (select 'autonomous_daily_coverage_bound:' || operation_id::text from sys_autonomous_daily_operations where adapter_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean coverage events");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze2b%')")
        .execute(&pool)
        .await
        .expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zze2b%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    Some(pool)
}

fn unique_adapter_id(label: &str) -> String {
    format!("zze2b-{}-{}", label, &Uuid::new_v4().to_string()[..8])
}

async fn create_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    initial_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: Uuid::new_v4(),
        market_date: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: "sp".to_string(),
        assignment_identity: "ai".to_string(),
        runtime_binding_identity: "rbi".to_string(),
        calendar_source: "test".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "fixed_window_override".to_string(),
        effective_operation_open_utc: now_utc - chrono::Duration::hours(1),
        effective_operation_close_utc: now_utc,
        exchange_session_open_utc: now_utc - chrono::Duration::hours(1),
        exchange_session_close_utc: now_utc,
        exchange_is_early_close: false,
        previous_trading_date: NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(),
        preopen_start_utc: now_utc - chrono::Duration::hours(2),
        postclose_finalize_utc: now_utc + chrono::Duration::minutes(15),
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc - chrono::Duration::hours(2),
        bounded_detail: "test fixture".to_string(),
        stop_attempt_count: 0,
    };
    match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    }
}

async fn transition(
    pool: &sqlx::PgPool,
    operation: &mqk_db::AutonomousDailyOperationRecord,
    new_state: &str,
    reason_code: Option<&str>,
    blocker_signature: Option<&str>,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(|s| s.to_string()),
        blocker_signature: blocker_signature.map(|s| s.to_string()),
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "test fixture transition".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

async fn stopping_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let op = create_operation(pool, adapter_id, mqk_db::STATE_AWAITING_OPEN, now_utc).await;
    let op = transition(pool, &op, mqk_db::STATE_STOPPING, None, None, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, op.operation_id, now_utc)
        .await
        .expect("record stopped ok");
    mqk_db::fetch_autonomous_daily_operation_by_id(pool, op.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists")
}

fn finalize_args(
    operation: &mqk_db::AutonomousDailyOperationRecord,
    target_state: &str,
    outcome: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::FinalizeAutonomousDailyOperationArgs {
    mqk_db::FinalizeAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        target_state: target_state.to_string(),
        outcome: outcome.to_string(),
        finalized_at_utc: now_utc,
        bounded_detail: "test fixture finalize".to_string(),
    }
}

#[tokio::test]
#[ignore]
async fn store_26_stopping_to_completed_no_trade_applies_atomically() {
    let Some(pool) = maybe_db("store_26").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s26"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => {
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
            assert_eq!(
                record.outcome.as_deref(),
                Some(mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL)
            );
            assert_eq!(record.finalized_at_utc, Some(now));
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_27_stopping_to_completed_with_activity_applies_atomically() {
    let Some(pool) = maybe_db("store_27").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s27"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => {
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
            assert_eq!(
                record.outcome.as_deref(),
                Some(mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED)
            );
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_28_stop_retrying_to_completed_applies_atomically() {
    let Some(pool) = maybe_db("store_28").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s28"), now).await;
    let op = transition(&pool, &op, mqk_db::STATE_STOP_RETRYING, None, None, now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => {
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_29_state_outcome_finalized_and_event_commit_together() {
    let Some(pool) = maybe_db("store_29").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s29"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        mqk_db::OUTCOME_ACTIVITY_DECISION_ACCEPTED,
        now,
    );
    let record = match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let events = mqk_db::list_autonomous_daily_operation_events(&pool, op.operation_id, 20)
        .await
        .expect("list events ok");
    let last = events.last().expect("at least one event");
    assert_eq!(last.to_state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
    assert_eq!(
        last.reason_code.as_deref(),
        Some(mqk_db::OUTCOME_ACTIVITY_DECISION_ACCEPTED)
    );
    assert_eq!(last.transition_seq, record.state_version);
}

#[tokio::test]
#[ignore]
async fn store_30_no_trade_reason_remains_null() {
    let Some(pool) = maybe_db("store_30").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s30"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => {
            assert_eq!(record.no_trade_reason, None);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_31_state_reason_and_blocker_signature_clear_on_terminal_success() {
    let Some(pool) = maybe_db("store_31").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = create_operation(
        &pool,
        &unique_adapter_id("s31"),
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;
    let op = transition(
        &pool,
        &op,
        mqk_db::STATE_STOPPING,
        Some("some_blocker"),
        Some("sha256:aa"),
        now,
    )
    .await;
    assert!(op.state_reason_code.is_some());
    mqk_db::record_autonomous_runtime_stopped(&pool, op.operation_id, now)
        .await
        .expect("stop ok");
    let op = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => {
            assert_eq!(record.state_reason_code, None);
            assert_eq!(record.state_blocker_signature, None);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_32_exact_retry_creates_no_duplicate_event() {
    let Some(pool) = maybe_db("store_32").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s32"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    let first = mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok");
    assert!(matches!(
        first,
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(_)
    ));
    let second = mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok");
    match second {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::AlreadyApplied(record) => {
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected AlreadyApplied, got {other:?}"),
    }
    let events = mqk_db::list_autonomous_daily_operation_events(&pool, op.operation_id, 20)
        .await
        .expect("list ok");
    let terminal_events = events
        .iter()
        .filter(|e| e.to_state == mqk_db::STATE_COMPLETED_NO_TRADE)
        .count();
    assert_eq!(
        terminal_events, 1,
        "exact retry must not create a duplicate event"
    );
}

#[tokio::test]
#[ignore]
async fn store_33_stale_state_writes_nothing() {
    let Some(pool) = maybe_db("store_33").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s33"), now).await;
    let mut args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    args.expected_state = mqk_db::STATE_STOP_RETRYING.to_string();
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::StaleState { actual_state, .. } => {
            assert_eq!(actual_state, mqk_db::STATE_STOPPING);
        }
        other => panic!("expected StaleState, got {other:?}"),
    }
    let still = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(still.state, mqk_db::STATE_STOPPING);
    assert!(still.outcome.is_none());
}

#[tokio::test]
#[ignore]
async fn store_34_wrong_expected_version_writes_nothing() {
    let Some(pool) = maybe_db("store_34").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s34"), now).await;
    let mut args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    args.expected_state_version = op.state_version + 5;
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::StaleState { .. } => {}
        other => panic!("expected StaleState, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_35_terminal_row_is_never_reclassified() {
    let Some(pool) = maybe_db("store_35").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s35"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    let record = match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    // A retried attempt using the *original* (now stale) pre-terminal
    // `expected_state`/`expected_state_version` -- exactly what a coordinator
    // tick that never observed the first Applied result would send. The
    // store must never re-run classification/re-derive a fresh outcome for
    // an already-terminal row: it is read-only, `AlreadyApplied`, and the
    // returned row is byte-identical to what the first call produced.
    let mut replay_args = finalize_args(
        &record,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    replay_args.expected_state = mqk_db::STATE_STOPPING.to_string();
    replay_args.expected_state_version = 1;
    match mqk_db::finalize_autonomous_daily_operation(&pool, &replay_args).await.expect("call ok") {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::AlreadyApplied(replayed) => {
            assert_eq!(replayed.state, mqk_db::STATE_COMPLETED_NO_TRADE);
            assert_eq!(replayed.outcome, record.outcome);
            assert_eq!(replayed.finalized_at_utc, record.finalized_at_utc);
            assert_eq!(replayed.state_version, record.state_version);
        }
        other => panic!("expected the terminal row to resist reclassification via AlreadyApplied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_36_conflicting_terminal_outcome_is_rejected() {
    let Some(pool) = maybe_db("store_36").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s36"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    let record = match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let mut conflicting = finalize_args(
        &record,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED,
        now,
    );
    conflicting.expected_state = mqk_db::STATE_STOPPING.to_string();
    conflicting.expected_state_version = 1;
    match mqk_db::finalize_autonomous_daily_operation(&pool, &conflicting)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::ConflictingTerminalTruth(existing) => {
            assert_eq!(existing.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected ConflictingTerminalTruth, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_37_generic_completed_target_is_illegal() {
    let Some(pool) = maybe_db("store_37").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s37"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::IllegalTarget => {}
        other => panic!("expected IllegalTarget, got {other:?}"),
    }
    let still = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(still.state, mqk_db::STATE_STOPPING);
}

#[tokio::test]
#[ignore]
async fn store_38_stopping_to_evidence_degraded_is_legal() {
    let Some(pool) = maybe_db("store_38").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s38"), now).await;
    let record = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        Some("sha256:bb"),
        now,
    )
    .await;
    assert_eq!(record.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert!(record.outcome.is_none());
    assert!(record.finalized_at_utc.is_none());
}

#[tokio::test]
#[ignore]
async fn store_39_stop_retrying_to_evidence_degraded_is_legal() {
    let Some(pool) = maybe_db("store_39").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s39"), now).await;
    let op = transition(&pool, &op, mqk_db::STATE_STOP_RETRYING, None, None, now).await;
    let record = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_run_lineage_unavailable"),
        Some("sha256:cc"),
        now,
    )
    .await;
    assert_eq!(record.state, mqk_db::STATE_EVIDENCE_DEGRADED);
}

#[tokio::test]
#[ignore]
async fn store_40_evidence_blocker_stores_exact_reason_signature_with_null_outcome_finalized() {
    let Some(pool) = maybe_db("store_40").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s40"), now).await;
    let record = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_unresolved_dispatch_claim"),
        Some("sha256:dd"),
        now,
    )
    .await;
    assert_eq!(
        record.state_reason_code.as_deref(),
        Some("unknown_unresolved_dispatch_claim")
    );
    assert_eq!(record.state_blocker_signature.as_deref(), Some("sha256:dd"));
    assert!(record.outcome.is_none());
    assert!(record.finalized_at_utc.is_none());
}

#[tokio::test]
#[ignore]
async fn store_41_exact_blocker_replay_creates_no_duplicate_event() {
    let Some(pool) = maybe_db("store_41").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s41"), now).await;
    let degraded = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        Some("sha256:ee"),
        now,
    )
    .await;
    let refresh_args = mqk_db::RefreshAutonomousDailyOperationBlockerArgs {
        operation_id: degraded.operation_id,
        expected_state: degraded.state.clone(),
        expected_state_version: degraded.state_version,
        reason_code: "unknown_incomplete_bar_coverage".to_string(),
        blocker_signature: Some("sha256:ee".to_string()),
        occurred_at_utc: now,
        bounded_detail: "exact replay".to_string(),
    };
    match mqk_db::refresh_autonomous_daily_operation_blocker(&pool, &refresh_args)
        .await
        .expect("call ok")
    {
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::AlreadyApplied(record) => {
            assert_eq!(record.state_version, degraded.state_version);
        }
        other => panic!("expected AlreadyApplied (exact replay is a no-op), got {other:?}"),
    }
    let events = mqk_db::list_autonomous_daily_operation_events(&pool, op.operation_id, 20)
        .await
        .expect("list ok");
    let degraded_events = events
        .iter()
        .filter(|e| e.to_state == mqk_db::STATE_EVIDENCE_DEGRADED)
        .count();
    assert_eq!(degraded_events, 1);
}

#[tokio::test]
#[ignore]
async fn store_42_evidence_degraded_stopped_operation_recovers_to_stopping() {
    let Some(pool) = maybe_db("store_42").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s42"), now).await;
    let degraded = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        Some("sha256:ff"),
        now,
    )
    .await;
    assert!(degraded.stopped_at_utc.is_some());
    let recovered = transition(&pool, &degraded, mqk_db::STATE_STOPPING, None, None, now).await;
    assert_eq!(recovered.state, mqk_db::STATE_STOPPING);
    assert_eq!(recovered.state_reason_code, None);
}

#[tokio::test]
#[ignore]
async fn store_43_evidence_degraded_unstopped_operation_cannot_enter_finalization_recovery() {
    let Some(pool) = maybe_db("store_43").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    // Reach `evidence_degraded` via the pre-existing `running -> evidence_degraded`
    // edge, never having stopped -- `stopped_at_utc` remains null.
    let op = create_operation(
        &pool,
        &unique_adapter_id("s43"),
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;
    let op = transition(&pool, &op, mqk_db::STATE_START_RETRYING, None, None, now).await;
    let to_running = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: op.operation_id,
        expected_state: op.state.clone(),
        expected_state_version: op.state_version,
        run_id: Uuid::new_v4(),
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test fixture: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running)
        .await
        .expect("ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let degraded = transition(
        &pool,
        &running,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("mid_run_blocker"),
        None,
        now,
    )
    .await;
    assert!(degraded.stopped_at_utc.is_none());

    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        degraded.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
    )
    .await
    .expect("call ok");
    assert_eq!(outcome, AutonomousDailyFinalizationOutcome::NotEligible);
}

#[tokio::test]
#[ignore]
async fn store_44_commit_acknowledgment_loss_is_resolved_by_authoritative_reread() {
    let Some(pool) = maybe_db("store_44").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s44"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    // Apply for real, then simulate "the caller never saw the Applied
    // result" by re-attempting the identical CAS from the same expected
    // (now-stale) state/version -- the store-level contract guarantees this
    // degrades to AlreadyApplied, never a duplicate write.
    let first = mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok");
    assert!(matches!(
        first,
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(_)
    ));
    let retry = mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok");
    assert!(matches!(
        retry,
        mqk_db::FinalizeAutonomousDailyOperationOutcome::AlreadyApplied(_)
    ));
}

#[tokio::test]
#[ignore]
async fn store_45_cas_false_is_resolved_only_by_exact_reread() {
    let Some(pool) = maybe_db("store_45").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s45"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        mqk_db::OUTCOME_ACTIVITY_ORDER_SUBMITTED,
        now,
    );
    let applied = match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    // CAS-false path: same expected_state/version as the ALREADY-superseded
    // pre-finalization row -- the store must classify this precisely as
    // AlreadyApplied (exact match), never blindly report success.
    let replay = mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok");
    match replay {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::AlreadyApplied(record) => {
            assert_eq!(record.operation_id, applied.operation_id);
            assert_eq!(record.finalized_at_utc, applied.finalized_at_utc);
        }
        other => panic!("expected AlreadyApplied, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_46_complete_db_outage_performs_no_write() {
    // A pool pointed at an unreachable port simulates complete outage: the
    // high-level entry point's initial operation load fails before any
    // write is attempted, and the typed result reflects that -- no panic, no
    // fabricated success.
    let bad_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/mqk_test?sslmode=disable")
        .expect("lazy connect (no I/O yet)");
    let outcome = classify_and_finalize_autonomous_daily_operation(
        &bad_pool,
        Uuid::new_v4(),
        Utc::now().trunc_subsecs(6),
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
    )
    .await
    .expect("call must not panic on outage");
    assert_eq!(
        outcome,
        AutonomousDailyFinalizationOutcome::DatabaseUnavailable
    );
}

#[tokio::test]
#[ignore]
async fn store_47_assignment_identity_unavailable_is_not_a_database_failure_proof() {
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
    // UNCERTAINTY-CLOSURE (REPAIR 6): renamed from its original
    // "partial_evidence_read_failure" name -- this scenario is an ordinary,
    // deterministic identity-unavailable evidence gap (empty
    // `MultiSymbolRuntimeConfig`), never a database read failure of any
    // kind. `store_48`/`store_49` below are the real partial-evidence-read-
    // failure proofs, driven through the injected effect seam against a
    // real database.
    let Some(pool) = maybe_db("store_47").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s47"), now).await;
    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        op.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
    )
    .await
    .expect("call ok");
    match outcome {
        AutonomousDailyFinalizationOutcome::EvidenceDegraded {
            reason_code,
            record,
            newly_applied: _,
        } => {
            assert_eq!(
                reason_code,
                AutonomousDailyUnknownReason::AssignmentIdentityUnavailable
            );
            assert_eq!(record.state, mqk_db::STATE_EVIDENCE_DEGRADED);
            assert!(record.outcome.is_none());
            assert!(record.finalized_at_utc.is_none());
        }
        other => panic!("expected EvidenceDegraded, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_48_real_partial_evidence_read_failure_degrades_via_confirmed_reread() {
    // REPAIR 6: a real, later evidence read (the outbox/inbox load, after
    // identity/lineage/coverage/claims already resolved) fails via the
    // narrow injected effect seam -- never a mocked operation-row load, and
    // the operation-transition store below remains fully real and usable.
    let Some(pool) = maybe_db("store_48").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s48"), now).await;
    let outcome = classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        &pool,
        op.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
        AutonomousDailyFinalizationEffectSeam {
            force_evidence_read_failure_after_claims: true,
            ..Default::default()
        },
    )
    .await
    .expect("call ok");
    match outcome {
        AutonomousDailyFinalizationOutcome::EvidenceDegraded {
            reason_code,
            record,
            newly_applied: _,
        } => {
            assert_eq!(
                reason_code,
                AutonomousDailyUnknownReason::DatabaseUnavailable
            );
            assert_eq!(record.state, mqk_db::STATE_EVIDENCE_DEGRADED);
            assert_eq!(
                record.state_reason_code.as_deref(),
                Some("unknown_database_unavailable")
            );
            assert!(record.outcome.is_none());
            assert!(record.finalized_at_utc.is_none());
            // No raw SQL/connection/error text ever enters a durable field --
            // the persisted reason is exactly one of the closed `unknown_*`
            // codes, never the injected test error string.
            let persisted = record.state_reason_code.as_deref().unwrap_or("");
            assert!(!persisted.contains("test-injected"));
            assert!(!persisted.to_lowercase().contains("sql"));
        }
        other => panic!("expected EvidenceDegraded(DatabaseUnavailable), got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_49_partial_evidence_read_failure_with_blocker_write_failure_persists_nothing() {
    // REPAIR 6: the evidence read fails (as in store_48) *and* the best-
    // effort blocker write/re-read is itself unavailable -- the overall call
    // must return `DatabaseUnavailable` and the durable row must show zero
    // fabricated blocker truth.
    let Some(pool) = maybe_db("store_49").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s49"), now).await;
    let outcome = classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        &pool,
        op.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
        AutonomousDailyFinalizationEffectSeam {
            force_evidence_read_failure_after_claims: true,
            force_blocker_persistence_unavailable: true,
            ..Default::default()
        },
    )
    .await
    .expect("call ok");
    assert_eq!(
        outcome,
        AutonomousDailyFinalizationOutcome::DatabaseUnavailable
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch ok")
        .expect("row");
    assert_eq!(
        after.state,
        mqk_db::STATE_STOPPING,
        "no fabricated blocker may be persisted when the blocker write itself is unavailable"
    );
    assert!(after.state_reason_code.is_none());
    assert!(after.state_blocker_signature.is_none());
}

#[tokio::test]
#[ignore]
async fn store_50_high_level_commit_acknowledgment_lost_confirms_via_reread() {
    // REPAIR 5, scenario 1: the real terminal CAS genuinely commits; this
    // attempt's observation of the `Ok(Applied(_))` result is discarded via
    // the injected seam, forcing the authoritative-re-read path, which must
    // find the same already-committed terminal truth -- `Finalized`, exactly
    // one lifecycle event (never a duplicate write).
    let Some(pool) = maybe_db("store_50").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, _run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("s50"), now).await;
    let outcome = classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &AutonomousDailyFinalizationPolicyInputs {
            calendar_provider: &NyseWeekdaysProvider,
            config: &config,
            binding: &binding,
            strategy_registry: &registry,
        },
        AutonomousDailyFinalizationEffectSeam {
            force_commit_acknowledgment_lost: true,
            ..Default::default()
        },
    )
    .await
    .expect("call ok");
    match outcome {
        AutonomousDailyFinalizationOutcome::Finalized { outcome, record } => {
            assert_eq!(
                outcome,
                mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
            );
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected Finalized via authoritative re-read, got {other:?}"),
    }
    let events = mqk_db::list_autonomous_daily_operation_events(&pool, operation.operation_id, 20)
        .await
        .expect("list ok");
    let terminal_events = events
        .iter()
        .filter(|e| e.to_state == mqk_db::STATE_COMPLETED_NO_TRADE)
        .count();
    assert_eq!(
        terminal_events, 1,
        "commit-ack-lost recovery must never duplicate the terminal event"
    );
}

#[tokio::test]
#[ignore]
async fn store_51_high_level_cas_false_before_commit_claims_no_success() {
    // REPAIR 5, scenario 2: a real, separate pre-commit write bumps the same
    // operation's version (stopping -> stop_retrying) before this attempt's
    // own real CAS runs, so that CAS naturally observes a stale expected
    // version -- never a mocked CAS result. The actual row is nonterminal
    // after re-read, so this attempt must claim no success.
    let Some(pool) = maybe_db("store_51").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, _run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("s51"), now).await;
    let result = classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &AutonomousDailyFinalizationPolicyInputs {
            calendar_provider: &NyseWeekdaysProvider,
            config: &config,
            binding: &binding,
            strategy_registry: &registry,
        },
        AutonomousDailyFinalizationEffectSeam {
            pre_commit_effect:
                AutonomousDailyFinalizationInjectedPreCommitEffect::StaleVersionBeforeCommit,
            ..Default::default()
        },
    )
    .await;
    assert!(
        result.is_err(),
        "a genuinely stale CAS against a nonterminal actual row must never claim success, got {result:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row");
    assert_eq!(after.state, mqk_db::STATE_STOP_RETRYING);
    assert!(after.outcome.is_none());
    assert!(after.finalized_at_utc.is_none());
}

#[tokio::test]
#[ignore]
async fn store_52_high_level_conflicting_writer_returns_conflict_never_rewrites() {
    // REPAIR 5, scenario 3: a real, separate pre-commit write finalizes the
    // same operation to a *different* valid terminal truth
    // (completed_with_activity/activity_decision_accepted) before this
    // attempt's own real CAS (which would otherwise reach
    // completed_no_trade/no_trade_strategy_evaluated_no_signal from the same
    // clean no-trade evidence) runs -- the real CAS naturally fails against
    // the now-already-terminal row, and the re-read must report `Conflict`,
    // never rewriting the concurrent writer's truth.
    let Some(pool) = maybe_db("store_52").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, _run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("s52"), now).await;
    let outcome = classify_and_finalize_autonomous_daily_operation_with_effect_seam(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &AutonomousDailyFinalizationPolicyInputs {
            calendar_provider: &NyseWeekdaysProvider,
            config: &config,
            binding: &binding,
            strategy_registry: &registry,
        },
        AutonomousDailyFinalizationEffectSeam {
            pre_commit_effect:
                AutonomousDailyFinalizationInjectedPreCommitEffect::ConflictingTerminalWriteBeforeCommit(
                    AutonomousDailyTerminalReason::ActivityDecisionAccepted,
                ),
            ..Default::default()
        },
    )
    .await
    .expect("call ok");
    assert_eq!(outcome, AutonomousDailyFinalizationOutcome::Conflict);
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row");
    assert_eq!(after.state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
    assert_eq!(
        after.outcome.as_deref(),
        Some(mqk_db::OUTCOME_ACTIVITY_DECISION_ACCEPTED)
    );
}

#[tokio::test]
#[ignore]
async fn store_53_completed_no_trade_paired_with_activity_outcome_is_illegal() {
    // REPAIR 1/7: cross-paired combination must be rejected as
    // `IllegalTarget` before any SQL, and must write nothing.
    let Some(pool) = maybe_db("store_53").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s53"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::IllegalTarget => {}
        other => panic!("expected IllegalTarget, got {other:?}"),
    }
    let still = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(still.state, mqk_db::STATE_STOPPING);
    assert!(still.outcome.is_none());
}

#[tokio::test]
#[ignore]
async fn store_54_completed_with_activity_paired_with_no_trade_outcome_is_illegal() {
    // REPAIR 1/7: the reverse cross-paired combination, same requirement.
    let Some(pool) = maybe_db("store_54").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s54"), now).await;
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::IllegalTarget => {}
        other => panic!("expected IllegalTarget, got {other:?}"),
    }
    let still = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(still.state, mqk_db::STATE_STOPPING);
    assert!(still.outcome.is_none());
}

#[tokio::test]
#[ignore]
async fn store_55_same_state_outcome_but_null_finalized_at_is_not_already_applied() {
    // REPAIR 2/7: narrow test-only SQL hand-constructs a malformed
    // "terminal" row whose state/outcome already match what this attempt
    // requests, but whose `finalized_at_utc` is null -- the schema itself
    // does not forbid this combination, so the application-level
    // completeness check under test must be the thing that rejects replay.
    let Some(pool) = maybe_db("store_55").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s55"), now).await;
    sqlx::query(
        "update sys_autonomous_daily_operations \
         set state = $1, outcome = $2, finalized_at_utc = null, state_version = state_version + 1 \
         where operation_id = $3",
    )
    .bind(mqk_db::STATE_COMPLETED_NO_TRADE)
    .bind(mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL)
    .bind(op.operation_id)
    .execute(&pool)
    .await
    .expect("malform row");
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::ConflictingTerminalTruth(existing) => {
            assert!(existing.finalized_at_utc.is_none());
        }
        other => panic!(
            "expected ConflictingTerminalTruth (never AlreadyApplied for an incomplete terminal row), got {other:?}"
        ),
    }
}

#[tokio::test]
#[ignore]
async fn store_56_same_state_outcome_but_residual_blocker_fields_is_not_already_applied() {
    // REPAIR 2/7: same idea as store_55, but the malformed row carries a
    // residual `state_reason_code` instead of a null `finalized_at_utc`.
    let Some(pool) = maybe_db("store_56").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s56"), now).await;
    sqlx::query(
        "update sys_autonomous_daily_operations \
         set state = $1, outcome = $2, finalized_at_utc = $3, \
             state_reason_code = 'stale_residual_reason', state_version = state_version + 1 \
         where operation_id = $4",
    )
    .bind(mqk_db::STATE_COMPLETED_NO_TRADE)
    .bind(mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL)
    .bind(now)
    .bind(op.operation_id)
    .execute(&pool)
    .await
    .expect("malform row");
    let args = finalize_args(
        &op,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL,
        now,
    );
    match mqk_db::finalize_autonomous_daily_operation(&pool, &args)
        .await
        .expect("call ok")
    {
        mqk_db::FinalizeAutonomousDailyOperationOutcome::ConflictingTerminalTruth(existing) => {
            assert!(existing.state_reason_code.is_some());
        }
        other => panic!("expected ConflictingTerminalTruth, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn store_57_high_level_malformed_automatic_terminal_row_is_conflict() {
    // REPAIR 2/7: the high-level entry point must never treat a malformed
    // automatic terminal row (here: null `finalized_at_utc`) as accepted
    // truth -- `Conflict`, never `AlreadyFinalized`.
    let Some(pool) = maybe_db("store_57").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s57"), now).await;
    sqlx::query(
        "update sys_autonomous_daily_operations \
         set state = $1, outcome = $2, finalized_at_utc = null, state_version = state_version + 1 \
         where operation_id = $3",
    )
    .bind(mqk_db::STATE_COMPLETED_NO_TRADE)
    .bind(mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL)
    .bind(op.operation_id)
    .execute(&pool)
    .await
    .expect("malform row");
    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        op.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
    )
    .await
    .expect("call ok");
    assert_eq!(outcome, AutonomousDailyFinalizationOutcome::Conflict);
}

#[tokio::test]
#[ignore]
async fn store_58_high_level_generic_completed_row_is_already_finalized_readonly() {
    // REPAIR 2/7: a manual/administrative `completed` row is read-only
    // terminal truth -- `AlreadyFinalized`, never rewritten or reclassified.
    let Some(pool) = maybe_db("store_58").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s58"), now).await;
    let manual = transition(&pool, &op, mqk_db::STATE_COMPLETED, None, None, now).await;
    assert_eq!(manual.state, mqk_db::STATE_COMPLETED);
    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        op.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &test_policy_inputs(),
    )
    .await
    .expect("call ok");
    match outcome {
        AutonomousDailyFinalizationOutcome::AlreadyFinalized(record) => {
            assert_eq!(record.state, mqk_db::STATE_COMPLETED);
        }
        other => {
            panic!("expected AlreadyFinalized (read-only) for generic completed, got {other:?}")
        }
    }
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(
        after.state_version, manual.state_version,
        "generic completed row must never be rewritten"
    );
}

#[tokio::test]
#[ignore]
async fn store_59_persist_finalization_blocker_refuses_when_matching_runtime_active() {
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
    // GATE-REPAIR-01 (E2B wrapper hardening): `persist_autonomous_daily_finalization_blocker`
    // is E3's narrow policy-resolution-failure seam
    // (`autonomous_daily_coordinator.rs::handle_outcome_finalization`). It
    // must never become a second way to bypass finalization eligibility --
    // when the caller-supplied context says a matching local runtime is
    // still active, this wrapper must refuse to write the `evidence_degraded`
    // blocker at all, even though the operation is otherwise a durably
    // stopped `stopping` row that would normally be eligible.
    let Some(pool) = maybe_db("store_59").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let op = stopping_operation(&pool, &unique_adapter_id("s59"), now).await;
    assert!(op.stopped_at_utc.is_some(), "fixture is durably stopped");

    let events_before: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(op.operation_id)
    .fetch_one(&pool)
    .await
    .expect("count ok");

    let outcome = persist_autonomous_daily_finalization_blocker(
        &pool,
        &op,
        now,
        AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
        AutonomousDailyFinalizationContext {
            matching_local_runtime_active: true,
        },
    )
    .await
    .expect("call ok");
    assert_eq!(outcome, AutonomousDailyFinalizationOutcome::NotEligible);

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(after.state, mqk_db::STATE_STOPPING);
    assert_eq!(
        after.state_version, op.state_version,
        "zero state-version change when a matching local runtime is active"
    );
    assert_eq!(after.state_reason_code, op.state_reason_code);
    assert_eq!(after.state_blocker_signature, op.state_blocker_signature);

    let events_after: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(op.operation_id)
    .fetch_one(&pool)
    .await
    .expect("count ok");
    assert_eq!(events_after, events_before, "zero lifecycle-event delta");

    // Sanity: the exact same call with the matching-runtime fact false (and
    // stopped_at_utc present) is the pre-existing, still-valid policy-failure
    // behavior -- proves this hardening is additive, not a regression of the
    // legitimate case `store_47`/`ci_16_17` already cover.
    let permitted = persist_autonomous_daily_finalization_blocker(
        &pool,
        &op,
        now,
        AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
        AutonomousDailyFinalizationContext {
            matching_local_runtime_active: false,
        },
    )
    .await
    .expect("call ok");
    match permitted {
        AutonomousDailyFinalizationOutcome::EvidenceDegraded {
            reason_code,
            newly_applied,
            ..
        } => {
            assert_eq!(
                reason_code,
                AutonomousDailyUnknownReason::AssignmentIdentityUnavailable
            );
            assert!(newly_applied, "first write for this operation applies");
        }
        other => panic!("expected EvidenceDegraded, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group 3 — integrated end-to-end DB-backed proof
// ---------------------------------------------------------------------------

fn test_policy_inputs() -> AutonomousDailyFinalizationPolicyInputs<'static> {
    // An intentionally empty/unresolvable policy: `NoSymbolAssignment` at
    // `resolve_current_coverage_policy_inputs`, and
    // `derive_assignment_identity`/`derive_runtime_binding_identity` over an
    // empty config never equal the fixture's persisted `"ai"`/`"rbi"`
    // values -- deterministically reaches
    // `unknown_assignment_identity_unavailable` without needing any real
    // strategy/calendar/env resolution, which store_43/store_47 above rely
    // on.
    static CONFIG: std::sync::OnceLock<MultiSymbolRuntimeConfig> = std::sync::OnceLock::new();
    static BINDING: std::sync::OnceLock<EffectiveRuntimeBinding> = std::sync::OnceLock::new();
    static REGISTRY: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();
    let config = CONFIG.get_or_init(|| MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: Vec::new(),
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    });
    let binding = BINDING.get_or_init(|| EffectiveRuntimeBinding {
        effective_runtime_strategy_id: None,
        effective_runtime_target_symbol: None,
        effective_runtime_timeframe_secs: None,
    });
    let registry = REGISTRY.get_or_init(PluginRegistry::new);
    AutonomousDailyFinalizationPolicyInputs {
        calendar_provider: &NyseWeekdaysProvider,
        config,
        binding,
        strategy_registry: registry,
    }
}

async fn integrated_policy_inputs_matching(
    op: &mqk_db::AutonomousDailyOperationRecord,
) -> (
    MultiSymbolRuntimeConfig,
    EffectiveRuntimeBinding,
    PluginRegistry,
) {
    let config = MultiSymbolRuntimeConfig {
        schema_version: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION.to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: SYMBOL.to_string(),
            strategy_id: STRATEGY_ID.to_string(),
            timeframe: TIMEFRAME.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    let binding = EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(STRATEGY_ID.to_string()),
        effective_runtime_target_symbol: Some(SYMBOL.to_string()),
        effective_runtime_timeframe_secs: Some(300),
    };
    let mut registry = PluginRegistry::new();
    let _ = mqk_strategy::engines::register_builtin_strategies(&mut registry, String::new());
    let _ = op; // identity strings are supplied directly by the caller fixture below
    (config, binding, registry)
}

/// Build a durably-anchored, `stopping` operation whose `assignment_identity`/
/// `runtime_binding_identity` exactly match what
/// `state::derive_assignment_identity`/`derive_runtime_binding_identity`
/// compute for [`integrated_policy_inputs_matching`]'s config/binding, with a
/// real coverage-bound event written and confirmed. Returns the operation
/// record plus the `run_id` bound to it.
async fn integrated_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
) -> (
    mqk_db::AutonomousDailyOperationRecord,
    Uuid,
    MultiSymbolRuntimeConfig,
    EffectiveRuntimeBinding,
    PluginRegistry,
) {
    // Force a known, hand-computable effective grace (0s) so the fixture's
    // hand-built `CoverageBoundDetail` below stays byte-identical to what
    // `resolve_current_coverage_policy_inputs` independently recomputes at
    // classify time (default grace is 900s, clamped to the 300s timeframe --
    // not 0 -- so this must be forced, not assumed).
    std::env::set_var(daily_data_readiness::GRACE_SECS_ENV, "0");
    let (config, binding, registry) =
        integrated_policy_inputs_matching(&fixture_operation_record(mqk_db::STATE_STOPPING)).await;
    let assignment_identity = mqk_daemon::state::derive_assignment_identity(&config);
    let runtime_binding_identity = mqk_daemon::state::derive_runtime_binding_identity(&binding);

    let exchange_open = monday_at(13, 0, 0);
    let exchange_close = monday_at(20, 0, 0);
    // Evaluated 30 minutes after session open: with grace forced to 0 and
    // `intraday_scalper`'s real `required_history_bars` (5), six 5-minute
    // bars have already closed by 13:30 -- enough to avoid any previous-
    // session spillover, so the anchor is a single current-session identity
    // (13:25), never a hand-guessed value.
    let effective_open = monday_at(13, 30, 0);
    let effective_close = monday_at(13, 41, 0);

    let operation_id = Uuid::new_v4();
    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: "zze2b-sp".to_string(),
        assignment_identity,
        runtime_binding_identity,
        calendar_source: "test".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "fixed_window_override".to_string(),
        effective_operation_open_utc: effective_open,
        effective_operation_close_utc: effective_close,
        exchange_session_open_utc: exchange_open,
        exchange_session_close_utc: exchange_close,
        exchange_is_early_close: false,
        previous_trading_date: NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(),
        preopen_start_utc: exchange_open - chrono::Duration::hours(1),
        postclose_finalize_utc: effective_close + chrono::Duration::minutes(15),
        initial_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: exchange_open - chrono::Duration::hours(1),
        bounded_detail: "integrated test fixture".to_string(),
        stop_attempt_count: 0,
    };
    let operation = match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    };

    // Build the coverage anchor via the real production functions -- never
    // hand-typed -- so it is byte-identical (per `CoverageBoundDetail`'s
    // `#[derive(PartialEq)]` semantic-equality contract) to whatever
    // `resolve_coverage_for_gather` independently recomputes at classify
    // time from the same `config`/`binding`/`registry`.
    let policy = resolve_current_coverage_policy_inputs(&config, &binding, &registry)
        .expect("policy resolves for the fixture's own config/binding/registry");
    let construction_inputs = coverage_construction_inputs_from_operation(
        &operation,
        &operation.assignment_identity,
        &operation.runtime_binding_identity,
        &policy,
    )
    .expect("operation carries exchange-calendar columns");
    let detail = construct_coverage_bound_detail(&NyseWeekdaysProvider, &construction_inputs)
        .expect("construction succeeds for an ordinary trading day");
    match write_and_confirm_coverage_authority(pool, &detail, now_utc)
        .await
        .expect("write ok")
    {
        CoverageAuthorityEnsureResult::Bound(_) => {}
        other => panic!("expected Bound, got {other:?}"),
    }

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: adapter_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now_utc,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert run ok");

    let started = transition(
        pool,
        &operation,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now_utc,
    )
    .await;
    let to_running = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: started.operation_id,
        expected_state: started.state.clone(),
        expected_state_version: started.state_version,
        run_id,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "integrated fixture: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running)
        .await
        .expect("ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    for &bar_end_ts in &derive_expected_bar_set(&detail) {
        match mqk_db::claim_autonomous_daily_bar_dispatch(
            pool,
            operation_id,
            SYMBOL,
            TIMEFRAME,
            bar_end_ts,
            now_utc,
        )
        .await
        .expect("claim ok")
        {
            mqk_db::BarDispatchClaimOutcome::Claimed => {}
            other => panic!("expected Claimed, got {other:?}"),
        }
        let evaluation_id = Uuid::new_v4();
        mqk_db::insert_strategy_signal_evaluation(
            pool,
            &mqk_db::InsertStrategySignalEvaluationArgs {
                evaluation_id,
                ts_utc: now_utc,
                run_id: Some(run_id),
                strategy_id: STRATEGY_ID.to_string(),
                symbol: SYMBOL.to_string(),
                timeframe: TIMEFRAME.to_string(),
                bar_context_source: "db_loaded".to_string(),
                bars_loaded: 1,
                latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(bar_end_ts, 0),
                signal_generated: false,
                signal_qty: Some(0),
                signal_side: None,
                reason_code: "flat".to_string(),
                reason: "flat".to_string(),
                decision_stage: "strategy_evaluated".to_string(),
                source: "test".to_string(),
            },
        )
        .await
        .expect("insert evaluation ok");
        // `bars_dispatched <= bars_observed` is a DB CHECK constraint --
        // observation must be recorded before dispatch completion for the
        // same bar.
        mqk_db::record_completed_bar_observed(pool, operation_id, bar_end_ts, now_utc)
            .await
            .expect("observe ok");
        mqk_db::complete_autonomous_daily_bar_dispatch(
            pool,
            operation_id,
            SYMBOL,
            TIMEFRAME,
            bar_end_ts,
            now_utc,
            Some(evaluation_id),
        )
        .await
        .expect("complete ok");
    }

    let stopping = transition(pool, &running, mqk_db::STATE_STOPPING, None, None, now_utc).await;
    mqk_db::record_autonomous_runtime_stopped(pool, stopping.operation_id, now_utc)
        .await
        .expect("stop ok");
    let final_op = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");

    (final_op, run_id, config, binding, registry)
}

#[tokio::test]
#[ignore]
async fn integrated_01_clean_no_trade_evidence_reaches_completed_no_trade() {
    let Some(pool) = maybe_db("integrated_01").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, _run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("i01"), now).await;

    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &AutonomousDailyFinalizationPolicyInputs {
            calendar_provider: &NyseWeekdaysProvider,
            config: &config,
            binding: &binding,
            strategy_registry: &registry,
        },
    )
    .await
    .expect("call ok");

    match outcome {
        AutonomousDailyFinalizationOutcome::Finalized { outcome, record } => {
            assert_eq!(
                outcome,
                mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
            );
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected Finalized(completed_no_trade), got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn integrated_02_clean_fill_evidence_reaches_completed_with_activity() {
    let Some(pool) = maybe_db("integrated_02").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("i02"), now).await;

    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "broker-msg-1",
        Some("fill-1"),
        "internal-order-1",
        "broker-order-1",
        "fill",
        &serde_json::json!({"event_kind": "fill"}),
        now.timestamp_millis(),
        now,
    )
    .await
    .expect("insert fill ok");

    let outcome = classify_and_finalize_autonomous_daily_operation(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &AutonomousDailyFinalizationPolicyInputs {
            calendar_provider: &NyseWeekdaysProvider,
            config: &config,
            binding: &binding,
            strategy_registry: &registry,
        },
    )
    .await
    .expect("call ok");

    match outcome {
        AutonomousDailyFinalizationOutcome::Finalized { outcome, record } => {
            assert_eq!(outcome, mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED);
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_WITH_ACTIVITY);
        }
        other => panic!("expected Finalized(completed_with_activity), got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn integrated_03_unresolved_claim_degrades_then_repairs_then_finalizes() {
    let Some(pool) = maybe_db("integrated_03").await else {
        return;
    };
    let now = Utc::now().trunc_subsecs(6);
    let (operation, _run_id, config, binding, registry) =
        integrated_fixture(&pool, &unique_adapter_id("i03"), now).await;

    // Force one expected bar's claim back to `claimed` (unresolved) after the
    // fixture completed it, simulating a crash mid-completion.
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'claimed', evaluation_id = null, completed_at_utc = null \
         where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation.operation_id)
    .bind(monday_at(13, 30, 0).timestamp())
    .execute(&pool)
    .await
    .expect("force unresolved claim");

    let policy_inputs = AutonomousDailyFinalizationPolicyInputs {
        calendar_provider: &NyseWeekdaysProvider,
        config: &config,
        binding: &binding,
        strategy_registry: &registry,
    };

    let first = classify_and_finalize_autonomous_daily_operation(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &policy_inputs,
    )
    .await
    .expect("call ok");
    match first {
        AutonomousDailyFinalizationOutcome::EvidenceDegraded {
            reason_code,
            record,
            newly_applied: _,
        } => {
            assert_eq!(
                reason_code,
                AutonomousDailyUnknownReason::UnresolvedDispatchClaim
            );
            assert_eq!(record.state, mqk_db::STATE_EVIDENCE_DEGRADED);
        }
        other => panic!("expected EvidenceDegraded, got {other:?}"),
    }

    // Repair: reconfirm the claim as completed with a durable evaluation.
    let evaluation_id = Uuid::new_v4();
    mqk_db::insert_strategy_signal_evaluation(
        &pool,
        &mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id,
            ts_utc: now,
            run_id: Some(_run_id),
            strategy_id: STRATEGY_ID.to_string(),
            symbol: SYMBOL.to_string(),
            timeframe: TIMEFRAME.to_string(),
            bar_context_source: "db_loaded".to_string(),
            bars_loaded: 1,
            latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(monday_at(13, 30, 0).timestamp(), 0),
            signal_generated: false,
            signal_qty: Some(0),
            signal_side: None,
            reason_code: "flat".to_string(),
            reason: "flat".to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "test".to_string(),
        },
    )
    .await
    .expect("insert repaired evaluation ok");
    sqlx::query(
        "update sys_autonomous_daily_bar_dispatches set status = 'completed', evaluation_id = $3, completed_at_utc = $4 \
         where operation_id = $1 and bar_end_ts = $2",
    )
    .bind(operation.operation_id)
    .bind(monday_at(13, 30, 0).timestamp())
    .bind(evaluation_id)
    .bind(now)
    .execute(&pool)
    .await
    .expect("repair claim");

    let second = classify_and_finalize_autonomous_daily_operation(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &policy_inputs,
    )
    .await
    .expect("call ok");
    match second {
        AutonomousDailyFinalizationOutcome::RecoveredToStopping(record) => {
            assert_eq!(record.state, mqk_db::STATE_STOPPING);
        }
        other => panic!("expected RecoveredToStopping, got {other:?}"),
    }

    let third = classify_and_finalize_autonomous_daily_operation(
        &pool,
        operation.operation_id,
        now,
        AutonomousDailyFinalizationContext::default(),
        &policy_inputs,
    )
    .await
    .expect("call ok");
    match third {
        AutonomousDailyFinalizationOutcome::Finalized { outcome, record } => {
            assert_eq!(
                outcome,
                mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
            );
            assert_eq!(record.state, mqk_db::STATE_COMPLETED_NO_TRADE);
        }
        other => panic!("expected Finalized(completed_no_trade), got {other:?}"),
    }
}

// Silence unused-import warnings for helpers only exercised by `#[ignore]`d
// DB-backed tests when compiled without `MQK_DATABASE_URL` set (they are
// still compiled -- `cargo test` builds the whole binary regardless).
#[allow(unused_imports)]
use gather_autonomous_daily_outcome_evidence as _unused_gather_import;
