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

use crate::api_types::{
    PaperJournalAdmissionRow, PaperJournalAdmissionsLane, PaperJournalClosedTradeRow,
    PaperJournalClosedTradesLane, PaperJournalFillRow, PaperJournalFillsLane, PaperJournalResponse,
};
use crate::state::{resolve_authoritative_closed_trade_view, AppState, ClosureFragment};

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
    // WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01 (P3.1): this lane now resolves
    // through the single shared `resolve_authoritative_closed_trade_view` --
    // the SAME authority computation `routes/strategy_performance.rs`
    // consumes. See that function's doc for the full provenance/parity/
    // watermark contract (unchanged from WAVE05-STRATEGY-CLOSED-TRADE-READ-
    // MODEL-01 / -REPAIR-01).
    const CLOSED_TRADES_BACKEND: &str = "mqk_daemon.closed_trade_attribution";

    let view = resolve_authoritative_closed_trade_view(db, run_id).await;
    let closed_trades_truth_state = view.truth_state;
    let closed_trades_epoch = view.accounting_epoch;
    let closed_trades_epoch_reason = view.accounting_epoch_reason;
    let closed_trades_sum = view.sum_gross_realized_pnl_micros;
    let closed_trades_provenance_state = view.accounting_provenance_state;
    let closed_trades_canonical_watermark = view.canonical_last_applied_inbox_id;
    let closed_trades_accounting_watermark = view.accounting_last_applied_inbox_id;
    let closed_trades_watermark_state = view.accounting_watermark_state;
    let api_closed_trades = map_closure_fragments(run_id, &view.fragments);

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
