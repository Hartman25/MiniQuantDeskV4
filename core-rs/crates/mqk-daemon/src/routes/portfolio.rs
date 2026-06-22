//! Portfolio and risk route handlers.
//!
//! Contains: portfolio_summary, portfolio_positions, portfolio_open_orders,
//! portfolio_fills, risk_summary, risk_denials, portfolio_live_weights.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::api_types::{
    PortfolioFillRow, PortfolioFillsResponse, PortfolioLiveWeightRow,
    PortfolioLiveWeightsResponse, PortfolioOpenOrderRow, PortfolioOpenOrdersResponse,
    PortfolioPositionRow, PortfolioPositionsResponse, PortfolioSummaryResponse, RiskDenialRow,
    RiskDenialsResponse, RiskSummaryResponse,
};
use crate::state::AppState;

use super::helpers::{exposure_breakdown, parse_decimal, position_market_value};

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/summary
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();

    let summary = if let Some(snapshot) = snap {
        let account_equity = parse_decimal(&snapshot.account.equity);
        let cash = parse_decimal(&snapshot.account.cash);
        let (long_market_value, short_market_value, _, _) = exposure_breakdown(&snapshot.positions);

        PortfolioSummaryResponse {
            has_snapshot: true,
            truth_state: "active".to_string(),
            account_equity: Some(account_equity),
            cash: Some(cash),
            long_market_value: Some(long_market_value),
            short_market_value: Some(short_market_value),
            daily_pnl: None,
            buying_power: Some(cash),
        }
    } else {
        PortfolioSummaryResponse {
            has_snapshot: false,
            truth_state: "no_snapshot".to_string(),
            account_equity: None,
            cash: None,
            long_market_value: None,
            short_market_value: None,
            daily_pnl: None,
            buying_power: None,
        }
    };

    (StatusCode::OK, Json(summary)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/positions
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_positions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    // PORT-05: session_boundary is always "in_memory_only" — broker_snapshot is
    // held in-memory and lost on daemon restart regardless of broker kind.
    let session_boundary = "in_memory_only".to_string();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioPositionsResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
                snapshot_source: None,
                session_boundary,
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .positions
                .iter()
                .map(|p| {
                    let qty = p.qty.parse::<i64>().unwrap_or(0);
                    let avg_price = parse_decimal(&p.avg_price);
                    PortfolioPositionRow {
                        symbol: p.symbol.clone(),
                        strategy_id: None,
                        qty,
                        avg_price,
                        mark_price: None,
                        unrealized_pnl: None,
                        realized_pnl_today: None,
                        broker_qty: qty,
                        drift: None,
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioPositionsResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                    snapshot_source: Some(st.broker_snapshot_source.as_str().to_string()),
                    session_boundary,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/orders/open
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_open_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let session_boundary = "in_memory_only".to_string();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioOpenOrdersResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
                snapshot_source: None,
                session_boundary,
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .orders
                .iter()
                .map(|o| {
                    let requested_qty = o.qty.parse::<i64>().unwrap_or(0);
                    PortfolioOpenOrderRow {
                        internal_order_id: o.client_order_id.clone(),
                        symbol: o.symbol.clone(),
                        strategy_id: None,
                        side: o.side.clone(),
                        status: o.status.clone(),
                        requested_qty,
                        filled_qty: None,
                        entered_at: o.created_at_utc.to_rfc3339(),
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioOpenOrdersResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                    snapshot_source: Some(st.broker_snapshot_source.as_str().to_string()),
                    session_boundary,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/fills
// ---------------------------------------------------------------------------

pub(crate) async fn portfolio_fills(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let session_boundary = "in_memory_only".to_string();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioFillsResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
                snapshot_source: None,
                session_boundary,
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .fills
                .iter()
                .map(|f| {
                    let qty = f.qty.parse::<i64>().unwrap_or(0);
                    let price = parse_decimal(&f.price);
                    PortfolioFillRow {
                        fill_id: f.broker_fill_id.clone(),
                        internal_order_id: f.client_order_id.clone(),
                        symbol: f.symbol.clone(),
                        strategy_id: None,
                        side: f.side.clone(),
                        qty,
                        price,
                        broker_exec_id: f.broker_fill_id.clone(),
                        applied: true,
                        at: f.ts_utc.to_rfc3339(),
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioFillsResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                    snapshot_source: Some(st.broker_snapshot_source.as_str().to_string()),
                    session_boundary,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/risk/summary
// ---------------------------------------------------------------------------

pub(crate) async fn risk_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let durable_risk = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db).await.ok().flatten()
    } else {
        None
    };
    let risk_blocked = durable_risk.as_ref().is_some_and(|state| state.blocked);

    // RISK-ENGINE-HALTED-VISIBILITY-01: read-only overlay of the live risk
    // gate's sticky `RiskState.halted` flag. Distinct from `risk_blocked`
    // above, which is derived from the transient `sys_risk_block_state` DB
    // row and is reset every orchestrator tick.
    let risk_engine_halted = match st.current_execution_snapshot().await {
        Some(exec_snapshot) => match exec_snapshot.risk_engine_sticky_halt {
            mqk_execution::RiskEngineHaltStatus::Known { halted } => Some(halted),
            mqk_execution::RiskEngineHaltStatus::Unavailable => None,
        },
        None => None,
    };
    // RiskState does not currently track a reason alongside `halted`.
    let risk_engine_halt_reason_code: Option<String> = None;

    let summary = if let Some(snapshot) = snap {
        let (_, _, gross_exposure, max_abs_position) = exposure_breakdown(&snapshot.positions);
        let net_exposure = snapshot
            .positions
            .iter()
            .map(position_market_value)
            .sum::<f64>();
        let concentration_pct = if gross_exposure > 0.0 {
            (max_abs_position / gross_exposure) * 100.0
        } else {
            0.0
        };

        RiskSummaryResponse {
            has_snapshot: true,
            gross_exposure: Some(gross_exposure),
            net_exposure: Some(net_exposure),
            concentration_pct: Some(concentration_pct),
            daily_pnl: None,
            drawdown_pct: None,
            loss_limit_utilization_pct: None,
            kill_switch_active: risk_blocked,
            active_breaches: usize::from(risk_blocked),
            risk_engine_halted,
            risk_engine_halt_reason_code,
        }
    } else {
        RiskSummaryResponse {
            has_snapshot: false,
            gross_exposure: None,
            net_exposure: None,
            concentration_pct: None,
            daily_pnl: None,
            drawdown_pct: None,
            loss_limit_utilization_pct: None,
            kill_switch_active: risk_blocked,
            active_breaches: usize::from(risk_blocked),
            risk_engine_halted,
            risk_engine_halt_reason_code,
        }
    };

    (StatusCode::OK, Json(summary)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/risk/denials
// ---------------------------------------------------------------------------

pub(crate) async fn risk_denials(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.execution_snapshot.read().await.clone();

    let db_to_api = |r: &mqk_db::RiskDenialEventRow| RiskDenialRow {
        id: r.id.clone(),
        at: r.denied_at_utc.to_rfc3339(),
        strategy_id: None,
        symbol: r.symbol.clone().unwrap_or_default(),
        rule: r.rule.clone(),
        message: r.message.clone(),
        severity: r.severity.clone(),
    };

    if let Some(pool) = st.db.as_ref() {
        let db_rows = match mqk_db::load_recent_risk_denial_events(pool, 100).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!("load_recent_risk_denial_events failed: {err}");
                return (
                    StatusCode::OK,
                    Json(RiskDenialsResponse {
                        truth_state: "no_snapshot".to_string(),
                        snapshot_at_utc: None,
                        denials: vec![],
                    }),
                )
                    .into_response();
            }
        };

        return if let Some(snapshot) = snap {
            let denials = db_rows.iter().map(db_to_api).collect();
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "active".to_string(),
                    snapshot_at_utc: Some(snapshot.snapshot_at_utc.to_rfc3339()),
                    denials,
                }),
            )
                .into_response()
        } else if db_rows.is_empty() {
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "no_snapshot".to_string(),
                    snapshot_at_utc: None,
                    denials: vec![],
                }),
            )
                .into_response()
        } else {
            let denials = db_rows.iter().map(db_to_api).collect();
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "durable_history".to_string(),
                    snapshot_at_utc: None,
                    denials,
                }),
            )
                .into_response()
        };
    }

    let Some(snapshot) = snap else {
        return (
            StatusCode::OK,
            Json(RiskDenialsResponse {
                truth_state: "no_snapshot".to_string(),
                snapshot_at_utc: None,
                denials: vec![],
            }),
        )
            .into_response();
    };

    let denials = snapshot
        .recent_risk_denials
        .iter()
        .map(|r| RiskDenialRow {
            id: r.id.clone(),
            at: r.denied_at_utc.to_rfc3339(),
            strategy_id: None,
            symbol: r.symbol.clone().unwrap_or_default(),
            rule: r.rule.clone(),
            message: r.message.clone(),
            severity: r.severity.clone(),
        })
        .collect();
    (
        StatusCode::OK,
        Json(RiskDenialsResponse {
            truth_state: "active_session_only".to_string(),
            snapshot_at_utc: Some(snapshot.snapshot_at_utc.to_rfc3339()),
            denials,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/live-weights (PORTFOLIO-LIVE-WEIGHTS-01)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub(crate) struct LiveWeightsParams {
    pub timeframe: Option<String>,
}

const DEFAULT_LIVE_WEIGHTS_TIMEFRAME: &str = "1D";

fn clamp_i128_to_i64(x: i128) -> i64 {
    if x > i64::MAX as i128 {
        i64::MAX
    } else if x < i64::MIN as i128 {
        i64::MIN
    } else {
        x as i64
    }
}

fn live_weight_row_from_pure(row: mqk_portfolio::PositionWeightRow) -> PortfolioLiveWeightRow {
    PortfolioLiveWeightRow {
        symbol: row.symbol,
        signed_qty: row.signed_qty,
        mark_price_micros: row.mark_price_micros,
        mark_ts_utc: row.mark_ts_utc,
        mark_source: row.mark_source,
        market_value_micros: row.market_value_micros.map(clamp_i128_to_i64),
        absolute_notional_micros: row.absolute_notional_micros.map(clamp_i128_to_i64),
        weight_bps: row.weight_bps,
        missing_mark: row.missing_mark,
    }
}

/// Truthful, read-only live position valuation seam.
///
/// Reads positions/cash from the in-memory execution snapshot (the same
/// source `portfolio_summary`'s sibling routes do not use — that surface is
/// broker-account-derived; this one is the runtime's own ledger-derived
/// snapshot) and looks up a mark for each non-flat symbol from the latest
/// *completed* `md_bars` row at `timeframe`. No broker call, no provider
/// call, no live quote. Missing bars produce `missing_mark = true`, never a
/// fabricated price.
///
/// This route performs no writes of any kind (no outbox, no orders, no risk
/// state) and does not enforce any limit — it is a valuation seam only.
pub(crate) async fn portfolio_live_weights(
    Query(params): Query<LiveWeightsParams>,
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let timeframe = params
        .timeframe
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LIVE_WEIGHTS_TIMEFRAME.to_string());

    let snap = st.execution_snapshot.read().await.clone();
    let Some(snapshot) = snap else {
        return (
            StatusCode::OK,
            Json(PortfolioLiveWeightsResponse {
                truth_state: "no_snapshot".to_string(),
                timeframe,
                cash_micros: 0,
                nav_micros: None,
                gross_exposure_micros: None,
                positions: vec![],
                missing_mark_symbols: vec![],
            }),
        )
            .into_response();
    };

    let cash_micros = snapshot.portfolio.cash_micros;
    let inputs: Vec<mqk_portfolio::PositionWeightInput> = snapshot
        .portfolio
        .positions
        .iter()
        .map(|p| mqk_portfolio::PositionWeightInput {
            symbol: p.symbol.clone(),
            signed_qty: p.net_qty,
        })
        .collect();

    let non_flat_symbols: Vec<&str> = inputs
        .iter()
        .filter(|p| p.signed_qty != 0)
        .map(|p| p.symbol.as_str())
        .collect();

    let db_available = st.db.is_some();
    let mut marks: BTreeMap<String, mqk_portfolio::PositionMark> = BTreeMap::new();
    if let Some(pool) = st.db.as_ref() {
        for symbol in non_flat_symbols {
            match mqk_db::fetch_recent_completed_bars_for_strategy(pool, symbol, &timeframe, 1)
                .await
            {
                Ok(bars) => {
                    if let Some(bar) = bars.last() {
                        marks.insert(
                            symbol.to_string(),
                            mqk_portfolio::PositionMark {
                                symbol: symbol.to_string(),
                                mark_price_micros: bar.close_micros,
                                mark_ts_utc: Some(bar.end_ts),
                                source: format!("md_bars:{timeframe}:close"),
                            },
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "portfolio_live_weights: md_bars lookup failed for {symbol}: {err}"
                    );
                }
            }
        }
    }

    let result = mqk_portfolio::compute_portfolio_weights(cash_micros, &inputs, &marks);

    // Distinguish "DB absent so marks were never attempted" from "DB was
    // consulted but specific symbols have no completed bar" — both collapse
    // to the pure helper's "missing_marks", but they are different truths
    // for an operator (CLAUDE.md operator-truth discipline: unavailable vs.
    // empty vs. present).
    let truth_state = if !db_available && result.truth_state == "missing_marks" {
        "db_unavailable".to_string()
    } else {
        result.truth_state
    };

    (
        StatusCode::OK,
        Json(PortfolioLiveWeightsResponse {
            truth_state,
            timeframe,
            cash_micros: result.cash_micros,
            nav_micros: result.nav_micros.map(clamp_i128_to_i64),
            gross_exposure_micros: result.gross_exposure_micros.map(clamp_i128_to_i64),
            positions: result
                .positions
                .into_iter()
                .map(live_weight_row_from_pure)
                .collect(),
            missing_mark_symbols: result.missing_mark_symbols,
        }),
    )
        .into_response()
}
