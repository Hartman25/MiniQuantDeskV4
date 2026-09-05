//! Paper trading journal and evidence surface (JOUR-01).
//!
//! `GET /api/v1/paper/journal` — unified paper-trading evidence endpoint for
//! operator review.  Surfaces two independent evidence lanes:
//!
//! - **fills_lane** — fill-quality telemetry for the active run
//!   (`postgres.fill_quality_telemetry`).  Answers "what executed?"
//! - **admissions_lane** — signal-admission audit events written by the
//!   strategy-signal route at Gate 7 `Ok(true)`
//!   (`postgres.audit_events[topic=signal_ingestion]`).
//!   Answers "what signals were submitted and accepted for dispatch?"
//!
//! Both lanes carry explicit `truth_state` values.  No history is fabricated.
//! If a lane is unavailable its `rows` is always empty and `truth_state`
//! says so explicitly.
//!
//! # Truth state semantics
//!
//! | State          | Meaning                                                |
//! |----------------|--------------------------------------------------------|
//! | `"active"`     | DB + active run present; rows are authoritative.       |
//! | `"no_active_run"` | DB present but no active run; rows empty, not auth. |
//! | `"no_db"`      | No DB pool; rows empty, not authoritative.             |

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use super::portfolio_provenance::{
    classify_portfolio_provenance, validate_run_scoped_snapshot_authority, PortfolioProvenanceState,
};
use crate::api_types::{
    PaperJournalAdmissionRow, PaperJournalAdmissionsLane, PaperJournalClosedTradeRow,
    PaperJournalClosedTradesLane, PaperJournalFillRow, PaperJournalFillsLane, PaperJournalResponse,
};
use crate::state::{build_closed_trade_projection, AppState, ClosureFragment};

const CANONICAL: &str = "/api/v1/paper/journal";

/// Build a no-db (or no-active-run) journal response.
fn unavailable_response(truth_state: &str) -> Response {
    (
        StatusCode::OK,
        Json(PaperJournalResponse {
            canonical_route: CANONICAL.to_string(),
            run_id: None,
            fills_lane: PaperJournalFillsLane {
                truth_state: truth_state.to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            },
            admissions_lane: PaperJournalAdmissionsLane {
                truth_state: truth_state.to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            },
            closed_trades_lane: PaperJournalClosedTradesLane {
                truth_state: truth_state.to_string(),
                backend: "unavailable".to_string(),
                accounting_epoch: None,
                accounting_epoch_reason: None,
                sum_gross_realized_pnl_micros: None,
                accounting_provenance_state: None,
                canonical_last_applied_inbox_id: None,
                accounting_last_applied_inbox_id: None,
                accounting_watermark_state: None,
                rows: vec![],
            },
        }),
    )
        .into_response()
}

/// Map internal read-model fragments into the API row shape. Pure/no IO.
fn map_closure_fragments(
    run_id: uuid::Uuid,
    fragments: &[ClosureFragment],
) -> Vec<PaperJournalClosedTradeRow> {
    fragments
        .iter()
        .map(|f| {
            let (open_strategy_id, open_strategy_semantic_fingerprint) =
                f.open_lineage.identity_pair();
            let (close_strategy_id, close_strategy_semantic_fingerprint) =
                f.close_lineage.identity_pair();
            PaperJournalClosedTradeRow {
                run_id,
                symbol: f.symbol.clone(),
                direction: f.direction.to_string(),
                qty: f.qty,
                entry_price_micros: f.entry_price_micros,
                exit_price_micros: f.exit_price_micros,
                gross_realized_pnl_micros: f.gross_realized_pnl_micros,
                open_inbox_id: f.open_inbox_id,
                open_internal_order_id: f.open_internal_order_id.clone(),
                close_inbox_id: f.close_inbox_id,
                close_internal_order_id: f.close_internal_order_id.clone(),
                open_strategy_id,
                open_strategy_semantic_fingerprint,
                close_strategy_id,
                close_strategy_semantic_fingerprint,
                attribution_state: f.attribution.as_str().to_string(),
            }
        })
        .collect()
}

/// Pure decision for the closed-trades lane's exposed `truth_state` plus the
/// bounded `accounting_watermark_state` label, given the shared portfolio
/// provenance verdict and the same-watermark / durable-realized-P&L parity
/// checks. No I/O -- fully unit-testable (WAVE05-STRATEGY-CLOSED-TRADE-
/// READ-MODEL-01-REPAIR-01).
///
/// `truth_state` is `"active"` only when ALL of the following hold:
/// - `provenance == Active` (shared classifier: run-scoped snapshot exists,
///   passes independent authority validation, and the durable accounting
///   row's `source_snapshot_id` matches it with `accounting_epoch ==
///   "complete"`);
/// - the durable accounting watermark exactly equals the canonical replay
///   watermark (Defect 2 repair -- a stale accounting row must never be
///   treated as same-watermark current merely because realized P&L happens
///   to still match);
/// - the durable `realized_pnl_micros` exactly equals the canonical
///   projection's summed gross realized P&L.
///
/// Any other `provenance` value fails closed to `"query_failed"` (when the
/// classifier itself reports a query failure) or `"incomplete"` (every other
/// non-active state -- `fill_history_incomplete`, `not_found`,
/// `accounting_epoch_unavailable`, `accounting_snapshot_mismatch`,
/// `unsupported_source`, `invalid_snapshot`) -- the exact classification is
/// still exposed separately via `accounting_provenance_state`, never hidden
/// behind this coarser label.
struct ClosedTradesAuthority {
    truth_state: &'static str,
    /// `Some("same_watermark" | "accounting_watermark_mismatch")` only when
    /// `provenance == Active` -- a same-watermark comparison is only
    /// meaningful once the shared classifier has already proven the
    /// snapshot/accounting relationship current. `None` otherwise.
    accounting_watermark_state: Option<&'static str>,
    /// `true` when the durable `realized_pnl_micros` contradicted the
    /// canonical projection's sum despite matching watermarks -- distinct
    /// from an ordinary watermark mismatch, which is never called
    /// "parity_failed" (a fresher, unpersisted accounting refresh is
    /// expected/benign, not a contradiction).
    durable_pnl_mismatch: bool,
}

fn classify_closed_trades_authority(
    provenance: PortfolioProvenanceState,
    canonical_last_applied_inbox_id: i64,
    accounting_last_applied_inbox_id: Option<i64>,
    canonical_sum_realized_pnl_micros: i64,
    accounting_realized_pnl_micros: Option<i64>,
) -> ClosedTradesAuthority {
    if provenance != PortfolioProvenanceState::Active {
        return ClosedTradesAuthority {
            truth_state: if provenance == PortfolioProvenanceState::QueryFailed {
                "query_failed"
            } else {
                "incomplete"
            },
            accounting_watermark_state: None,
            durable_pnl_mismatch: false,
        };
    }

    // `provenance == Active` guarantees (by `classify_portfolio_provenance`'s
    // own contract) that a durable accounting row exists, so both `Option`s
    // below are `Some` in practice -- compared via `Option` equality anyway
    // so a hypothetical `None` fails closed (mismatch) rather than panicking.
    if accounting_last_applied_inbox_id != Some(canonical_last_applied_inbox_id) {
        return ClosedTradesAuthority {
            truth_state: "incomplete",
            accounting_watermark_state: Some("accounting_watermark_mismatch"),
            durable_pnl_mismatch: false,
        };
    }

    if accounting_realized_pnl_micros != Some(canonical_sum_realized_pnl_micros) {
        return ClosedTradesAuthority {
            truth_state: "parity_failed",
            accounting_watermark_state: Some("same_watermark"),
            durable_pnl_mismatch: true,
        };
    }

    ClosedTradesAuthority {
        truth_state: "active",
        accounting_watermark_state: Some("same_watermark"),
        durable_pnl_mismatch: false,
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/paper/journal
// ---------------------------------------------------------------------------

pub(crate) async fn paper_journal(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return unavailable_response("no_db");
    };

    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return unavailable_response("no_active_run");
    };

    // --- Fills lane: from fill_quality_telemetry ---
    //
    // truth_state is "active" only when the query succeeds — including
    // authoritative empty (zero fills is a valid active state).
    // A query failure is "query_failed": the lane is present but non-authoritative.
    let (fills_truth_state, fills_backend, api_fills) =
        match mqk_db::fetch_fill_quality_telemetry_recent(db, run_id, 100).await {
            Ok(rows) => {
                // WAVE05-PAPER-JOURNAL-STRATEGY-LINEAGE-01: recover durable
                // strategy identity per fill from the exact originating
                // outbox row (internal_order_id -> idempotency_key), proven
                // run-coherent (outbox.run_id == fill.run_id) and parsed with
                // explicit field-level validation. A lineage LOOKUP error
                // (DB failure) is a transient failure, not a "no strategy"
                // fact — it degrades the whole lane to query_failed rather
                // than emitting a row with a silently omitted or invented
                // attribution alongside otherwise-active rows. Row-level
                // `lineage_missing` (join resolves to no outbox row) and
                // `lineage_invalid` (cross-run mismatch or malformed
                // attribution fields) are distinct, non-transient truths and
                // are kept as authoritative "active" rows rather than
                // failing the lane, per this route's existing truth_state
                // contract of fabricating nothing while still surfacing
                // genuine query failures at the lane level.
                let mut mapped: Vec<PaperJournalFillRow> = Vec::with_capacity(rows.len());
                let mut lineage_lookup_failed = false;
                for r in rows {
                    let lineage = match mqk_db::fetch_fill_strategy_lineage(
                        db,
                        r.run_id,
                        &r.internal_order_id,
                    )
                    .await
                    {
                        Ok(l) => l,
                        Err(e) => {
                            tracing::warn!(
                                "paper_journal strategy-lineage lookup failed for \
                                 internal_order_id={} (non-fatal, degrades lane): {e}",
                                r.internal_order_id
                            );
                            lineage_lookup_failed = true;
                            break;
                        }
                    };
                    let (
                        strategy_id,
                        strategy_semantic_fingerprint,
                        strategy_attribution_state,
                        strategy_attribution_reason,
                    ) = match lineage {
                        mqk_db::FillStrategyLineage::Resolved {
                            strategy_id,
                            strategy_semantic_fingerprint,
                        } => {
                            let state = if strategy_id.is_some() {
                                "attributed"
                            } else {
                                "unattributed_manual"
                            };
                            (strategy_id, strategy_semantic_fingerprint, state, None)
                        }
                        mqk_db::FillStrategyLineage::OriginatingOrderMissing => {
                            (None, None, "lineage_missing", None)
                        }
                        mqk_db::FillStrategyLineage::Invalid { reason_code } => {
                            (None, None, "lineage_invalid", Some(reason_code))
                        }
                    };
                    mapped.push(PaperJournalFillRow {
                        telemetry_id: r.telemetry_id,
                        run_id: r.run_id,
                        internal_order_id: r.internal_order_id,
                        broker_order_id: r.broker_order_id,
                        broker_fill_id: r.broker_fill_id,
                        broker_message_id: r.broker_message_id,
                        symbol: r.symbol,
                        side: r.side,
                        ordered_qty: r.ordered_qty,
                        fill_qty: r.fill_qty,
                        fill_price_micros: r.fill_price_micros,
                        reference_price_micros: r.reference_price_micros,
                        slippage_bps: r.slippage_bps,
                        submit_ts_utc: r.submit_ts_utc.map(|t| t.to_rfc3339()),
                        fill_received_at_utc: r.fill_received_at_utc.to_rfc3339(),
                        submit_to_fill_ms: r.submit_to_fill_ms,
                        fill_kind: r.fill_kind,
                        provenance_ref: r.provenance_ref,
                        created_at_utc: r.created_at_utc.to_rfc3339(),
                        strategy_id,
                        strategy_semantic_fingerprint,
                        strategy_attribution_state: strategy_attribution_state.to_string(),
                        strategy_attribution_reason,
                    });
                }
                if lineage_lookup_failed {
                    ("query_failed", "postgres.fill_quality_telemetry", vec![])
                } else {
                    ("active", "postgres.fill_quality_telemetry", mapped)
                }
            }
            Err(e) => {
                tracing::warn!("paper_journal fills query failed (non-fatal): {e}");
                ("query_failed", "postgres.fill_quality_telemetry", vec![])
            }
        };

    // --- Admissions lane: from audit_events topic='signal_ingestion' ---
    //
    // Same truth_state contract: "active" only on query success; "query_failed"
    // on error.  Empty rows on success is authoritative zero — the operator
    // has submitted no admitted signals for this run.
    let admissions_query = sqlx::query(
        r#"
        select event_id, run_id, ts_utc, payload
        from audit_events
        where topic = 'signal_ingestion'
          and event_type = 'signal.admitted'
          and run_id = $1
        order by ts_utc desc
        limit 200
        "#,
    )
    .bind(run_id)
    .fetch_all(db)
    .await;

    let (admissions_truth_state, admissions_backend, api_admissions) = match admissions_query {
        Ok(rows) => {
            let mapped: Vec<PaperJournalAdmissionRow> = rows
                .into_iter()
                .filter_map(|row| {
                    let event_id: uuid::Uuid = row.try_get("event_id").ok()?;
                    let ts_utc: chrono::DateTime<chrono::Utc> = row.try_get("ts_utc").ok()?;
                    let payload: serde_json::Value = row.try_get("payload").ok()?;

                    let signal_id = payload
                        .get("signal_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    let strategy_id = payload
                        .get("strategy_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    let symbol = payload
                        .get("symbol")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    let side = payload
                        .get("side")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    let qty = payload.get("qty").and_then(|v| v.as_i64()).unwrap_or(0);

                    Some(PaperJournalAdmissionRow {
                        event_id: event_id.to_string(),
                        ts_utc: ts_utc.to_rfc3339(),
                        signal_id,
                        strategy_id,
                        symbol,
                        side,
                        qty,
                        run_id: run_id.to_string(),
                        provenance_ref: format!("audit_events:{}", event_id),
                    })
                })
                .collect();
            (
                "active",
                "postgres.audit_events[topic=signal_ingestion]",
                mapped,
            )
        }
        Err(e) => {
            tracing::warn!("paper_journal admissions query failed (non-fatal): {e}");
            (
                "query_failed",
                "postgres.audit_events[topic=signal_ingestion]",
                vec![],
            )
        }
    };

    // --- Closed-trades lane: attributed FIFO closure projection ---
    //
    // WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01: reuses the exact canonical
    // effective-fill replay (via `build_closed_trade_projection`) and P1's
    // strategy-lineage resolver. `truth_state` distinguishes a proven-active
    // projection from an accounting-incomplete one (durable epoch says this
    // run's fill history cannot explain all inherited positions) from a
    // fail-closed parity contradiction (never surfaced as authoritative).
    //
    // WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01-REPAIR-01: this lane must
    // never hand-roll its own accounting-provenance classification. Prior to
    // this repair it compared only `accounting_epoch == "complete"` plus a
    // bare realized-P&L equality, which reopens the exact Bundle-4 defect
    // class (a stale accounting row pointing at a superseded snapshot could
    // still be reported "active") and additionally could not detect a
    // durable accounting row whose replay watermark had gone stale despite
    // unchanged realized P&L. This lane now reuses the exact shared
    // classifier (`classify_portfolio_provenance`) and run-scoped snapshot
    // authority validator (`validate_run_scoped_snapshot_authority`) every
    // other durable-portfolio surface (`durable_portfolio.rs`,
    // `paper_lifecycle.rs`) already uses, then additionally requires the
    // canonical replay watermark to exactly match the durable accounting
    // row's `last_applied_inbox_id` before "active" may be reported (see
    // `classify_closed_trades_authority`).
    const CLOSED_TRADES_BACKEND: &str = "mqk_daemon.closed_trade_attribution";

    let run_record = mqk_db::fetch_run(db, run_id).await;
    if let Err(e) = &run_record {
        tracing::warn!("paper_journal closed_trades run lookup failed (non-fatal): {e}");
    }
    let run_query_failed = run_record.is_err();
    let is_paper_mode = run_record
        .as_ref()
        .map(|r| r.mode == "PAPER")
        .unwrap_or(false);

    #[allow(clippy::type_complexity)]
    let (
        closed_trades_truth_state,
        closed_trades_epoch,
        closed_trades_epoch_reason,
        closed_trades_sum,
        closed_trades_provenance_state,
        closed_trades_canonical_watermark,
        closed_trades_accounting_watermark,
        closed_trades_watermark_state,
        api_closed_trades,
    ): (
        &str,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<&'static str>,
        Option<i64>,
        Option<i64>,
        Option<&'static str>,
        Vec<PaperJournalClosedTradeRow>,
    ) = if run_query_failed {
        (
            "query_failed",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            vec![],
        )
    } else {
        match build_closed_trade_projection(db, run_id).await {
            Ok(proj)
                if proj.sum_gross_realized_pnl_micros != proj.canonical_realized_pnl_micros =>
            {
                tracing::error!(
                    "paper_journal closed_trades parity failure: projection_sum={} \
                     canonical_replay_realized_pnl={} run_id={run_id}",
                    proj.sum_gross_realized_pnl_micros,
                    proj.canonical_realized_pnl_micros,
                );
                (
                    "parity_failed",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    vec![],
                )
            }
            Ok(proj) => {
                let snapshot_result = mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
                    db,
                    "paper",
                    mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
                    run_id,
                )
                .await;
                if let Err(err) = &snapshot_result {
                    tracing::warn!(
                        error = %err, run_id = %run_id,
                        "paper_journal_closed_trades_snapshot_query_failed"
                    );
                }
                let accounting_result =
                    mqk_db::fetch_paper_portfolio_accounting_state(db, run_id).await;
                if let Err(err) = &accounting_result {
                    tracing::warn!(
                        error = %err, run_id = %run_id,
                        "paper_journal_closed_trades_accounting_query_failed"
                    );
                }

                let snapshot_query_failed = snapshot_result.is_err();
                let accounting_query_failed = accounting_result.is_err();
                // B4 final read-side authority repair (reused, not
                // reimplemented): a selected snapshot that fails independent
                // authority validation must never be paired with an
                // accounting row.
                let snapshot_invalid = snapshot_result
                    .as_ref()
                    .ok()
                    .and_then(|s| s.as_ref())
                    .is_some_and(|s| validate_run_scoped_snapshot_authority(s, run_id).is_err());
                let snapshot_id = snapshot_result
                    .as_ref()
                    .ok()
                    .and_then(|s| s.as_ref())
                    .map(|s| s.snapshot.snapshot_id);
                let accounting = accounting_result.as_ref().ok().and_then(|a| a.clone());

                let provenance = classify_portfolio_provenance(
                    is_paper_mode,
                    snapshot_query_failed,
                    accounting_query_failed,
                    snapshot_id,
                    snapshot_invalid,
                    accounting.as_ref(),
                );

                let epoch = accounting.as_ref().map(|a| a.accounting_epoch.clone());
                let epoch_reason = accounting
                    .as_ref()
                    .and_then(|a| a.accounting_epoch_reason.clone());
                let accounting_watermark = accounting.as_ref().map(|a| a.last_applied_inbox_id);
                let accounting_realized_pnl = accounting.as_ref().map(|a| a.realized_pnl_micros);

                let authority = classify_closed_trades_authority(
                    provenance,
                    proj.canonical_last_applied_inbox_id,
                    accounting_watermark,
                    proj.sum_gross_realized_pnl_micros,
                    accounting_realized_pnl,
                );

                if authority.durable_pnl_mismatch {
                    tracing::error!(
                        "paper_journal closed_trades durable parity failure: \
                         projection_sum={} durable_realized_pnl={accounting_realized_pnl:?} \
                         run_id={run_id}",
                        proj.sum_gross_realized_pnl_micros,
                    );
                }

                // `sum_gross_realized_pnl_micros`/`rows` are populated only
                // for "active"/"incomplete" -- never for "parity_failed" or
                // "query_failed" (unchanged existing API contract).
                let (sum, epoch_out, epoch_reason_out, rows) =
                    if matches!(authority.truth_state, "active" | "incomplete") {
                        (
                            Some(proj.sum_gross_realized_pnl_micros),
                            epoch,
                            epoch_reason,
                            map_closure_fragments(run_id, &proj.fragments),
                        )
                    } else {
                        (None, None, None, vec![])
                    };

                (
                    authority.truth_state,
                    epoch_out,
                    epoch_reason_out,
                    sum,
                    Some(provenance.as_str()),
                    Some(proj.canonical_last_applied_inbox_id),
                    accounting_watermark,
                    authority.accounting_watermark_state,
                    rows,
                )
            }
            Err(e) => {
                tracing::warn!("paper_journal closed_trades projection failed (non-fatal): {e}");
                (
                    "query_failed",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    vec![],
                )
            }
        }
    };

    (
        StatusCode::OK,
        Json(PaperJournalResponse {
            canonical_route: CANONICAL.to_string(),
            run_id: Some(run_id.to_string()),
            fills_lane: PaperJournalFillsLane {
                truth_state: fills_truth_state.to_string(),
                backend: fills_backend.to_string(),
                rows: api_fills,
            },
            admissions_lane: PaperJournalAdmissionsLane {
                truth_state: admissions_truth_state.to_string(),
                backend: admissions_backend.to_string(),
                rows: api_admissions,
            },
            closed_trades_lane: PaperJournalClosedTradesLane {
                truth_state: closed_trades_truth_state.to_string(),
                backend: CLOSED_TRADES_BACKEND.to_string(),
                accounting_epoch: closed_trades_epoch,
                accounting_epoch_reason: closed_trades_epoch_reason,
                sum_gross_realized_pnl_micros: closed_trades_sum,
                accounting_provenance_state: closed_trades_provenance_state.map(str::to_string),
                canonical_last_applied_inbox_id: closed_trades_canonical_watermark,
                accounting_last_applied_inbox_id: closed_trades_accounting_watermark,
                accounting_watermark_state: closed_trades_watermark_state.map(str::to_string),
                rows: api_closed_trades,
            },
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{classify_closed_trades_authority, PortfolioProvenanceState};

    #[test]
    fn active_provenance_with_matching_watermark_and_pnl_is_active() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            5,
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "active");
        assert_eq!(authority.accounting_watermark_state, Some("same_watermark"));
        assert!(!authority.durable_pnl_mismatch);
    }

    /// RED6 target: CT15's exact counterexample -- realized P&L still
    /// matches, but the durable accounting watermark is stale relative to
    /// the canonical replay. Must never be "active" or "parity_failed".
    #[test]
    fn active_provenance_with_stale_watermark_and_matching_pnl_is_incomplete_not_active() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            6, // canonical watermark advanced past accounting's
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "incomplete");
        assert_eq!(
            authority.accounting_watermark_state,
            Some("accounting_watermark_mismatch")
        );
        assert!(
            !authority.durable_pnl_mismatch,
            "a stale watermark with unchanged realized P&L must never be reported as a P&L contradiction"
        );
    }

    #[test]
    fn active_provenance_with_matching_watermark_but_mismatched_pnl_is_parity_failed() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            5,
            Some(5),
            100,
            Some(999),
        );
        assert_eq!(authority.truth_state, "parity_failed");
        assert_eq!(authority.accounting_watermark_state, Some("same_watermark"));
        assert!(authority.durable_pnl_mismatch);
    }

    /// RED5 target: CT14's exact counterexample -- the shared classifier
    /// reports a stale-snapshot mismatch; realized-P&L equality must not
    /// bypass it.
    #[test]
    fn non_active_provenance_never_reaches_active_regardless_of_watermark_or_pnl_equality() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::AccountingSnapshotMismatch,
            5,
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "incomplete");
        assert_eq!(authority.accounting_watermark_state, None);
        assert!(!authority.durable_pnl_mismatch);
    }

    #[test]
    fn query_failed_provenance_maps_to_query_failed_not_incomplete() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::QueryFailed,
            5,
            None,
            100,
            None,
        );
        assert_eq!(authority.truth_state, "query_failed");
        assert_eq!(authority.accounting_watermark_state, None);
    }

    #[test]
    fn every_other_non_active_state_is_incomplete() {
        for provenance in [
            PortfolioProvenanceState::FillHistoryIncomplete,
            PortfolioProvenanceState::AccountingEpochUnavailable,
            PortfolioProvenanceState::NotFound,
            PortfolioProvenanceState::UnsupportedSource,
            PortfolioProvenanceState::InvalidSnapshot,
        ] {
            let authority = classify_closed_trades_authority(provenance, 5, None, 100, None);
            assert_eq!(
                authority.truth_state, "incomplete",
                "provenance={provenance:?}"
            );
            assert_eq!(
                authority.accounting_watermark_state, None,
                "provenance={provenance:?}"
            );
        }
    }
}
