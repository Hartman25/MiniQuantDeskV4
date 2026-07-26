// core-rs/crates/mqk-daemon/src/routes/paper_lifecycle.rs
//
// PAPER-ORDER-LIFECYCLE-VIS-01C: GET /api/v1/execution/paper-lifecycle
//
// Read-only, DB-backed reconstruction of one paper run's full lifecycle
// chain: run -> strategy signal evaluation -> no-trade diagnostics (if
// any) -> oms_outbox order intent/submission -> oms_inbox broker
// ack/fill -> portfolio/accounting visibility status -> P&L visibility
// readiness.
//
// Hard constraints observed:
// - Read-only. Never inserts/updates/deletes any row.
// - Never calls a broker/provider/network endpoint.
// - Never submits, cancels, or replaces an order.
// - Restart-surviving: resolves the target run via `mqk_db::fetch_run`
//   (explicit `run_id`) or `mqk_db::fetch_latest_run_for_engine` (latest
//   PAPER run), never via in-memory active-run state.
// - Portfolio/P&L visibility is reported honestly as
//   `"in_memory_only_not_restart_surviving"` — this route does not
//   reconstruct positions or P&L from `oms_inbox` fills.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use super::durable_portfolio::{resolve_run, RunResolution, QUERY_FAILED_MESSAGE};
use crate::api_types::{
    PaperLifecycleInboxRow, PaperLifecycleNoTradeDiagnosticRow, PaperLifecycleOutboxRow,
    PaperLifecycleResponse, PaperLifecycleRunRow, PaperLifecycleSignalEvaluationRow,
    PaperLifecycleSummary,
};
use crate::state::AppState;

const CANONICAL: &str = "/api/v1/execution/paper-lifecycle";

/// This route only ever resolves PAPER-mode runs when no explicit `run_id`
/// is supplied — matching the route's name and mission (paper order
/// lifecycle visibility), independent of the daemon's currently configured
/// deployment mode.
const PAPER_MODE: &str = "PAPER";

/// Same engine identity every other durable-run route in this crate uses
/// (`routes/system.rs::DAEMON_ENGINE_ID`, `state.rs::DAEMON_ENGINE_ID`).
const DAEMON_ENGINE_ID: &str = "mqk-daemon";

/// Row cap per source table — matches `execution_flow`'s existing
/// `MAX_LIMIT` convention to bound query cost on an operator surface.
const ROW_LIMIT: i64 = 200;

#[derive(Debug, Deserialize)]
pub(crate) struct PaperLifecycleParams {
    /// Optional explicit run. Must parse as a UUID. When absent, the
    /// latest durable PAPER run for this engine is resolved instead.
    pub run_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub(crate) async fn execution_paper_lifecycle(
    State(st): State<Arc<AppState>>,
    Query(params): Query<PaperLifecycleParams>,
) -> impl IntoResponse {
    // Gate 1: explicit run_id, if supplied, must be a valid UUID.
    let explicit_run_id: Option<Uuid> = match params.run_id.as_deref() {
        Some(s) => match s.parse::<Uuid>() {
            Ok(id) => Some(id),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(empty_response(
                        "invalid_request",
                        None,
                        vec![format!("run_id is not a valid UUID: {s}")],
                    )),
                )
                    .into_response();
            }
        },
        None => None,
    };

    // Gate 2: DB must be available — every field this route reports comes
    // from a durable table, so with no pool there is nothing to resolve.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(empty_response(
                "db_unavailable",
                None,
                vec!["no DB pool configured".to_string()],
            )),
        )
            .into_response();
    };

    // Resolve the target run: explicit run_id takes priority, else the
    // latest durable PAPER run for this engine (never in-memory state).
    // Shares `resolve_run`/`RunResolution` with the durable-portfolio
    // routes so an explicit-run DB failure is distinguished from a genuine
    // not_found here too (B4 closure repair, Defect 5) instead of the two
    // collapsing to the same `SERVICE_UNAVAILABLE`/`not_found` outcomes the
    // prior `.ok()` conversion produced.
    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            return (
                StatusCode::OK,
                Json(empty_response(
                    "query_failed",
                    explicit_run_id.map(|id| id.to_string()),
                    vec![QUERY_FAILED_MESSAGE.to_string()],
                )),
            )
                .into_response();
        }
        RunResolution::NotFound => {
            let truth_state = if explicit_run_id.is_some() {
                "not_found"
            } else {
                "no_rows"
            };
            return (
                StatusCode::OK,
                Json(empty_response(
                    truth_state,
                    explicit_run_id.map(|id| id.to_string()),
                    vec![match explicit_run_id {
                        Some(id) => format!("no run found for run_id={id}"),
                        None => format!("no {PAPER_MODE} run exists for engine={DAEMON_ENGINE_ID}"),
                    }],
                )),
            )
                .into_response();
        }
    };

    let run_id = run.run_id;

    let mut warnings: Vec<String> = Vec::new();
    if run.mode != PAPER_MODE {
        warnings.push(format!(
            "resolved run's mode is '{}', not '{PAPER_MODE}' — this route is intended for paper lifecycle visibility",
            run.mode
        ));
    }

    // Fetch every source table for this run. All read-only, all run-scoped.
    let signal_rows = mqk_db::fetch_strategy_signal_evaluations_for_run(db, run_id, ROW_LIMIT)
        .await
        .unwrap_or_default();
    let no_trade_rows =
        mqk_db::fetch_autonomous_no_trade_diagnostics_for_run(db, run_id, ROW_LIMIT)
            .await
            .unwrap_or_default();
    let outbox_rows = mqk_db::outbox_fetch_for_supervisor(db, run_id)
        .await
        .unwrap_or_default();
    let inbox_applied_rows = mqk_db::inbox_load_all_applied_for_run(db, run_id)
        .await
        .unwrap_or_default();
    let inbox_unapplied_rows = mqk_db::inbox_load_unapplied_for_run(db, run_id)
        .await
        .unwrap_or_default();

    // DURABLE-PAPER-PORTFOLIO-AND-PNL-01E: read (never write) the durable
    // snapshot/accounting truth B4-B/B4-C/B4-D added, so
    // portfolio_truth_state/pnl_truth_state below are real computed values
    // instead of the prior hardcoded "in_memory_only_not_restart_surviving".
    // This route only ever reads these tables -- it never triggers a
    // snapshot persist or accounting replay itself.
    //
    // Run-scoped (B4 closure repair, Repair G): a non-PAPER-mode run can
    // never surface active durable portfolio/P&L truth, and the snapshot
    // lookup is scoped to exactly this run_id so a newer snapshot belonging
    // to a different run can never be attributed here. A query failure is
    // surfaced as `query_failed`, never silently collapsed into
    // `snapshot_unavailable`/`not_found`.
    let (portfolio_truth_state, pnl_truth_state, durable_accounting) = if run.mode != PAPER_MODE {
        ("unsupported_source", "unsupported_source", None)
    } else {
        let snapshot_result = mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
            db,
            "paper",
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
            run_id,
        )
        .await;
        let accounting_result = mqk_db::fetch_paper_portfolio_accounting_state(db, run_id).await;

        let portfolio_state = match &snapshot_result {
            Ok(Some(_)) => "active",
            Ok(None) => "snapshot_unavailable",
            Err(err) => {
                tracing::warn!(error = %err, run_id = %run_id, "paper_lifecycle_snapshot_query_failed");
                "query_failed"
            }
        };
        let accounting = accounting_result.as_ref().ok().and_then(|a| a.clone());
        let pnl_state = match &accounting_result {
            Err(err) => {
                tracing::warn!(error = %err, run_id = %run_id, "paper_lifecycle_accounting_query_failed");
                "query_failed"
            }
            Ok(Some(a)) if a.accounting_epoch == "incomplete" => "fill_history_incomplete",
            Ok(Some(_)) => "active",
            Ok(None) => "not_found",
        };
        (portfolio_state, pnl_state, accounting)
    };

    let signal_count = signal_rows.len();
    let generated_signal_count = signal_rows.iter().filter(|r| r.signal_generated).count();
    let no_trade_diagnostic_count = no_trade_rows.len();
    let outbox_count = outbox_rows.len();
    let inbox_count = inbox_applied_rows.len() + inbox_unapplied_rows.len();

    let broker_ack_seen = inbox_applied_rows
        .iter()
        .chain(inbox_unapplied_rows.iter())
        .any(|r| r.event_kind == "ack");
    let fill_seen = inbox_applied_rows
        .iter()
        .chain(inbox_unapplied_rows.iter())
        .any(|r| r.event_kind == "fill" || r.event_kind == "partial_fill");
    let outbox_failed_or_ambiguous = outbox_rows
        .iter()
        .any(|r| r.status == "FAILED" || r.status == "AMBIGUOUS");
    let inbox_rejected = inbox_applied_rows
        .iter()
        .chain(inbox_unapplied_rows.iter())
        .any(|r| r.event_kind == "reject");
    let order_failed_or_rejected = outbox_failed_or_ambiguous || inbox_rejected;

    let paper_order_attempted =
        outbox_count > 0 || no_trade_rows.iter().any(|r| r.paper_order_attempted);
    let live_order_attempted = no_trade_rows.iter().any(|r| r.live_order_attempted);
    if live_order_attempted {
        warnings.push(
            "a no-trade diagnostic row for this run reports live_order_attempted=true".to_string(),
        );
    }

    let (overall_lifecycle_state, full_lifecycle_visible) = classify_overall_lifecycle_state(
        signal_count,
        generated_signal_count,
        no_trade_diagnostic_count,
        outbox_count,
        fill_seen,
        order_failed_or_rejected,
        durable_accounting.as_ref(),
    );

    let summary = PaperLifecycleSummary {
        overall_lifecycle_state,
        signal_count,
        generated_signal_count,
        no_trade_diagnostic_count,
        outbox_count,
        inbox_count,
        paper_order_attempted,
        live_order_attempted,
        broker_ack_seen,
        fill_seen,
        order_failed_or_rejected,
        full_lifecycle_visible,
    };

    if portfolio_truth_state != "active" {
        warnings.push(format!(
            "durable portfolio truth is not yet available for this run (portfolio_truth_state={portfolio_truth_state})"
        ));
    }
    if pnl_truth_state == "fill_history_incomplete" {
        warnings.push(
            "durable P&L truth is incomplete for this run — a position exists whose fill \
             history does not fully explain it (see /api/v1/portfolio/durable-summary)"
                .to_string(),
        );
    }

    (
        StatusCode::OK,
        Json(PaperLifecycleResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            run_truth_state: "resolved".to_string(),
            signal_truth_state: truth_label(signal_count),
            no_trade_truth_state: truth_label(no_trade_diagnostic_count),
            outbox_truth_state: truth_label(outbox_count),
            inbox_truth_state: truth_label(inbox_count),
            portfolio_truth_state: portfolio_truth_state.to_string(),
            pnl_truth_state: pnl_truth_state.to_string(),
            run_id: Some(run_id.to_string()),
            run: Some(run_row_to_api(&run)),
            signal_evaluations: signal_rows.into_iter().map(signal_row_to_api).collect(),
            no_trade_diagnostics: no_trade_rows.into_iter().map(no_trade_row_to_api).collect(),
            outbox_orders: outbox_rows.into_iter().map(outbox_row_to_api).collect(),
            inbox_events: inbox_applied_rows
                .into_iter()
                .chain(inbox_unapplied_rows)
                .map(inbox_row_to_api)
                .collect(),
            lifecycle_summary: Some(summary),
            blockers: Vec::new(),
            warnings,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn truth_label(count: usize) -> String {
    if count > 0 {
        "present".to_string()
    } else {
        "empty".to_string()
    }
}

/// Deterministic classification of a run's lifecycle state from already
/// computed counts/flags. Pure — no I/O, fully unit-testable.
///
/// `durable_accounting` distinguishes, once a fill has been seen, whether
/// durable P&L truth (B4-D) is already available for this run:
/// - `None` — no durable accounting row exists yet: `"order_filled_pnl_pending"`.
/// - `Some` with `accounting_epoch == "complete"` — durable realized P&L is
///   available: `"order_filled_portfolio_durable_pnl_available"`.
/// - `Some` with `accounting_epoch == "incomplete"` — a position exists
///   whose fill history doesn't fully explain it:
///   `"order_filled_portfolio_durable_pnl_incomplete"`.
///
/// Returns `(overall_lifecycle_state, full_lifecycle_visible)`.
fn classify_overall_lifecycle_state(
    signal_count: usize,
    generated_signal_count: usize,
    no_trade_diagnostic_count: usize,
    outbox_count: usize,
    fill_seen: bool,
    order_failed_or_rejected: bool,
    durable_accounting: Option<&mqk_db::PaperPortfolioAccountingStateRecord>,
) -> (String, bool) {
    if fill_seen {
        let state = match durable_accounting {
            None => "order_filled_pnl_pending",
            Some(a) if a.accounting_epoch == "incomplete" => {
                "order_filled_portfolio_durable_pnl_incomplete"
            }
            Some(_) => "order_filled_portfolio_durable_pnl_available",
        };
        return (state.to_string(), true);
    }
    if order_failed_or_rejected {
        return ("order_rejected_or_failed".to_string(), true);
    }
    if outbox_count > 0 {
        return ("order_submitted_fill_pending".to_string(), true);
    }
    if no_trade_diagnostic_count > 0 || (signal_count > 0 && generated_signal_count == 0) {
        return ("no_signal_durably_explained".to_string(), true);
    }
    ("partial_visibility".to_string(), false)
}

fn empty_response(
    truth_state: &str,
    run_id: Option<String>,
    blockers: Vec<String>,
) -> PaperLifecycleResponse {
    PaperLifecycleResponse {
        canonical_route: CANONICAL.to_string(),
        truth_state: truth_state.to_string(),
        run_truth_state: match truth_state {
            "not_found" => "not_found".to_string(),
            _ => "unavailable".to_string(),
        },
        signal_truth_state: "unavailable".to_string(),
        no_trade_truth_state: "unavailable".to_string(),
        outbox_truth_state: "unavailable".to_string(),
        inbox_truth_state: "unavailable".to_string(),
        portfolio_truth_state: "unavailable".to_string(),
        pnl_truth_state: "unavailable".to_string(),
        run_id,
        run: None,
        signal_evaluations: Vec::new(),
        no_trade_diagnostics: Vec::new(),
        outbox_orders: Vec::new(),
        inbox_events: Vec::new(),
        lifecycle_summary: None,
        blockers,
        warnings: Vec::new(),
    }
}

fn run_row_to_api(r: &mqk_db::RunRow) -> PaperLifecycleRunRow {
    PaperLifecycleRunRow {
        run_id: r.run_id.to_string(),
        engine_id: r.engine_id.clone(),
        mode: r.mode.clone(),
        status: r.status.as_str().to_string(),
        started_at_utc: r.started_at_utc.to_rfc3339(),
        armed_at_utc: r.armed_at_utc.map(|t| t.to_rfc3339()),
        running_at_utc: r.running_at_utc.map(|t| t.to_rfc3339()),
        stopped_at_utc: r.stopped_at_utc.map(|t| t.to_rfc3339()),
        halted_at_utc: r.halted_at_utc.map(|t| t.to_rfc3339()),
    }
}

fn signal_row_to_api(
    r: mqk_db::StrategySignalEvaluationRecord,
) -> PaperLifecycleSignalEvaluationRow {
    PaperLifecycleSignalEvaluationRow {
        evaluation_id: r.evaluation_id.to_string(),
        ts_utc: r.ts_utc.to_rfc3339(),
        strategy_id: r.strategy_id,
        symbol: r.symbol,
        timeframe: r.timeframe,
        signal_generated: r.signal_generated,
        signal_qty: r.signal_qty,
        signal_side: r.signal_side,
        reason_code: r.reason_code,
        reason: r.reason,
        decision_stage: r.decision_stage,
    }
}

fn no_trade_row_to_api(
    r: mqk_db::AutonomousNoTradeDiagnosticRecord,
) -> PaperLifecycleNoTradeDiagnosticRow {
    PaperLifecycleNoTradeDiagnosticRow {
        diagnostic_id: r.diagnostic_id.to_string(),
        observed_at_utc: r.observed_at_utc.to_rfc3339(),
        mode: r.mode,
        session_window_state: r.session_window_state,
        arm_state: r.arm_state,
        overall_ready: r.overall_ready,
        reason_code: r.reason_code,
        reason: r.reason,
        stage: r.stage,
        paper_order_attempted: r.paper_order_attempted,
        live_order_attempted: r.live_order_attempted,
    }
}

fn outbox_row_to_api(r: mqk_db::OutboxSupervisorRow) -> PaperLifecycleOutboxRow {
    let symbol = r
        .order_json
        .get("symbol")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let side = r
        .order_json
        .get("side")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let qty = r.order_json.get("qty").and_then(|v| v.as_i64());
    PaperLifecycleOutboxRow {
        idempotency_key: r.idempotency_key,
        status: r.status,
        symbol,
        side,
        qty,
        created_at_utc: r.created_at_utc.to_rfc3339(),
        claimed_at_utc: r.claimed_at_utc.map(|t| t.to_rfc3339()),
        dispatching_at_utc: r.dispatching_at_utc.map(|t| t.to_rfc3339()),
        sent_at_utc: r.sent_at_utc.map(|t| t.to_rfc3339()),
    }
}

fn inbox_row_to_api(r: mqk_db::InboxRow) -> PaperLifecycleInboxRow {
    let internal_order_id = r
        .message_json
        .get("internal_order_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let broker_order_id = r
        .message_json
        .get("broker_order_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    PaperLifecycleInboxRow {
        inbox_id: r.inbox_id,
        broker_message_id: r.broker_message_id,
        internal_order_id,
        broker_order_id,
        event_kind: r.event_kind,
        received_at_utc: r.received_at_utc.to_rfc3339(),
        applied_at_utc: r.applied_at_utc.map(|t| t.to_rfc3339()),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_overall_lifecycle_state;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn accounting_row(epoch: &str) -> mqk_db::PaperPortfolioAccountingStateRecord {
        mqk_db::PaperPortfolioAccountingStateRecord {
            run_id: Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                b"test.paper-lifecycle.v1|accounting_row",
            ),
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 1,
            accounting_epoch: epoch.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
            source_snapshot_id: Some(Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                b"test.paper-lifecycle.v1|accounting_row.snapshot",
            )),
        }
    }

    #[test]
    fn fill_seen_without_durable_accounting_is_pnl_pending() {
        let (state, visible) = classify_overall_lifecycle_state(3, 1, 0, 1, true, true, None);
        assert_eq!(state, "order_filled_pnl_pending");
        assert!(visible);
    }

    #[test]
    fn fill_seen_with_complete_durable_accounting_is_available() {
        let complete = accounting_row("complete");
        let (state, visible) =
            classify_overall_lifecycle_state(3, 1, 0, 1, true, true, Some(&complete));
        assert_eq!(state, "order_filled_portfolio_durable_pnl_available");
        assert!(visible);
    }

    #[test]
    fn fill_seen_with_incomplete_durable_accounting_is_incomplete() {
        let incomplete = accounting_row("incomplete");
        let (state, visible) =
            classify_overall_lifecycle_state(3, 1, 0, 1, true, true, Some(&incomplete));
        assert_eq!(state, "order_filled_portfolio_durable_pnl_incomplete");
        assert!(visible);
    }

    #[test]
    fn failed_or_rejected_without_fill() {
        let (state, visible) = classify_overall_lifecycle_state(1, 1, 0, 1, false, true, None);
        assert_eq!(state, "order_rejected_or_failed");
        assert!(visible);
    }

    #[test]
    fn outbox_only_is_fill_pending() {
        let (state, visible) = classify_overall_lifecycle_state(1, 1, 0, 1, false, false, None);
        assert_eq!(state, "order_submitted_fill_pending");
        assert!(visible);
    }

    #[test]
    fn no_trade_diagnostic_explains_absence_of_order() {
        let (state, visible) = classify_overall_lifecycle_state(0, 0, 2, 0, false, false, None);
        assert_eq!(state, "no_signal_durably_explained");
        assert!(visible);
    }

    #[test]
    fn signal_evaluated_but_none_generated_explains_absence_of_order() {
        let (state, visible) = classify_overall_lifecycle_state(3, 0, 0, 0, false, false, None);
        assert_eq!(state, "no_signal_durably_explained");
        assert!(visible);
    }

    #[test]
    fn nothing_at_all_is_partial_visibility() {
        let (state, visible) = classify_overall_lifecycle_state(0, 0, 0, 0, false, false, None);
        assert_eq!(state, "partial_visibility");
        assert!(!visible);
    }
}
