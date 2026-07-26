// core-rs/crates/mqk-daemon/src/routes/durable_portfolio.rs
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01E: read-only durable portfolio truth.
//
// GET /api/v1/portfolio/durable-summary
// GET /api/v1/portfolio/durable-positions
// GET /api/v1/portfolio/durable-snapshots?limit=20
//
// Additive to the existing (broker-snapshot-derived, in-memory-only)
// /api/v1/portfolio/summary and /api/v1/portfolio/positions routes -- these
// three read the durable tables B4-B/B4-C/B4-D added instead, and survive a
// daemon restart. GET-only: no route in this file ever inserts, updates, or
// deletes a row. `null` always means unavailable/unproven -- never zero; a
// true zero is always the literal numeric `0`.
//
// Run resolution mirrors `routes/paper_lifecycle.rs` exactly: an explicit
// `?run_id=` query param, or else the latest durable PAPER run for this
// engine -- never in-memory active-run state, so this route keeps working
// identically before and after a restart.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::portfolio::{aggregate_positions_pnl, compute_broker_positions_pnl, resolve_daily_pnl};
use super::portfolio_provenance::{classify_portfolio_provenance, PortfolioProvenanceState};
use crate::api_types::{
    PortfolioDurablePositionRow, PortfolioDurablePositionsResponse, PortfolioDurableSnapshotRow,
    PortfolioDurableSnapshotsResponse, PortfolioDurableSummaryResponse,
};
use crate::state::AppState;

/// Same engine identity every other durable-run route in this crate uses.
const DAEMON_ENGINE_ID: &str = "mqk-daemon";
const PAPER_MODE: &str = "PAPER";

/// Default positions-P&L mark timeframe, matching the existing
/// `/api/v1/portfolio/summary`/`/positions` default.
const DEFAULT_TIMEFRAME: &str = "1D";

/// Mirrors `routes/system.rs::BROKER_SNAPSHOT_STALE_SECS` -- the same
/// staleness threshold for an External (Alpaca) broker snapshot, applied
/// here to the durable snapshot's `captured_at_utc` instead of the
/// in-memory one.
const DURABLE_SNAPSHOT_STALE_SECS: i64 = 180;

#[derive(Debug, Deserialize)]
pub(crate) struct RunIdParam {
    pub run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SnapshotsListParams {
    pub limit: Option<i64>,
}

/// Bounded, closed-vocabulary run resolution outcome. Distinguishes an
/// explicit-run query failure (`QueryFailed`) from a genuine absence
/// (`NotFound`) — the prior `.ok()` collapse could not tell these apart,
/// silently reporting a transient DB failure as an ordinary not-found run
/// (B4 closure repair, Defect 5).
pub(crate) enum RunResolution {
    Found(Box<mqk_db::RunRow>),
    NotFound,
    QueryFailed,
}

/// `true` when `err`'s cause chain bottoms out at `sqlx::Error::RowNotFound`
/// — i.e. the query executed successfully and simply found no row, as
/// opposed to a connection/transport/syntax failure.
fn is_row_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::Error>()
            .is_some_and(|e| matches!(e, sqlx::Error::RowNotFound))
    })
}

/// Fixed, bounded message for any durable-portfolio query failure. Never
/// includes the raw DB error, SQL text, connection string, or any other
/// internal detail on the public wire — the real error is logged
/// server-side via `tracing::warn!` at the call site instead.
pub(crate) const QUERY_FAILED_MESSAGE: &str = "a durable portfolio query failed";

pub(crate) async fn resolve_run(db: &sqlx::PgPool, explicit_run_id: Option<Uuid>) -> RunResolution {
    if let Some(run_id) = explicit_run_id {
        match mqk_db::fetch_run(db, run_id).await {
            Ok(run) => RunResolution::Found(Box::new(run)),
            Err(err) if is_row_not_found(&err) => RunResolution::NotFound,
            Err(err) => {
                tracing::warn!(error = %err, run_id = %run_id, "durable_portfolio_run_query_failed");
                RunResolution::QueryFailed
            }
        }
    } else {
        match mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, PAPER_MODE).await {
            Ok(Some(run)) => RunResolution::Found(Box::new(run)),
            Ok(None) => RunResolution::NotFound,
            Err(err) => {
                tracing::warn!(error = %err, "durable_portfolio_latest_run_query_failed");
                RunResolution::QueryFailed
            }
        }
    }
}

/// Fixed bounded message for an invalid `run_id` query param — never echoes
/// the caller-supplied raw value back onto the wire.
pub(crate) const INVALID_RUN_ID_MESSAGE: &str = "run_id query parameter is not a valid UUID";

fn parse_explicit_run_id(raw: Option<&str>) -> Result<Option<Uuid>, &'static str> {
    match raw {
        Some(s) => s
            .parse::<Uuid>()
            .map(Some)
            .map_err(|_| INVALID_RUN_ID_MESSAGE),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/durable-summary
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_durable_summary(
    State(st): State<Arc<AppState>>,
    Query(params): Query<RunIdParam>,
) -> impl IntoResponse {
    let explicit_run_id = match parse_explicit_run_id(params.run_id.as_deref()) {
        Ok(id) => id,
        Err(detail) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "detail": detail})),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(unavailable_summary(
                "db_unavailable",
                "no_db_pool_configured",
            )),
        )
            .into_response();
    };

    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", QUERY_FAILED_MESSAGE)),
            )
                .into_response();
        }
        RunResolution::NotFound => {
            return (
                StatusCode::OK,
                Json(unavailable_summary("not_found", "no run resolved")),
            )
                .into_response();
        }
    };

    // A run resolved to a non-PAPER mode can never supply active Paper+Alpaca
    // portfolio truth -- fail closed rather than silently reading whatever
    // snapshot/accounting rows happen to share its run_id.
    if run.mode != PAPER_MODE {
        return (
            StatusCode::OK,
            Json(unavailable_summary(
                "unsupported_source",
                "resolved run is not a PAPER-mode run",
            )),
        )
            .into_response();
    }

    let snapshot = match mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run.run_id,
    )
    .await
    {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run.run_id, "durable_portfolio_snapshot_query_failed");
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", QUERY_FAILED_MESSAGE)),
            )
                .into_response();
        }
    };

    let accounting = match mqk_db::fetch_paper_portfolio_accounting_state(db, run.run_id).await {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run.run_id, "durable_portfolio_accounting_query_failed");
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", QUERY_FAILED_MESSAGE)),
            )
                .into_response();
        }
    };

    let mut blockers = Vec::new();

    let Some(snapshot) = snapshot else {
        // No durable snapshot yet -- accounting/pnl fields stay unavailable
        // too, since unrealized P&L needs the snapshot's positions.
        let (
            accounting_truth_state,
            accounting_epoch,
            accounting_epoch_reason,
            accounting_source_snapshot_id,
            last_applied_inbox_id,
            realized_pnl,
            realized_pnl_truth_state,
            realized_pnl_reason,
            fees,
            cash_movement,
        ) = accounting_fields(None, accounting.as_ref());
        return (
            StatusCode::OK,
            Json(PortfolioDurableSummaryResponse {
                truth_state: "snapshot_unavailable".to_string(),
                snapshot_truth_state: "snapshot_unavailable".to_string(),
                snapshot_id: None,
                captured_at_utc: None,
                source: None,
                deployment_mode: None,
                account_equity: None,
                cash: None,
                currency: None,
                run_id: Some(run.run_id.to_string()),
                operation_id: None,
                accounting_truth_state,
                accounting_epoch,
                accounting_epoch_reason,
                accounting_source_snapshot_id,
                last_applied_inbox_id,
                realized_pnl,
                realized_pnl_truth_state,
                realized_pnl_unavailable_reason: realized_pnl_reason,
                fees,
                cumulative_cash_movement: cash_movement,
                unrealized_pnl: None,
                unrealized_pnl_truth_state: "snapshot_unavailable".to_string(),
                unrealized_pnl_unavailable_reason: Some(
                    "no durable snapshot exists yet".to_string(),
                ),
                daily_pnl: None,
                daily_pnl_truth_state: "snapshot_unavailable".to_string(),
                daily_pnl_unavailable_reason: Some("no durable snapshot exists yet".to_string()),
                blockers: vec!["no durable Paper+Alpaca snapshot exists yet".to_string()],
            }),
        )
            .into_response();
    };

    let age_secs = (Utc::now() - snapshot.snapshot.captured_at_utc).num_seconds();
    let snapshot_truth_state = if age_secs > DURABLE_SNAPSHOT_STALE_SECS {
        blockers.push(format!(
            "durable snapshot is {age_secs}s old (> {DURABLE_SNAPSHOT_STALE_SECS}s threshold)"
        ));
        "snapshot_stale"
    } else {
        "active"
    };

    let account_equity =
        snapshot.snapshot.equity_micros as f64 / mqk_portfolio::MICROS_SCALE as f64;
    let cash = snapshot.snapshot.cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64;

    let (
        accounting_truth_state,
        accounting_epoch,
        accounting_epoch_reason,
        accounting_source_snapshot_id,
        last_applied_inbox_id,
        realized_pnl,
        realized_pnl_truth_state,
        realized_pnl_reason,
        fees,
        cash_movement,
    ) = accounting_fields(Some(snapshot.snapshot.snapshot_id), accounting.as_ref());
    if accounting_truth_state == "accounting_snapshot_mismatch" {
        blockers.push(
            "accounting row's source snapshot does not match the currently selected durable snapshot"
                .to_string(),
        );
    }

    // Unrealized P&L reuses the existing broker-position mark lookup,
    // driven off the durable snapshot's positions rather than the
    // in-memory one.
    let broker_positions: Vec<mqk_schemas::BrokerPosition> = snapshot
        .positions
        .iter()
        .map(|p| mqk_schemas::BrokerPosition {
            symbol: p.symbol.clone(),
            qty: p.qty_signed.to_string(),
            avg_price: (p.avg_entry_price_micros as f64 / mqk_portfolio::MICROS_SCALE as f64)
                .to_string(),
        })
        .collect();
    let pnl_by_symbol =
        compute_broker_positions_pnl(&st, &broker_positions, DEFAULT_TIMEFRAME).await;
    let (unrealized_pnl, unrealized_pnl_truth_state, unrealized_pnl_unavailable_reason) =
        aggregate_positions_pnl(&pnl_by_symbol);

    let daily = resolve_daily_pnl(&st, account_equity, Utc::now()).await;

    (
        StatusCode::OK,
        Json(PortfolioDurableSummaryResponse {
            truth_state: snapshot_truth_state.to_string(),
            snapshot_truth_state: snapshot_truth_state.to_string(),
            snapshot_id: Some(snapshot.snapshot.snapshot_id.to_string()),
            captured_at_utc: Some(snapshot.snapshot.captured_at_utc.to_rfc3339()),
            source: Some(snapshot.snapshot.source.clone()),
            deployment_mode: Some(snapshot.snapshot.deployment_mode.clone()),
            account_equity: Some(account_equity),
            cash: Some(cash),
            currency: Some(snapshot.snapshot.currency.clone()),
            run_id: Some(run.run_id.to_string()),
            operation_id: snapshot.snapshot.operation_id.map(|id| id.to_string()),
            accounting_truth_state,
            accounting_epoch,
            accounting_epoch_reason,
            accounting_source_snapshot_id,
            last_applied_inbox_id,
            realized_pnl,
            realized_pnl_truth_state,
            realized_pnl_unavailable_reason: realized_pnl_reason,
            fees,
            cumulative_cash_movement: cash_movement,
            unrealized_pnl,
            unrealized_pnl_truth_state,
            unrealized_pnl_unavailable_reason,
            daily_pnl: daily.daily_pnl,
            daily_pnl_truth_state: daily.truth_state,
            daily_pnl_unavailable_reason: daily.unavailable_reason,
            blockers,
        }),
    )
        .into_response()
}

/// Projects the shared [`classify_portfolio_provenance`] verdict into the
/// granular fields `PortfolioDurableSummaryResponse` exposes. `snapshot_id`
/// is the currently-selected run-scoped durable snapshot's id (`None` if no
/// such snapshot exists yet for this run) -- this is what lets the
/// classifier detect a stale/mismatched accounting row instead of the prior
/// implementation, which never compared `accounting.source_snapshot_id`
/// against the selected snapshot at all (B4 closure repair, Phase B).
#[allow(clippy::type_complexity)]
fn accounting_fields(
    snapshot_id: Option<Uuid>,
    accounting: Option<&mqk_db::PaperPortfolioAccountingStateRecord>,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<f64>,
    String,
    Option<String>,
    Option<f64>,
    Option<f64>,
) {
    const SCALE: f64 = mqk_portfolio::MICROS_SCALE as f64;
    let state = classify_portfolio_provenance(true, false, false, snapshot_id, accounting);
    let truth_state = state.as_str().to_string();

    match state {
        PortfolioProvenanceState::NotFound => (
            truth_state.clone(),
            None,
            None,
            None,
            None,
            None,
            truth_state,
            Some("no accounting rows exist yet for this run".to_string()),
            None,
            None,
        ),
        PortfolioProvenanceState::AccountingEpochUnavailable => {
            let row = accounting
                .expect("classifier: AccountingEpochUnavailable implies an accounting row exists");
            let has_snapshot = snapshot_id.is_some();
            let reason = if has_snapshot {
                "accounting row has no recorded source snapshot; completeness cannot be traced"
                    .to_string()
            } else {
                "no durable snapshot exists yet for this run; accounting completeness cannot be traced"
                    .to_string()
            };
            (
                truth_state.clone(),
                Some(row.accounting_epoch.clone()),
                None,
                None,
                Some(row.last_applied_inbox_id),
                None,
                truth_state,
                Some(reason),
                has_snapshot.then_some(row.fees_micros as f64 / SCALE),
                has_snapshot.then_some(row.cash_micros as f64 / SCALE),
            )
        }
        PortfolioProvenanceState::AccountingSnapshotMismatch => {
            let row = accounting
                .expect("classifier: AccountingSnapshotMismatch implies an accounting row exists");
            (
                truth_state.clone(),
                Some(row.accounting_epoch.clone()),
                row.accounting_epoch_reason.clone(),
                row.source_snapshot_id.map(|id| id.to_string()),
                Some(row.last_applied_inbox_id),
                None,
                truth_state,
                Some(
                    "accounting row's source snapshot does not match the currently selected durable snapshot"
                        .to_string(),
                ),
                Some(row.fees_micros as f64 / SCALE),
                Some(row.cash_micros as f64 / SCALE),
            )
        }
        PortfolioProvenanceState::FillHistoryIncomplete => {
            let row = accounting
                .expect("classifier: FillHistoryIncomplete implies an accounting row exists");
            (
                truth_state.clone(),
                Some(row.accounting_epoch.clone()),
                row.accounting_epoch_reason.clone(),
                row.source_snapshot_id.map(|id| id.to_string()),
                Some(row.last_applied_inbox_id),
                None,
                truth_state,
                row.accounting_epoch_reason.clone(),
                Some(row.fees_micros as f64 / SCALE),
                Some(row.cash_micros as f64 / SCALE),
            )
        }
        PortfolioProvenanceState::Active => {
            let row = accounting.expect("classifier: Active implies an accounting row exists");
            (
                truth_state.clone(),
                Some(row.accounting_epoch.clone()),
                None,
                row.source_snapshot_id.map(|id| id.to_string()),
                Some(row.last_applied_inbox_id),
                Some(row.realized_pnl_micros as f64 / SCALE),
                truth_state,
                None,
                Some(row.fees_micros as f64 / SCALE),
                Some(row.cash_micros as f64 / SCALE),
            )
        }
        PortfolioProvenanceState::QueryFailed | PortfolioProvenanceState::UnsupportedSource => (
            truth_state.clone(),
            None,
            None,
            None,
            None,
            None,
            truth_state,
            Some(QUERY_FAILED_MESSAGE.to_string()),
            None,
            None,
        ),
    }
}

fn unavailable_summary(truth_state: &str, detail: &str) -> PortfolioDurableSummaryResponse {
    PortfolioDurableSummaryResponse {
        truth_state: truth_state.to_string(),
        snapshot_truth_state: truth_state.to_string(),
        snapshot_id: None,
        captured_at_utc: None,
        source: None,
        deployment_mode: None,
        account_equity: None,
        cash: None,
        currency: None,
        run_id: None,
        operation_id: None,
        accounting_truth_state: truth_state.to_string(),
        accounting_epoch: None,
        accounting_epoch_reason: None,
        accounting_source_snapshot_id: None,
        last_applied_inbox_id: None,
        realized_pnl: None,
        realized_pnl_truth_state: truth_state.to_string(),
        realized_pnl_unavailable_reason: Some(detail.to_string()),
        fees: None,
        cumulative_cash_movement: None,
        unrealized_pnl: None,
        unrealized_pnl_truth_state: truth_state.to_string(),
        unrealized_pnl_unavailable_reason: Some(detail.to_string()),
        daily_pnl: None,
        daily_pnl_truth_state: truth_state.to_string(),
        daily_pnl_unavailable_reason: Some(detail.to_string()),
        blockers: vec![detail.to_string()],
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/durable-positions
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_durable_positions(
    State(st): State<Arc<AppState>>,
    Query(params): Query<RunIdParam>,
) -> impl IntoResponse {
    let explicit_run_id = match parse_explicit_run_id(params.run_id.as_deref()) {
        Ok(id) => id,
        Err(detail) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "detail": detail})),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(PortfolioDurablePositionsResponse {
                truth_state: "db_unavailable".to_string(),
                snapshot_id: None,
                captured_at_utc: None,
                run_id: None,
                positions: vec![],
            }),
        )
            .into_response();
    };

    // Run resolution is now the same fail-closed authority every other
    // durable route uses: an explicit-run query failure is distinct from
    // not_found, and the snapshot lookup below is scoped to exactly this
    // run (see docs/specs/durable_paper_portfolio_and_pnl_01e_read_only_api.md).
    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            return (
                StatusCode::OK,
                Json(PortfolioDurablePositionsResponse {
                    truth_state: "query_failed".to_string(),
                    snapshot_id: None,
                    captured_at_utc: None,
                    run_id: None,
                    positions: vec![],
                }),
            )
                .into_response();
        }
        RunResolution::NotFound => {
            return (
                StatusCode::OK,
                Json(PortfolioDurablePositionsResponse {
                    truth_state: "not_found".to_string(),
                    snapshot_id: None,
                    captured_at_utc: None,
                    run_id: explicit_run_id.map(|id| id.to_string()),
                    positions: vec![],
                }),
            )
                .into_response();
        }
    };
    let run_id = Some(run.run_id);

    if run.mode != PAPER_MODE {
        return (
            StatusCode::OK,
            Json(PortfolioDurablePositionsResponse {
                truth_state: "unsupported_source".to_string(),
                snapshot_id: None,
                captured_at_utc: None,
                run_id: run_id.map(|id| id.to_string()),
                positions: vec![],
            }),
        )
            .into_response();
    }

    let snapshot = match mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run.run_id,
    )
    .await
    {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run.run_id, "durable_positions_snapshot_query_failed");
            return (
                StatusCode::OK,
                Json(PortfolioDurablePositionsResponse {
                    truth_state: "query_failed".to_string(),
                    snapshot_id: None,
                    captured_at_utc: None,
                    run_id: run_id.map(|id| id.to_string()),
                    positions: vec![],
                }),
            )
                .into_response();
        }
    };

    let Some(snapshot) = snapshot else {
        return (
            StatusCode::OK,
            Json(PortfolioDurablePositionsResponse {
                truth_state: "snapshot_unavailable".to_string(),
                snapshot_id: None,
                captured_at_utc: None,
                run_id: run_id.map(|id| id.to_string()),
                positions: vec![],
            }),
        )
            .into_response();
    };

    let age_secs = (Utc::now() - snapshot.snapshot.captured_at_utc).num_seconds();
    let truth_state = if age_secs > DURABLE_SNAPSHOT_STALE_SECS {
        "snapshot_stale"
    } else {
        "active"
    };

    let positions = snapshot
        .positions
        .iter()
        .map(|p| PortfolioDurablePositionRow {
            symbol: p.symbol.clone(),
            qty_signed: p.qty_signed,
            avg_entry_price: p.avg_entry_price_micros as f64 / mqk_portfolio::MICROS_SCALE as f64,
            provenance: p.provenance.clone(),
        })
        .collect();

    (
        StatusCode::OK,
        Json(PortfolioDurablePositionsResponse {
            truth_state: truth_state.to_string(),
            snapshot_id: Some(snapshot.snapshot.snapshot_id.to_string()),
            captured_at_utc: Some(snapshot.snapshot.captured_at_utc.to_rfc3339()),
            run_id: run_id.map(|id| id.to_string()),
            positions,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/durable-snapshots?limit=20
// ---------------------------------------------------------------------------

const DEFAULT_SNAPSHOTS_LIMIT: i64 = 20;
const MAX_SNAPSHOTS_LIMIT: i64 = 200;

pub(crate) async fn portfolio_durable_snapshots(
    State(st): State<Arc<AppState>>,
    Query(params): Query<SnapshotsListParams>,
) -> impl IntoResponse {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_SNAPSHOTS_LIMIT)
        .clamp(1, MAX_SNAPSHOTS_LIMIT);

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(PortfolioDurableSnapshotsResponse {
                truth_state: "db_unavailable".to_string(),
                snapshots: vec![],
            }),
        )
            .into_response();
    };

    let rows = match mqk_db::fetch_recent_paper_portfolio_snapshots(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        limit,
    )
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return (
                StatusCode::OK,
                Json(PortfolioDurableSnapshotsResponse {
                    truth_state: "query_failed".to_string(),
                    snapshots: vec![],
                }),
            )
                .into_response();
        }
    };

    let snapshots = rows
        .into_iter()
        .map(|r| PortfolioDurableSnapshotRow {
            snapshot_id: r.snapshot_id.to_string(),
            captured_at_utc: r.captured_at_utc.to_rfc3339(),
            deployment_mode: r.deployment_mode,
            source: r.source,
            equity: r.equity_micros as f64 / mqk_portfolio::MICROS_SCALE as f64,
            cash: r.cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64,
            currency: r.currency,
            truth_state: r.truth_state,
            run_id: r.run_id.map(|id| id.to_string()),
            operation_id: r.operation_id.map(|id| id.to_string()),
        })
        .collect();

    (
        StatusCode::OK,
        Json(PortfolioDurableSnapshotsResponse {
            truth_state: "active".to_string(),
            snapshots,
        }),
    )
        .into_response()
}
