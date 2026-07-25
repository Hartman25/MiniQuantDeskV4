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

async fn resolve_run(
    db: &sqlx::PgPool,
    explicit_run_id: Option<Uuid>,
) -> Result<Option<mqk_db::RunRow>, anyhow::Error> {
    if let Some(run_id) = explicit_run_id {
        Ok(mqk_db::fetch_run(db, run_id).await.ok())
    } else {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, PAPER_MODE).await
    }
}

fn parse_explicit_run_id(raw: Option<&str>) -> Result<Option<Uuid>, String> {
    match raw {
        Some(s) => s
            .parse::<Uuid>()
            .map(Some)
            .map_err(|_| format!("run_id is not a valid UUID: {s}")),
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
            Json(unavailable_summary("db_unavailable", "no_db_pool_configured")),
        )
            .into_response();
    };

    let run = match resolve_run(db, explicit_run_id).await {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", &format!("{err:#}"))),
            )
                .into_response();
        }
    };

    let Some(run) = run else {
        return (StatusCode::OK, Json(unavailable_summary("not_found", "no run resolved")))
            .into_response();
    };

    let snapshot = match mqk_db::fetch_latest_paper_portfolio_snapshot(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    )
    .await
    {
        Ok(s) => s,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", &format!("{err:#}"))),
            )
                .into_response();
        }
    };

    let accounting = match mqk_db::fetch_paper_portfolio_accounting_state(db, run.run_id).await {
        Ok(a) => a,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(unavailable_summary("query_failed", &format!("{err:#}"))),
            )
                .into_response();
        }
    };

    let mut blockers = Vec::new();

    let Some(snapshot) = snapshot else {
        // No durable snapshot yet -- accounting/pnl fields stay unavailable
        // too, since unrealized P&L needs the snapshot's positions.
        let (accounting_truth_state, accounting_epoch, accounting_epoch_reason, last_applied_inbox_id, realized_pnl, realized_pnl_truth_state, realized_pnl_reason, fees, cash_movement) =
            accounting_fields(accounting.as_ref());
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
                last_applied_inbox_id,
                realized_pnl,
                realized_pnl_truth_state,
                realized_pnl_unavailable_reason: realized_pnl_reason,
                fees,
                cumulative_cash_movement: cash_movement,
                unrealized_pnl: None,
                unrealized_pnl_truth_state: "snapshot_unavailable".to_string(),
                unrealized_pnl_unavailable_reason: Some("no durable snapshot exists yet".to_string()),
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

    let account_equity = snapshot.snapshot.equity_micros as f64 / mqk_portfolio::MICROS_SCALE as f64;
    let cash = snapshot.snapshot.cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64;

    let (accounting_truth_state, accounting_epoch, accounting_epoch_reason, last_applied_inbox_id, realized_pnl, realized_pnl_truth_state, realized_pnl_reason, fees, cash_movement) =
        accounting_fields(accounting.as_ref());

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
    let pnl_by_symbol = compute_broker_positions_pnl(&st, &broker_positions, DEFAULT_TIMEFRAME).await;
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

#[allow(clippy::type_complexity)]
fn accounting_fields(
    accounting: Option<&mqk_db::PaperPortfolioAccountingStateRecord>,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<f64>,
    String,
    Option<String>,
    Option<f64>,
    Option<f64>,
) {
    match accounting {
        None => (
            "not_found".to_string(),
            None,
            None,
            None,
            None,
            "not_found".to_string(),
            Some("no accounting rows exist yet for this run".to_string()),
            None,
            None,
        ),
        Some(row) if row.accounting_epoch == "incomplete" => (
            "fill_history_incomplete".to_string(),
            Some(row.accounting_epoch.clone()),
            row.accounting_epoch_reason.clone(),
            Some(row.last_applied_inbox_id),
            None,
            "fill_history_incomplete".to_string(),
            row.accounting_epoch_reason.clone(),
            Some(row.fees_micros as f64 / mqk_portfolio::MICROS_SCALE as f64),
            Some(row.cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64),
        ),
        Some(row) => (
            "active".to_string(),
            Some(row.accounting_epoch.clone()),
            None,
            Some(row.last_applied_inbox_id),
            Some(row.realized_pnl_micros as f64 / mqk_portfolio::MICROS_SCALE as f64),
            "active".to_string(),
            None,
            Some(row.fees_micros as f64 / mqk_portfolio::MICROS_SCALE as f64),
            Some(row.cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64),
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

    // run_id is resolved only to echo it on the response and to keep this
    // route's query-param contract identical to the other durable routes;
    // the snapshot lookup itself is not run-scoped (see
    // docs/specs/durable_paper_portfolio_and_pnl_01e_read_only_api.md).
    let run_id = match resolve_run(db, explicit_run_id).await {
        Ok(r) => r.map(|r| r.run_id),
        Err(_) => None,
    };

    let snapshot = match mqk_db::fetch_latest_paper_portfolio_snapshot(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    )
    .await
    {
        Ok(s) => s,
        Err(_) => {
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

