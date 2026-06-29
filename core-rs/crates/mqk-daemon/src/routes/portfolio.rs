//! Portfolio and risk route handlers.
//!
//! Contains: portfolio_summary, portfolio_positions, portfolio_open_orders,
//! portfolio_fills, risk_summary, risk_denials, portfolio_live_weights,
//! portfolio_economics_status.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::api_types::{
    PortfolioEconomicsStatusExposureRow, PortfolioEconomicsStatusPositionRow,
    PortfolioEconomicsStatusResponse, PortfolioFillRow, PortfolioFillsResponse,
    PortfolioLiveWeightRow, PortfolioLiveWeightsResponse, PortfolioOpenOrderRow,
    PortfolioOpenOrdersResponse, PortfolioPositionRow, PortfolioPositionsResponse,
    PortfolioSummaryResponse, RiskDenialRow, RiskDenialsResponse, RiskSummaryResponse,
};
use crate::state::{
    bridge_instrument_registry_v2_to_economics, AppState, InstrumentEconomicsBridgeResult,
};

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

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/economics/status (ASSET-CORE-04D)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub(crate) struct PortfolioEconomicsStatusParams {
    pub timeframe: Option<String>,
    pub symbol: Option<String>,
    pub limit: Option<usize>,
}

const DEFAULT_PORTFOLIO_ECONOMICS_TIMEFRAME: &str = "1D";

/// Hardcoded account currency for the ASSET-CORE-04D economics status route.
///
/// `mqk_runtime::observability::PortfolioSnapshot` carries no account-currency
/// field -- every other live portfolio/accounting path in this crate
/// (`accounting.rs`, `compute_portfolio_weights`,
/// `/api/v1/portfolio/live-weights`) already implicitly assumes USD. This
/// route makes that assumption explicit rather than inventing a new concept;
/// it never performs FX conversion, and any resolved position whose registry
/// quote currency differs from this constant fails closed via the existing
/// ASSET-CORE-04A/04C `CurrencyConversionUnsupported` check.
const PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY: &str = "USD";

fn portfolio_economics_status_message(truth_state: &str) -> String {
    match truth_state {
        "no_snapshot" => "No in-memory execution snapshot is present.".to_string(),
        "registry_unavailable" => {
            "No instrument registry file exists at the configured path.".to_string()
        }
        "registry_invalid" => "The instrument registry failed to load or validate.".to_string(),
        "no_positions" => "Execution snapshot has zero positions; NAV equals cash.".to_string(),
        "active" => {
            "Every held position is valued or flat; NAV/exposure/weights are computed.".to_string()
        }
        "position_value_unavailable" => "At least one position could not be valued (registry \
            bridge failure or unresolved symbol)."
            .to_string(),
        "missing_marks" => "DB is present but one or more non-flat positions have no completed \
            md_bars row."
            .to_string(),
        "db_unavailable" => {
            "No DB pool is configured; no mark could be looked up for any non-flat position."
                .to_string()
        }
        "currency_conversion_unsupported" => {
            "A resolved position's quote currency differs from the account currency.".to_string()
        }
        "nav_unavailable" => {
            "Every position is valued or flat but nav_micros is not positive.".to_string()
        }
        "aggregation_unavailable" => "Summing NAV or exposure overflowed i128.".to_string(),
        _ => "Unrecognized truth state.".to_string(),
    }
}

/// Build the empty-positions failure/short-circuit response shared by the
/// no-snapshot, registry-unavailable, and registry-invalid truth states.
#[allow(clippy::too_many_arguments)]
fn portfolio_economics_status_envelope(
    truth_state: &str,
    reason_code: &str,
    registry_path: String,
    registry_v2_valid: bool,
    has_execution_snapshot: bool,
    timeframe: String,
    symbol_filter: Option<String>,
) -> PortfolioEconomicsStatusResponse {
    PortfolioEconomicsStatusResponse {
        truth_state: truth_state.to_string(),
        reason_code: reason_code.to_string(),
        message: portfolio_economics_status_message(truth_state),
        model_only: true,
        trading_uses_portfolio_economics: false,
        runtime_uses_portfolio_economics: false,
        risk_uses_portfolio_economics: false,
        order_path_uses_portfolio_economics: false,
        timeframe,
        symbol_filter,
        registry_path,
        registry_v2_valid,
        has_execution_snapshot,
        position_count: 0,
        valued_position_count: 0,
        failed_position_count: 0,
        missing_mark_count: 0,
        non_equity_position_count: 0,
        non_equity_enabled_count: 0,
        account_currency: PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY.to_string(),
        cash_micros: None,
        nav_micros: None,
        gross_exposure_micros: None,
        net_exposure_micros: None,
        positions: Vec::new(),
        asset_class_exposures: Vec::new(),
        currency_exposures: Vec::new(),
        blockers: Vec::new(),
    }
}

/// Per-position context the registry bridge / md_bars lookup gather *before*
/// economics valuation -- threaded through to the response row after
/// [`mqk_portfolio::aggregate_portfolio_economics`] runs, since the pure
/// aggregator's own output rows do not carry mark provenance.
struct PortfolioEconomicsPositionContext {
    mark_price_micros: Option<i64>,
    mark_ts_utc: Option<i64>,
    mark_source: Option<String>,
    model_only: bool,
    /// `true` only when the symbol resolved to a registry-v2 instrument that
    /// successfully bridged to economics (the position went through the real
    /// [`mqk_portfolio::value_position_economics`]). `false` for every
    /// manufactured fail-closed value (symbol not found, registry bridge
    /// failed, qty-scale overflow) -- in that case the response row reports
    /// `quote_currency` as unknown regardless of the internal
    /// aggregator-routing placeholder set on the underlying
    /// `PositionEconomicsValue` (see [`resolve_position_economics`] docs).
    resolved: bool,
}

/// Resolve one position's symbol against the registry-v2 bridge and build
/// its [`mqk_portfolio::PositionEconomicsValue`] plus response context.
///
/// Never fabricates an equity/USD default for an unresolved symbol -- a
/// symbol absent from the registry, or present but failing to bridge, is
/// valued as an explicit fail-closed
/// [`mqk_portfolio::InstrumentEconomicsTruthState::UnsupportedInstrument`]
/// with the real reason carried through verbatim.
///
/// `quote_currency` on a manufactured (unresolved/unbridged) value is set to
/// `account_currency` -- not a claim that the instrument's real currency is
/// known, but a routing detail so ASSET-CORE-04C's aggregator classifies the
/// position as `position_value_unavailable` (this function's real, specific
/// reason survives) rather than `currency_conversion_unsupported` (a generic,
/// misleading reason this function never asserts). `notional_micros` and
/// `absolute_notional_micros` stay `None` either way -- no value is ever
/// fabricated regardless of which bucket the position lands in.
fn resolve_position_economics(
    symbol: &str,
    net_qty: i64,
    bridge_by_symbol: &BTreeMap<String, &InstrumentEconomicsBridgeResult>,
    mark: Option<(i64, i64, String)>,
    account_currency: &str,
) -> (
    mqk_portfolio::PositionEconomicsValue,
    PortfolioEconomicsPositionContext,
) {
    let mark_price_micros = mark.as_ref().map(|(px, _, _)| *px);
    let mark_ts_utc = mark.as_ref().map(|(_, ts, _)| *ts);
    let mark_source = mark.as_ref().map(|(_, _, src)| src.clone());

    let Some(qty_micros) = (net_qty as i128)
        .checked_mul(mqk_portfolio::MICROS_SCALE as i128)
        .and_then(|v| i64::try_from(v).ok())
    else {
        let value = mqk_portfolio::PositionEconomicsValue {
            instrument_id: String::new(),
            symbol: symbol.to_string(),
            asset_class: String::new(),
            quote_currency: account_currency.to_string(),
            account_currency: account_currency.to_string(),
            signed_qty_micros: 0,
            mark_price_micros,
            contract_multiplier_micros: 0,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: mqk_portfolio::InstrumentEconomicsTruthState::Overflow,
            reason_code: "qty_scale_overflowed_i64".to_string(),
        };
        return (
            value,
            PortfolioEconomicsPositionContext {
                mark_price_micros,
                mark_ts_utc,
                mark_source,
                model_only: false,
                resolved: false,
            },
        );
    };

    match bridge_by_symbol.get(symbol) {
        Some(bridge) if bridge.economics.is_some() => {
            let instrument = bridge.economics.clone().expect("checked is_some above");
            let model_only = bridge.model_only;
            let value =
                mqk_portfolio::value_position_economics(mqk_portfolio::PositionEconomicsInput {
                    instrument,
                    signed_qty_micros: qty_micros,
                    mark_price_micros,
                    account_currency: account_currency.to_string(),
                });
            (
                value,
                PortfolioEconomicsPositionContext {
                    mark_price_micros,
                    mark_ts_utc,
                    mark_source,
                    model_only,
                    resolved: true,
                },
            )
        }
        Some(bridge) => {
            // Registry row exists but failed to bridge to economics (e.g. a
            // non-equity asset class with an invalid/missing contract). The
            // real registry-side reason is carried through verbatim.
            let value = mqk_portfolio::PositionEconomicsValue {
                instrument_id: bridge.instrument_id.clone(),
                symbol: symbol.to_string(),
                asset_class: bridge.asset_class.clone(),
                quote_currency: account_currency.to_string(),
                account_currency: account_currency.to_string(),
                signed_qty_micros: qty_micros,
                mark_price_micros,
                contract_multiplier_micros: 0,
                notional_micros: None,
                absolute_notional_micros: None,
                truth_state: mqk_portfolio::InstrumentEconomicsTruthState::UnsupportedInstrument,
                reason_code: format!("registry_bridge_failed:{}", bridge.reason_code),
            };
            (
                value,
                PortfolioEconomicsPositionContext {
                    mark_price_micros,
                    mark_ts_utc,
                    mark_source,
                    model_only: bridge.model_only,
                    resolved: false,
                },
            )
        }
        None => {
            let value = mqk_portfolio::PositionEconomicsValue {
                instrument_id: String::new(),
                symbol: symbol.to_string(),
                asset_class: String::new(),
                quote_currency: account_currency.to_string(),
                account_currency: account_currency.to_string(),
                signed_qty_micros: qty_micros,
                mark_price_micros,
                contract_multiplier_micros: 0,
                notional_micros: None,
                absolute_notional_micros: None,
                truth_state: mqk_portfolio::InstrumentEconomicsTruthState::UnsupportedInstrument,
                reason_code: "symbol_not_found_in_registry".to_string(),
            };
            (
                value,
                PortfolioEconomicsPositionContext {
                    mark_price_micros,
                    mark_ts_utc,
                    mark_source,
                    model_only: false,
                    resolved: false,
                },
            )
        }
    }
}

fn portfolio_economics_status_row(
    row: &mqk_portfolio::PortfolioEconomicsPositionRow,
    ctx: &PortfolioEconomicsPositionContext,
) -> PortfolioEconomicsStatusPositionRow {
    // `quote_currency` is reported as unknown (empty) for unresolved
    // positions, even though the underlying aggregator input carried
    // `account_currency` as an internal routing placeholder -- see
    // `resolve_position_economics` docs. This route never claims to know an
    // unresolved instrument's real currency.
    let quote_currency = if ctx.resolved {
        row.quote_currency.clone()
    } else {
        String::new()
    };
    PortfolioEconomicsStatusPositionRow {
        symbol: row.symbol.clone(),
        instrument_id: row.instrument_id.clone(),
        asset_class: row.asset_class.clone(),
        quote_currency,
        signed_qty: row.signed_qty_micros / mqk_portfolio::MICROS_SCALE,
        mark_price_micros: ctx.mark_price_micros,
        mark_ts_utc: ctx.mark_ts_utc,
        mark_source: ctx.mark_source.clone(),
        notional_micros: row.notional_micros.map(clamp_i128_to_i64),
        absolute_notional_micros: row.absolute_notional_micros.map(clamp_i128_to_i64),
        weight_bps: row.weight_bps,
        model_only: ctx.model_only,
        truth_state: row.truth_state.clone(),
        reason_code: row.reason_code.clone(),
    }
}

fn portfolio_economics_exposure_row(
    row: &mqk_portfolio::PortfolioEconomicsExposureRow,
) -> PortfolioEconomicsStatusExposureRow {
    PortfolioEconomicsStatusExposureRow {
        key: row.key.clone(),
        signed_notional_micros: clamp_i128_to_i64(row.signed_notional_micros),
        absolute_notional_micros: clamp_i128_to_i64(row.absolute_notional_micros),
        weight_bps: row.weight_bps,
    }
}

/// Map the pure ASSET-CORE-04C [`mqk_portfolio::PortfolioEconomicsTruthState`]
/// to this route's own truth-state vocabulary. `"missing_marks"` is a
/// route-level refinement of `PositionValueUnavailable`: it fires only when
/// *every* unavailable position's blocker is specifically a missing
/// completed mark (never when mixed with a registry/bridge failure), so the
/// operator is told the single most specific true cause whenever one exists.
fn map_portfolio_economics_truth_state(
    snapshot: &mqk_portfolio::PortfolioEconomicsSnapshot,
) -> (String, String) {
    use mqk_portfolio::PortfolioEconomicsTruthState::*;
    match snapshot.truth_state {
        Active => ("active".to_string(), snapshot.reason_code.clone()),
        NoPositions => ("no_positions".to_string(), snapshot.reason_code.clone()),
        CurrencyConversionUnsupported => (
            "currency_conversion_unsupported".to_string(),
            snapshot.reason_code.clone(),
        ),
        NavUnavailable => ("nav_unavailable".to_string(), snapshot.reason_code.clone()),
        Overflow => (
            "aggregation_unavailable".to_string(),
            snapshot.reason_code.clone(),
        ),
        PositionValueUnavailable => {
            let unavailable: Vec<&mqk_portfolio::PortfolioEconomicsPositionRow> = snapshot
                .positions
                .iter()
                .filter(|p| p.truth_state == "position_value_unavailable")
                .collect();
            let all_missing_mark = !unavailable.is_empty()
                && unavailable
                    .iter()
                    .all(|p| p.reason_code == "missing_mark_for_non_flat_position");
            if all_missing_mark {
                (
                    "missing_marks".to_string(),
                    "missing_mark_for_non_flat_position".to_string(),
                )
            } else {
                (
                    "position_value_unavailable".to_string(),
                    snapshot.reason_code.clone(),
                )
            }
        }
    }
}

/// Truthful, read-only composition of the ASSET-CORE-04B registry-v2 bridge
/// and the ASSET-CORE-04A/04C economics model against the live execution
/// snapshot -- ASSET-CORE-04D.
///
/// Sources, identical to `/api/v1/portfolio/live-weights`
/// (`PORTFOLIO-LIVE-WEIGHTS-01`) for positions/cash/marks: the in-memory
/// execution snapshot for positions and cash, and the latest *completed*
/// `md_bars` row at `timeframe` for each non-flat symbol's mark. The registry
/// is the same configured v1 path ASSET-CORE-01C/04B/05B already load,
/// converted to v2 and bridged via
/// `mqk_daemon::state::instrument_economics_bridge`. No broker/provider call,
/// no DB write, no order/risk/runtime path is touched, and the honesty flags
/// below are always as documented.
pub(crate) async fn portfolio_economics_status(
    Query(params): Query<PortfolioEconomicsStatusParams>,
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let timeframe = params
        .timeframe
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PORTFOLIO_ECONOMICS_TIMEFRAME.to_string());
    let symbol_filter = params
        .symbol
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty());
    let registry_path = st.instrument_registry_path.clone();

    let snap = st.execution_snapshot.read().await.clone();
    let Some(snapshot) = snap else {
        return (
            StatusCode::OK,
            Json(portfolio_economics_status_envelope(
                "no_snapshot",
                "no_execution_snapshot",
                registry_path,
                false,
                false,
                timeframe,
                symbol_filter,
            )),
        )
            .into_response();
    };

    let path = std::path::Path::new(&registry_path);
    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(err) => {
            let truth_state = if path.exists() {
                "registry_invalid"
            } else {
                "registry_unavailable"
            };
            return (
                StatusCode::OK,
                Json(portfolio_economics_status_envelope(
                    truth_state,
                    &err.to_string(),
                    registry_path,
                    false,
                    true,
                    timeframe,
                    symbol_filter,
                )),
            )
                .into_response();
        }
    };

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    if let Err(err) = mqk_md::instrument_registry_v2::validate_registry_v2(&v2) {
        return (
            StatusCode::OK,
            Json(portfolio_economics_status_envelope(
                "registry_invalid",
                &err.to_string(),
                registry_path,
                false,
                true,
                timeframe,
                symbol_filter,
            )),
        )
            .into_response();
    }

    let bridge_summary = bridge_instrument_registry_v2_to_economics(&v2);
    let bridge_by_symbol: BTreeMap<String, &InstrumentEconomicsBridgeResult> = bridge_summary
        .rows
        .iter()
        .map(|r| (r.symbol.clone(), r))
        .collect();

    let position_count = snapshot.portfolio.positions.len();
    let non_flat_symbols: Vec<&str> = snapshot
        .portfolio
        .positions
        .iter()
        .filter(|p| p.net_qty != 0)
        .map(|p| p.symbol.as_str())
        .collect();

    let db_available = st.db.is_some();
    let mut marks: BTreeMap<String, (i64, i64, String)> = BTreeMap::new();
    if let Some(pool) = st.db.as_ref() {
        for symbol in non_flat_symbols {
            match mqk_db::fetch_recent_completed_bars_for_strategy(pool, symbol, &timeframe, 1)
                .await
            {
                Ok(bars) => {
                    if let Some(bar) = bars.last() {
                        marks.insert(
                            symbol.to_string(),
                            (
                                bar.close_micros,
                                bar.end_ts,
                                format!("md_bars:{timeframe}:close"),
                            ),
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "portfolio_economics_status: md_bars lookup failed for {symbol}: {err}"
                    );
                }
            }
        }
    }

    let mut econ_positions: Vec<mqk_portfolio::PositionEconomicsValue> =
        Vec::with_capacity(position_count);
    let mut contexts: Vec<PortfolioEconomicsPositionContext> = Vec::with_capacity(position_count);
    let mut non_equity_position_count = 0usize;

    for pos in &snapshot.portfolio.positions {
        let mark = marks.get(pos.symbol.as_str()).cloned();
        let (value, ctx) = resolve_position_economics(
            &pos.symbol,
            pos.net_qty,
            &bridge_by_symbol,
            mark,
            PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY,
        );
        if !value.asset_class.is_empty() && value.asset_class != "equity" {
            non_equity_position_count += 1;
        }
        econ_positions.push(value);
        contexts.push(ctx);
    }

    let agg =
        mqk_portfolio::aggregate_portfolio_economics(mqk_portfolio::PortfolioEconomicsInput {
            cash_micros: snapshot.portfolio.cash_micros as i128,
            account_currency: PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY.to_string(),
            positions: econ_positions,
        });

    let (mut truth_state, mut reason_code) = map_portfolio_economics_truth_state(&agg);
    if !db_available && truth_state == "missing_marks" {
        truth_state = "db_unavailable".to_string();
        reason_code = "no_db_pool_configured".to_string();
    }

    let valued_position_count = agg
        .positions
        .iter()
        .filter(|p| p.truth_state == "active")
        .count();
    let failed_position_count = agg.positions.len() - valued_position_count;
    let missing_mark_count = agg
        .positions
        .iter()
        .filter(|p| p.reason_code == "missing_mark_for_non_flat_position")
        .count();
    let blockers: Vec<String> = agg
        .positions
        .iter()
        .filter(|p| p.truth_state != "active")
        .map(|p| format!("{}: {} ({})", p.symbol, p.truth_state, p.reason_code))
        .collect();

    let all_rows: Vec<PortfolioEconomicsStatusPositionRow> = agg
        .positions
        .iter()
        .zip(contexts.iter())
        .map(|(row, ctx)| portfolio_economics_status_row(row, ctx))
        .collect();

    let positions: Vec<PortfolioEconomicsStatusPositionRow> = all_rows
        .into_iter()
        .filter(|row| {
            symbol_filter
                .as_deref()
                .is_none_or(|s| row.symbol.to_ascii_uppercase() == s)
        })
        .take(params.limit.unwrap_or(usize::MAX))
        .collect();

    let asset_class_exposures: Vec<PortfolioEconomicsStatusExposureRow> = agg
        .asset_class_exposures
        .iter()
        .map(portfolio_economics_exposure_row)
        .collect();
    let currency_exposures: Vec<PortfolioEconomicsStatusExposureRow> = agg
        .currency_exposures
        .iter()
        .map(portfolio_economics_exposure_row)
        .collect();

    let message = portfolio_economics_status_message(&truth_state);

    (
        StatusCode::OK,
        Json(PortfolioEconomicsStatusResponse {
            truth_state,
            reason_code,
            message,
            model_only: true,
            trading_uses_portfolio_economics: false,
            runtime_uses_portfolio_economics: false,
            risk_uses_portfolio_economics: false,
            order_path_uses_portfolio_economics: false,
            timeframe,
            symbol_filter,
            registry_path,
            registry_v2_valid: true,
            has_execution_snapshot: true,
            position_count,
            valued_position_count,
            failed_position_count,
            missing_mark_count,
            non_equity_position_count,
            non_equity_enabled_count: bridge_summary.non_equity_enabled_count,
            account_currency: PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY.to_string(),
            cash_micros: Some(snapshot.portfolio.cash_micros),
            nav_micros: agg.nav_micros.map(clamp_i128_to_i64),
            gross_exposure_micros: agg.gross_exposure_micros.map(clamp_i128_to_i64),
            net_exposure_micros: agg.net_exposure_micros.map(clamp_i128_to_i64),
            positions,
            asset_class_exposures,
            currency_exposures,
            blockers,
        }),
    )
        .into_response()
}
