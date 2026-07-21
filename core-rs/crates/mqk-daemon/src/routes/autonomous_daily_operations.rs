//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
//! PROJECTION: strictly read-only projection of durable
//! `sys_autonomous_daily_operations` truth (already written by the accepted
//! E2A/E2B/E3 foundation) onto two canonical GET routes, plus one shared
//! pure projection reused by the additive summary blocks on
//! readiness/paper-status/preflight.
//!
//! ```text
//! GET /api/v1/autonomous/daily-operation[?market_date=YYYY-MM-DD]
//! GET /api/v1/autonomous/daily-operations[?limit=N]
//! ```
//!
//! Binding contract: `docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md` §11.
//!
//! ## Guarantees
//!
//! - Strictly read-only: never calls `create_or_recover_autonomous_daily_operation`,
//!   `transition_autonomous_daily_operation*`, `finalize_autonomous_daily_operation`,
//!   `classify_and_finalize_autonomous_daily_operation`,
//!   `persist_autonomous_daily_finalization_blocker`,
//!   `persist_autonomous_session_event`, `write_and_confirm_coverage_authority`,
//!   `start_execution_runtime`, or `stop_execution_runtime`.
//! - Never reruns the E2B classifier or the E2A coverage-anchor construction —
//!   terminal `outcome`/`finalized_at_utc` and nonterminal `state_reason_code`
//!   are projected verbatim from already-durable columns.
//! - Activity counts are read-model aggregates over the already-validated
//!   full run lineage (`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`),
//!   never a false zero when the lineage itself is unavailable.
//! - Fail-soft: every truth-state branch returns HTTP 200 except a malformed
//!   `market_date` query parameter (HTTP 400).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use sqlx::PgPool;

use mqk_db::AutonomousDailyOperationRecord;

use crate::api_types::{
    AutonomousDailyOperationApiRow, AutonomousDailyOperationResponse,
    AutonomousDailyOperationSummary, AutonomousDailyOperationsResponse,
};
use crate::state::{
    resolve_autonomous_daily_session_plan_from_env, AppState, AutonomousDailyPlanTiming,
    AutonomousDailySessionPlanResolution,
};

pub(crate) const CANONICAL_SINGLE: &str = "/api/v1/autonomous/daily-operation";
pub(crate) const CANONICAL_HISTORY: &str = "/api/v1/autonomous/daily-operations";

const DEFAULT_HISTORY_LIMIT: i64 = 20;

/// Bounded closed set of nonterminal `unknown_*` reason codes the E1/E2B
/// contract defines (`state/autonomous_daily_outcome.rs`'s
/// `AutonomousDailyUnknownReason`). Duplicated as a literal string list
/// (rather than importing the enum) so this strictly-read-only module has
/// zero compile-time dependency on the classifier module's types.
const KNOWN_UNKNOWN_REASON_CODES: [&str; 8] = [
    "unknown_assignment_identity_unavailable",
    "unknown_run_lineage_unavailable",
    "unknown_incomplete_bar_coverage",
    "unknown_unresolved_dispatch_claim",
    "unknown_missing_evaluation_evidence",
    "unknown_order_evidence_conflict",
    "unknown_database_unavailable",
    "unknown_runtime_stop_unproven",
];

// ---------------------------------------------------------------------------
// Full-lineage activity counts (read-model aggregate, no classifier rerun)
// ---------------------------------------------------------------------------

/// Outcome of gathering durable activity counts across an operation's full
/// validated run lineage. Never a false zero: an unreadable/contradictory
/// lineage or a failed downstream read is distinguished from an
/// authoritatively empty lineage.
#[derive(Debug, Clone)]
pub(crate) enum ActivityCounts {
    Available {
        strategy_evaluation_count: i64,
        order_activity_count: i64,
        fill_count: i64,
    },
    LineageUnavailable,
    DatabaseUnavailable,
}

/// Gather `strategy_evaluation_count`/`order_activity_count`/`fill_count`
/// for one operation, scoped to its full validated run lineage
/// (`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`) —
/// never the operation's current `run_id` column alone. Read-only: only
/// `count_strategy_signal_evaluations_for_runs`, `outbox_load_all_for_run`,
/// and `inbox_load_all_for_run` are called. Never recomputes coverage or
/// reruns `classify_autonomous_daily_outcome`.
pub(crate) async fn gather_daily_operation_activity_counts(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
) -> ActivityCounts {
    let lineage =
        match mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(pool, operation)
            .await
        {
            Ok(Ok(lineage)) => lineage,
            Ok(Err(_)) => return ActivityCounts::LineageUnavailable,
            Err(_) => return ActivityCounts::DatabaseUnavailable,
        };

    if lineage.is_empty() {
        // An empty, valid lineage (operation.run_id IS NULL) is authoritative
        // zero activity — never treated as unavailable.
        return ActivityCounts::Available {
            strategy_evaluation_count: 0,
            order_activity_count: 0,
            fill_count: 0,
        };
    }

    let strategy_evaluation_count =
        match mqk_db::count_strategy_signal_evaluations_for_runs(pool, &lineage).await {
            Ok(n) => n,
            Err(_) => return ActivityCounts::DatabaseUnavailable,
        };

    let mut order_activity_count: i64 = 0;
    let mut fill_count: i64 = 0;
    for run_id in &lineage {
        let outbox_rows = match mqk_db::outbox_load_all_for_run(pool, *run_id).await {
            Ok(rows) => rows,
            Err(_) => return ActivityCounts::DatabaseUnavailable,
        };
        order_activity_count += outbox_rows.len() as i64;

        let inbox_rows = match mqk_db::inbox_load_all_for_run(pool, *run_id).await {
            Ok(rows) => rows,
            Err(_) => return ActivityCounts::DatabaseUnavailable,
        };
        for row in &inbox_rows {
            match row.event_kind.as_str() {
                "fill" | "partial_fill" => fill_count += 1,
                "ack" | "cancel_ack" | "replace_ack" | "reject" => order_activity_count += 1,
                _ => {}
            }
        }
    }

    ActivityCounts::Available {
        strategy_evaluation_count,
        order_activity_count,
        fill_count,
    }
}

// ---------------------------------------------------------------------------
// Shared pure projection (E4.10) — single route, history route, and every
// additive summary block all call this, never a second terminal-state or
// finalization-status mapping.
// ---------------------------------------------------------------------------

struct DailyOperationOutcomeProjection {
    outcome_class: Option<String>,
    outcome_reason_code: Option<String>,
    finalized_at_utc: Option<String>,
    finalization_status: String,
    evidence_state: String,
    evidence_blockers: Vec<String>,
}

/// Pure projection over one durable operation row plus its already-gathered
/// activity-count outcome. Never reruns the classifier: terminal
/// `outcome_class`/`outcome_reason_code`/`finalized_at_utc` are read
/// verbatim from `record.outcome`/`record.finalized_at_utc` when `state` is
/// one of the three `completed*` values; every nonterminal row projects
/// `null` for all three.
fn project_daily_operation_outcome(
    record: &AutonomousDailyOperationRecord,
    counts: &ActivityCounts,
) -> DailyOperationOutcomeProjection {
    let state = record.state.as_str();

    let terminal_outcome_class = if state == mqk_db::STATE_COMPLETED_NO_TRADE {
        Some("no_trade")
    } else if state == mqk_db::STATE_COMPLETED_WITH_ACTIVITY {
        Some("with_activity")
    } else if state == mqk_db::STATE_COMPLETED {
        Some("completed")
    } else {
        None
    };

    if let Some(class) = terminal_outcome_class {
        return DailyOperationOutcomeProjection {
            outcome_class: Some(class.to_string()),
            outcome_reason_code: record.outcome.clone(),
            finalized_at_utc: record.finalized_at_utc.map(|t| t.to_rfc3339()),
            finalization_status: "finalized".to_string(),
            evidence_state: "complete".to_string(),
            evidence_blockers: Vec::new(),
        };
    }

    // Nonterminal projection.
    let stopped = record.stopped_at_utc.is_some();
    let finalization_status = if state == mqk_db::STATE_EVIDENCE_DEGRADED && stopped {
        "blocked_insufficient_evidence"
    } else if (state == mqk_db::STATE_STOPPING || state == mqk_db::STATE_STOP_RETRYING) && stopped {
        "awaiting_finalization"
    } else {
        "not_yet_eligible"
    }
    .to_string();

    let (evidence_state, mut evidence_blockers) = match counts {
        ActivityCounts::LineageUnavailable => (
            "unavailable".to_string(),
            vec!["unknown_run_lineage_unavailable".to_string()],
        ),
        ActivityCounts::DatabaseUnavailable => (
            "unavailable".to_string(),
            vec!["unknown_database_unavailable".to_string()],
        ),
        ActivityCounts::Available { .. } => {
            if state == mqk_db::STATE_EVIDENCE_DEGRADED {
                ("degraded".to_string(), Vec::new())
            } else {
                ("pending".to_string(), Vec::new())
            }
        }
    };

    if evidence_blockers.is_empty() {
        if let Some(reason) = record.state_reason_code.as_deref() {
            if KNOWN_UNKNOWN_REASON_CODES.contains(&reason) {
                evidence_blockers.push(reason.to_string());
            }
        }
    }

    DailyOperationOutcomeProjection {
        outcome_class: None,
        outcome_reason_code: None,
        finalized_at_utc: None,
        finalization_status,
        evidence_state,
        evidence_blockers,
    }
}

/// Build the full API row for one operation. Pure over `record` + `counts`.
pub(crate) fn build_operation_api_row(
    record: &AutonomousDailyOperationRecord,
    counts: &ActivityCounts,
) -> AutonomousDailyOperationApiRow {
    let projection = project_daily_operation_outcome(record, counts);
    let (strategy_evaluation_count, order_activity_count, fill_count) = match counts {
        ActivityCounts::Available {
            strategy_evaluation_count,
            order_activity_count,
            fill_count,
        } => (
            Some(*strategy_evaluation_count),
            Some(*order_activity_count),
            Some(*fill_count),
        ),
        ActivityCounts::LineageUnavailable | ActivityCounts::DatabaseUnavailable => {
            (None, None, None)
        }
    };

    AutonomousDailyOperationApiRow {
        operation_id: record.operation_id.to_string(),
        market_date: record.market_date.format("%Y-%m-%d").to_string(),
        deployment_mode: record.deployment_mode.clone(),
        adapter_id: record.adapter_id.clone(),
        state: record.state.clone(),
        state_reason_code: record.state_reason_code.clone(),
        finalization_status: projection.finalization_status,
        outcome_class: projection.outcome_class,
        outcome_reason_code: projection.outcome_reason_code,
        finalized_at_utc: projection.finalized_at_utc,
        run_id: record.run_id.map(|u| u.to_string()),
        bars_observed: record.bars_observed,
        bars_dispatched: record.bars_dispatched,
        last_completed_bar_ts: record.last_completed_bar_ts,
        last_dispatched_bar_ts: record.last_dispatched_bar_ts,
        strategy_evaluation_count,
        order_activity_count,
        fill_count,
        evidence_state: projection.evidence_state,
        evidence_blockers: projection.evidence_blockers,
        created_at_utc: record.created_at_utc.to_rfc3339(),
        updated_at_utc: record.updated_at_utc.to_rfc3339(),
    }
}

fn summary_with_truth_state(truth_state: &str) -> AutonomousDailyOperationSummary {
    AutonomousDailyOperationSummary {
        truth_state: truth_state.to_string(),
        operation_id: None,
        market_date: None,
        state: None,
        finalization_status: None,
        outcome_class: None,
        outcome_reason_code: None,
        finalized_at_utc: None,
        evidence_state: None,
        evidence_blockers: Vec::new(),
    }
}

fn build_active_summary(
    record: &AutonomousDailyOperationRecord,
    counts: &ActivityCounts,
) -> AutonomousDailyOperationSummary {
    let projection = project_daily_operation_outcome(record, counts);
    AutonomousDailyOperationSummary {
        truth_state: "active".to_string(),
        operation_id: Some(record.operation_id.to_string()),
        market_date: Some(record.market_date.format("%Y-%m-%d").to_string()),
        state: Some(record.state.clone()),
        finalization_status: Some(projection.finalization_status),
        outcome_class: projection.outcome_class,
        outcome_reason_code: projection.outcome_reason_code,
        finalized_at_utc: projection.finalized_at_utc,
        evidence_state: Some(projection.evidence_state),
        evidence_blockers: projection.evidence_blockers,
    }
}

// ---------------------------------------------------------------------------
// Canonical market-date resolution (no DB, no operation/coverage-event
// creation) — reuses the exact pure resolver the coordinator itself uses.
// ---------------------------------------------------------------------------

fn resolve_current_market_date_str(now_utc: DateTime<Utc>) -> Option<String> {
    let timing = AutonomousDailyPlanTiming::production_default();
    match resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => Some(plan.market_date),
        AutonomousDailySessionPlanResolution::NotApplicable { market_date, .. } => {
            Some(market_date)
        }
        AutonomousDailySessionPlanResolution::Blocked { market_date, .. } => market_date,
    }
}

// ---------------------------------------------------------------------------
// Shared summary-block computation used by readiness/paper-status/preflight
// ---------------------------------------------------------------------------

/// Compute the additive `daily_operation` summary block for the current
/// deployment/adapter slot at `now_utc`. Strictly read-only: at most one
/// slot lookup plus full-lineage activity-count reads; never creates an
/// operation or coverage event, never calls the classifier/finalizer.
/// Fails soft to a `truth_state`-only summary on any failure — never panics,
/// never propagates an error to the caller.
pub(crate) async fn compute_daily_operation_summary(
    st: &AppState,
    now_utc: DateTime<Utc>,
) -> AutonomousDailyOperationSummary {
    let Some(pool) = st.db.as_ref() else {
        return summary_with_truth_state("backend_unavailable");
    };

    let Some(market_date_str) = resolve_current_market_date_str(now_utc) else {
        return summary_with_truth_state("query_failed");
    };
    let Ok(market_date) = NaiveDate::parse_from_str(&market_date_str, "%Y-%m-%d") else {
        return summary_with_truth_state("query_failed");
    };

    let deployment_mode = st.deployment_mode().as_db_mode();
    let adapter_id = st.adapter_id();

    match mqk_db::fetch_autonomous_daily_operation_for_slot(
        pool,
        market_date,
        deployment_mode,
        adapter_id,
    )
    .await
    {
        Ok(Some(record)) => {
            let counts = gather_daily_operation_activity_counts(pool, &record).await;
            build_active_summary(&record, &counts)
        }
        Ok(None) => summary_with_truth_state("not_found"),
        Err(_) => summary_with_truth_state("query_failed"),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/autonomous/daily-operation[?market_date=YYYY-MM-DD]
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct DailyOperationQuery {
    market_date: Option<String>,
}

fn single_response(
    truth_state: &str,
    operation: Option<AutonomousDailyOperationApiRow>,
    message: Option<String>,
) -> AutonomousDailyOperationResponse {
    AutonomousDailyOperationResponse {
        canonical_route: CANONICAL_SINGLE.to_string(),
        truth_state: truth_state.to_string(),
        operation,
        message,
    }
}

pub(crate) async fn autonomous_daily_operation(
    State(st): State<Arc<AppState>>,
    Query(query): Query<DailyOperationQuery>,
) -> Response {
    let market_date = match &query.market_date {
        Some(raw) => match NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(single_response(
                        "invalid_request",
                        None,
                        Some(format!(
                            "invalid market_date '{raw}'; expected exact YYYY-MM-DD"
                        )),
                    )),
                )
                    .into_response();
            }
        },
        None => match resolve_current_market_date_str(Utc::now())
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        {
            Some(d) => d,
            None => {
                return (
                    StatusCode::OK,
                    Json(single_response(
                        "query_failed",
                        None,
                        Some("canonical current market date could not be resolved".to_string()),
                    )),
                )
                    .into_response();
            }
        },
    };

    let Some(pool) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(single_response(
                "backend_unavailable",
                None,
                Some("no database pool configured".to_string()),
            )),
        )
            .into_response();
    };

    let deployment_mode = st.deployment_mode().as_db_mode();
    let adapter_id = st.adapter_id().to_string();

    match mqk_db::fetch_autonomous_daily_operation_for_slot(
        pool,
        market_date,
        deployment_mode,
        &adapter_id,
    )
    .await
    {
        Ok(Some(record)) => {
            let counts = gather_daily_operation_activity_counts(pool, &record).await;
            let row = build_operation_api_row(&record, &counts);
            (
                StatusCode::OK,
                Json(single_response("active", Some(row), None)),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(single_response(
                "not_found",
                None,
                Some(format!(
                    "no autonomous daily operation exists yet for market_date={}",
                    market_date.format("%Y-%m-%d")
                )),
            )),
        )
            .into_response(),
        Err(_) => (
            StatusCode::OK,
            Json(single_response(
                "query_failed",
                None,
                Some("autonomous daily operation query failed".to_string()),
            )),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/autonomous/daily-operations[?limit=N]
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct DailyOperationsQuery {
    limit: Option<i64>,
}

pub(crate) async fn autonomous_daily_operations(
    State(st): State<Arc<AppState>>,
    Query(query): Query<DailyOperationsQuery>,
) -> Response {
    let requested_limit = query.limit.unwrap_or(DEFAULT_HISTORY_LIMIT);
    let effective_limit = requested_limit.clamp(1, 100);

    let Some(pool) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(AutonomousDailyOperationsResponse {
                canonical_route: CANONICAL_HISTORY.to_string(),
                truth_state: "backend_unavailable".to_string(),
                requested_limit,
                effective_limit,
                rows: Vec::new(),
                message: Some("no database pool configured".to_string()),
            }),
        )
            .into_response();
    };

    match mqk_db::list_recent_autonomous_daily_operations(pool, effective_limit).await {
        Ok(records) => {
            let mut rows = Vec::with_capacity(records.len());
            for record in &records {
                let counts = gather_daily_operation_activity_counts(pool, record).await;
                rows.push(build_operation_api_row(record, &counts));
            }
            (
                StatusCode::OK,
                Json(AutonomousDailyOperationsResponse {
                    canonical_route: CANONICAL_HISTORY.to_string(),
                    truth_state: "active".to_string(),
                    requested_limit,
                    effective_limit,
                    rows,
                    message: None,
                }),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::OK,
            Json(AutonomousDailyOperationsResponse {
                canonical_route: CANONICAL_HISTORY.to_string(),
                truth_state: "query_failed".to_string(),
                requested_limit,
                effective_limit,
                rows: Vec::new(),
                message: Some("autonomous daily operations query failed".to_string()),
            }),
        )
            .into_response(),
    }
}
