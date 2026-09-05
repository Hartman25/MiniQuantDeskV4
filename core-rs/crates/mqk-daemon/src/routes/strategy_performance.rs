// core-rs/crates/mqk-daemon/src/routes/strategy_performance.rs
//
// WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01 (P3): deterministic, read-only
// exact-semantic-strategy performance analytics built on top of the P2
// closed-trade authority (`resolve_authoritative_closed_trade_view`).
//
// WAVE05-STRATEGY-DECAY-AND-REGIME-MONITOR-01 (P4): additively extends each
// row with a conservative forward performance-decay monitor and observational
// research-only current market-regime context.
//
// WAVE05-STRATEGY-RISK-VISIBILITY-01 (P5): additively extends each row with
// deterministic, VISIBILITY-ONLY strategy-level risk visibility built from
// P3/P4 plus the existing durable strategy-suppression READ seam
// (`mqk_db::fetch_active_suppression_for_strategy`). No automated
// suppression, no automated clearing, no order/promotion/accounting change --
// this route never calls `insert_strategy_suppression`/
// `clear_strategy_suppression`.
//
// GET /api/v1/strategy/performance?run_id=<uuid>
//
// Read-only overall: no DB write anywhere in this file, no order/broker/OMS
// path touched, no promotion-state write, no suppression write. Reuses the
// exact run-resolution seam `routes/durable_portfolio.rs` defines
// (`resolve_run`/`parse_explicit_run_id`) and the exact shared closed-trade
// authority `routes/paper_journal.rs`'s `closed_trades_lane` also consumes --
// this route never hand-rolls a second provenance classifier, snapshot-
// authority validator, canonical replay, or durable-accounting comparison.
//
// Analytics identity is `(strategy_id, strategy_semantic_fingerprint)` --
// never `strategy_id` alone. Only P2 `"attributed"` closure fragments ever
// contribute to a performance row; every other attribution state is visible
// only via `attribution_coverage`, never folded into exact strategy metrics.
// Suppression (P5) is the one exception: it is keyed by `strategy_id` alone,
// matching the real admission-gate semantics -- see
// `StrategyRiskVisibility::active_strategy_suppression`'s doc.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use mqk_backtest::regime::{
    detect_market_regime, MarketRegimeInput, MarketRegimeKind, MarketRegimePolicy,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::durable_portfolio::{parse_explicit_run_id, resolve_run, RunIdParam, RunResolution};
use crate::api_types::{
    StrategyDecayMonitor, StrategyDecayWindowMetrics, StrategyPerformanceCoverageBucket,
    StrategyPerformanceResponse, StrategyPerformanceRow, StrategyRegimeContext,
    StrategyRiskVisibility,
};
use crate::dynamic_selection_dispatch_authority::timeframe_secs_to_db_label;
use crate::state::{resolve_authoritative_closed_trade_view, AppState, ClosureAttribution, ClosureFragment};

const CANONICAL: &str = "/api/v1/strategy/performance";

/// Every P&L field in this response is gross trading P&L -- fees are never
/// netted in. See `FEE_ALLOCATION_STATE`.
const PNL_BASIS: &str = "gross_realized_before_fees";

/// Fees are not currently allocated deterministically across FIFO closure
/// fragments/close events. No `net_pnl`/`net_expectancy`/`after_cost_*`
/// field exists anywhere in this response.
const FEE_ALLOCATION_STATE: &str = "not_allocated_to_strategy_close_events";

/// P4.2: fixed deterministic decay monitor windows.
const BASELINE_EVENT_COUNT_REQUIRED: usize = 10;
const RECENT_EVENT_COUNT_REQUIRED: usize = 5;
const TOTAL_EVENT_COUNT_REQUIRED: usize = BASELINE_EVENT_COUNT_REQUIRED + RECENT_EVENT_COUNT_REQUIRED;

/// P4.5: this route's regime detection is ALWAYS research-only observational
/// context -- it must never gate execution, risk, promotion, or suppression.
const REGIME_AUTHORITY: &str = "research_only_observational";

/// P4.7: bounded recent completed-bar window handed to the regime detector.
/// Comfortably above `MarketRegimePolicy::conservative_defaults().min_bars`
/// (8) without being unbounded.
const REGIME_BAR_WINDOW: i64 = 20;

fn unavailable_response(
    status: StatusCode,
    truth_state: &str,
    run_id: Option<String>,
    accounting_provenance_state: Option<String>,
) -> Response {
    (
        status,
        Json(StrategyPerformanceResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: truth_state.to_string(),
            run_id,
            accounting_provenance_state,
            pnl_basis: PNL_BASIS.to_string(),
            fee_allocation_state: FEE_ALLOCATION_STATE.to_string(),
            rows: vec![],
            attribution_coverage: vec![],
            total_gross_realized_pnl_micros: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Pure analytics (P3.3 - P3.6) -- no I/O, fully unit-testable.
// ---------------------------------------------------------------------------

/// `Some((strategy_id, strategy_semantic_fingerprint))` only for a fragment
/// whose attribution is exactly `Attributed` AND both sides' resolved
/// identity genuinely agree (defensive re-check; `ClosureAttribution::
/// Attributed` already guarantees this by construction). `None` for every
/// other fragment -- it never contributes to exact strategy analytics.
fn attributed_fragment_identity(f: &ClosureFragment) -> Option<(String, String)> {
    if f.attribution != ClosureAttribution::Attributed {
        return None;
    }
    let (open_id, open_fp) = f.open_lineage.identity_pair();
    let (close_id, close_fp) = f.close_lineage.identity_pair();
    match (open_id, open_fp, close_id, close_fp) {
        (Some(oid), Some(ofp), Some(cid), Some(cfp)) if oid == cid && ofp == cfp => {
            Some((cid, cfp))
        }
        _ => None,
    }
}

/// One P3.4 `AttributedCloseEvent`: attributed FIFO fragments sharing the
/// same closing economic fill (`close_inbox_id` + `close_internal_order_id`)
/// and the same exact semantic strategy identity, collapsed into one event.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttributedCloseEvent {
    qty: i64,
    gross_pnl_micros: i64,
    /// Raw fragment count collapsed into this one event (P3-08: two FIFO
    /// fragments closed by one economic close order -> `fragment_count = 2`,
    /// but this is still exactly one close event).
    fragment_count: i64,
    /// The exact closing order this event's fragments share. P4.6 resolves
    /// each event's durable symbol/timeframe context from this field via
    /// `mqk_db::fetch_order_symbol_timeframe_context` -- never from current
    /// config or a symbol-latest lookup.
    close_internal_order_id: String,
}

/// Group `fragments` into ordered `AttributedCloseEvent` series per exact
/// semantic-strategy identity. Events within each strategy's series are
/// ordered deterministically by `(close_inbox_id, close_internal_order_id)`
/// -- never by timestamp, since `close_inbox_id` is already canonical.
/// `(strategy_id, strategy_semantic_fingerprint, close_inbox_id, close_internal_order_id)`
/// -> `(qty_sum, gross_pnl_sum, fragment_count)`.
type EventAccumulatorMap = BTreeMap<(String, String, i64, String), (i64, i64, i64)>;

fn build_attributed_close_events(
    fragments: &[ClosureFragment],
) -> BTreeMap<(String, String), Vec<AttributedCloseEvent>> {
    // Intermediate grouping key includes the close identity so multiple
    // fragments from the SAME closing fill collapse into one event; the key
    // order (strategy_id, fingerprint, close_inbox_id, close_internal_order_id)
    // is exactly the deterministic event order this function promises.
    let mut event_map: EventAccumulatorMap = BTreeMap::new();

    for f in fragments {
        let Some((strategy_id, fingerprint)) = attributed_fragment_identity(f) else {
            continue;
        };
        let key = (
            strategy_id,
            fingerprint,
            f.close_inbox_id,
            f.close_internal_order_id.clone(),
        );
        let entry = event_map.entry(key).or_insert((0, 0, 0));
        entry.0 = entry.0.saturating_add(f.qty);
        entry.1 = entry.1.saturating_add(f.gross_realized_pnl_micros);
        entry.2 = entry.2.saturating_add(1);
    }

    let mut by_strategy: BTreeMap<(String, String), Vec<AttributedCloseEvent>> = BTreeMap::new();
    for ((strategy_id, fingerprint, _close_inbox_id, close_internal_order_id), (qty, pnl, frag_count)) in
        event_map
    {
        by_strategy
            .entry((strategy_id, fingerprint))
            .or_default()
            .push(AttributedCloseEvent {
                qty,
                gross_pnl_micros: pnl,
                fragment_count: frag_count,
                close_internal_order_id,
            });
    }
    by_strategy
}

/// P3.5 drawdown definition: cumulative realized gross P&L across the
/// ordered close-event sequence, starting at 0, tracking a running peak;
/// drawdown = peak - current cumulative. Reports the maximum (always
/// nonnegative) drawdown observed. This is REALIZED closed-P&L drawdown --
/// NOT account-equity, mark-to-market, or intratrade drawdown, and NOT MAE.
fn compute_max_realized_pnl_drawdown(event_pnls_in_order: &[i64]) -> i64 {
    let mut cumulative: i64 = 0;
    let mut peak: i64 = 0;
    let mut max_drawdown: i64 = 0;
    for &pnl in event_pnls_in_order {
        cumulative = cumulative.saturating_add(pnl);
        if cumulative > peak {
            peak = cumulative;
        }
        let drawdown = peak.saturating_sub(cumulative);
        if drawdown > max_drawdown {
            max_drawdown = drawdown;
        }
    }
    max_drawdown
}

/// Shared aggregate over a slice of close-event gross P&Ls -- the common
/// core [`compute_performance_row`] (P3.5, full series) and the P4 decay
/// monitor (baseline/recent windows) both need. No I/O.
struct EventPnlAggregate {
    event_count: i64,
    gross_realized_pnl_micros: i64,
    gross_profit_micros: i64,
    gross_loss_abs_micros: i64,
    winning_close_event_count: i64,
    losing_close_event_count: i64,
    flat_close_event_count: i64,
    hit_rate: Option<f64>,
    gross_expectancy_micros_per_close_event: Option<f64>,
    max_realized_pnl_drawdown_micros: i64,
}

fn aggregate_event_pnls(event_pnls_in_order: &[i64]) -> EventPnlAggregate {
    let event_count = event_pnls_in_order.len() as i64;
    let gross_realized_pnl_micros = event_pnls_in_order
        .iter()
        .fold(0i64, |acc, &p| acc.saturating_add(p));
    let gross_profit_micros = event_pnls_in_order
        .iter()
        .filter(|&&p| p > 0)
        .fold(0i64, |acc, &p| acc.saturating_add(p));
    let gross_loss_abs_micros = event_pnls_in_order
        .iter()
        .filter(|&&p| p < 0)
        .fold(0i64, |acc, &p| acc.saturating_add(p.unsigned_abs() as i64));

    let winning_close_event_count = event_pnls_in_order.iter().filter(|&&p| p > 0).count() as i64;
    let losing_close_event_count = event_pnls_in_order.iter().filter(|&&p| p < 0).count() as i64;
    let flat_close_event_count = event_pnls_in_order.iter().filter(|&&p| p == 0).count() as i64;

    let hit_rate_denominator = winning_close_event_count + losing_close_event_count;
    let hit_rate = if hit_rate_denominator == 0 {
        None
    } else {
        Some(winning_close_event_count as f64 / hit_rate_denominator as f64)
    };

    let gross_expectancy_micros_per_close_event = if event_count == 0 {
        None
    } else {
        Some(gross_realized_pnl_micros as f64 / event_count as f64)
    };

    let max_realized_pnl_drawdown_micros = compute_max_realized_pnl_drawdown(event_pnls_in_order);

    EventPnlAggregate {
        event_count,
        gross_realized_pnl_micros,
        gross_profit_micros,
        gross_loss_abs_micros,
        winning_close_event_count,
        losing_close_event_count,
        flat_close_event_count,
        hit_rate,
        gross_expectancy_micros_per_close_event,
        max_realized_pnl_drawdown_micros,
    }
}

/// Build one exact semantic-strategy performance row (P3.5) from its ordered
/// attributed close-event series.
fn compute_performance_row(
    strategy_id: String,
    strategy_semantic_fingerprint: String,
    events: &[AttributedCloseEvent],
    decay_monitor: StrategyDecayMonitor,
    regime_context: StrategyRegimeContext,
    risk_visibility: StrategyRiskVisibility,
) -> StrategyPerformanceRow {
    let attributed_fragment_count: i64 = events.iter().map(|e| e.fragment_count).sum();
    let attributed_closed_qty: i64 = events.iter().map(|e| e.qty).sum();
    let event_pnls: Vec<i64> = events.iter().map(|e| e.gross_pnl_micros).collect();
    let agg = aggregate_event_pnls(&event_pnls);

    let average_win_micros = if agg.winning_close_event_count == 0 {
        None
    } else {
        Some(agg.gross_profit_micros as f64 / agg.winning_close_event_count as f64)
    };
    let average_loss_abs_micros = if agg.losing_close_event_count == 0 {
        None
    } else {
        Some(agg.gross_loss_abs_micros as f64 / agg.losing_close_event_count as f64)
    };

    // NEVER infinity/NaN/a fabricated sentinel: `None` when there is no loss
    // to divide by.
    let profit_factor = if agg.gross_loss_abs_micros == 0 {
        None
    } else {
        Some(agg.gross_profit_micros as f64 / agg.gross_loss_abs_micros as f64)
    };

    StrategyPerformanceRow {
        strategy_id,
        strategy_semantic_fingerprint,
        attributed_fragment_count,
        attributed_close_event_count: agg.event_count,
        attributed_closed_qty,
        gross_realized_pnl_micros: agg.gross_realized_pnl_micros,
        gross_profit_micros: agg.gross_profit_micros,
        gross_loss_abs_micros: agg.gross_loss_abs_micros,
        winning_close_event_count: agg.winning_close_event_count,
        losing_close_event_count: agg.losing_close_event_count,
        flat_close_event_count: agg.flat_close_event_count,
        hit_rate: agg.hit_rate,
        gross_expectancy_micros_per_close_event: agg.gross_expectancy_micros_per_close_event,
        average_win_micros,
        average_loss_abs_micros,
        profit_factor,
        max_realized_pnl_drawdown_micros: agg.max_realized_pnl_drawdown_micros,
        decay_monitor,
        regime_context,
        risk_visibility,
    }
}

// ---------------------------------------------------------------------------
// P4.2 - P4.4: decay monitor (pure, no I/O)
// ---------------------------------------------------------------------------

/// P4.2: split an ordered attributed close-event series into
/// `(baseline, recent)` slices when `events.len() >= 15` -- `recent` is the
/// newest 5, `baseline` is the 10 immediately preceding (never overlapping).
/// `None` when there are fewer than 15 events (`decay_state ==
/// "insufficient_data"`). Events older than the 15-event window are ignored
/// by this comparison (still visible in the row's own lifetime metrics).
fn split_decay_windows(events: &[AttributedCloseEvent]) -> Option<(&[AttributedCloseEvent], &[AttributedCloseEvent])> {
    let n = events.len();
    if n < TOTAL_EVENT_COUNT_REQUIRED {
        return None;
    }
    let recent = &events[n - RECENT_EVENT_COUNT_REQUIRED..];
    let baseline = &events[n - TOTAL_EVENT_COUNT_REQUIRED..n - RECENT_EVENT_COUNT_REQUIRED];
    Some((baseline, recent))
}

/// P4.4: closed-vocabulary conservative decay classifier. Detects ONLY a
/// strong gross-expectancy sign reversal -- never an arbitrary percentage
/// threshold. `decay_observed` is a deterministic monitoring flag, NOT proof
/// that the strategy's true alpha has disappeared.
fn classify_decay_state(
    baseline_expectancy: Option<f64>,
    recent_expectancy: Option<f64>,
) -> &'static str {
    match (baseline_expectancy, recent_expectancy) {
        (Some(baseline), Some(recent)) if baseline > 0.0 && recent < 0.0 => "decay_observed",
        (Some(baseline), Some(recent)) if baseline <= 0.0 && recent > 0.0 => "improvement_observed",
        (Some(_), Some(_)) => "no_expectancy_sign_flip",
        _ => "insufficient_data",
    }
}

fn window_metrics_from_aggregate(agg: EventPnlAggregate) -> StrategyDecayWindowMetrics {
    StrategyDecayWindowMetrics {
        event_count: agg.event_count,
        gross_realized_pnl_micros: agg.gross_realized_pnl_micros,
        gross_expectancy_micros_per_close_event: agg.gross_expectancy_micros_per_close_event,
        hit_rate: agg.hit_rate,
        gross_profit_micros: agg.gross_profit_micros,
        gross_loss_abs_micros: agg.gross_loss_abs_micros,
        max_realized_pnl_drawdown_micros: agg.max_realized_pnl_drawdown_micros,
    }
}

/// Build the P4 decay monitor for one strategy's ordered attributed
/// close-event series. Pure/no I/O -- reuses P3's exact event series,
/// counting close events (never raw FIFO fragments) toward the 15-event
/// sample size, and never mixes events from a different semantic-strategy
/// identity (each `(strategy_id, fingerprint)` gets its own independent
/// series upstream in `build_attributed_close_events`).
fn compute_decay_monitor(events: &[AttributedCloseEvent]) -> StrategyDecayMonitor {
    let Some((baseline_events, recent_events)) = split_decay_windows(events) else {
        return StrategyDecayMonitor {
            decay_state: "insufficient_data".to_string(),
            baseline: None,
            recent: None,
            expectancy_delta_micros: None,
            hit_rate_delta: None,
        };
    };

    let baseline_pnls: Vec<i64> = baseline_events.iter().map(|e| e.gross_pnl_micros).collect();
    let recent_pnls: Vec<i64> = recent_events.iter().map(|e| e.gross_pnl_micros).collect();
    let baseline_agg = aggregate_event_pnls(&baseline_pnls);
    let recent_agg = aggregate_event_pnls(&recent_pnls);

    let decay_state = classify_decay_state(
        baseline_agg.gross_expectancy_micros_per_close_event,
        recent_agg.gross_expectancy_micros_per_close_event,
    );
    let expectancy_delta_micros = match (
        recent_agg.gross_expectancy_micros_per_close_event,
        baseline_agg.gross_expectancy_micros_per_close_event,
    ) {
        (Some(r), Some(b)) => Some(r - b),
        _ => None,
    };
    let hit_rate_delta = match (recent_agg.hit_rate, baseline_agg.hit_rate) {
        (Some(r), Some(b)) => Some(r - b),
        _ => None,
    };

    StrategyDecayMonitor {
        decay_state: decay_state.to_string(),
        baseline: Some(window_metrics_from_aggregate(baseline_agg)),
        recent: Some(window_metrics_from_aggregate(recent_agg)),
        expectancy_delta_micros,
        hit_rate_delta,
    }
}

// ---------------------------------------------------------------------------
// P4.5 - P4.7: observational current market-regime context
// ---------------------------------------------------------------------------

fn unavailable_regime_context(regime_truth_state: &str) -> StrategyRegimeContext {
    StrategyRegimeContext {
        regime_truth_state: regime_truth_state.to_string(),
        regime_authority: REGIME_AUTHORITY.to_string(),
        symbol: None,
        timeframe_secs: None,
        regime_kind: None,
        confidence: None,
        reason_codes: vec![],
        input_bar_count: None,
        valid_bar_count: None,
    }
}

/// Convert one canonical `md_bars` row into the engine's `BacktestBar` shape.
/// Mirrors `routes/backtests.rs::md_bar_row_to_backtest_bar`. `day_id`/
/// `reject_window_id` are deterministically derived, bounded fields the
/// regime detector's own feature calculation does not consume.
fn md_bar_row_to_backtest_bar(r: mqk_db::MdBarRow) -> mqk_backtest::BacktestBar {
    let day_id = epoch_secs_to_yyyymmdd(r.end_ts);
    let reject_window_id = r.end_ts.div_euclid(60).try_into().unwrap_or(u32::MAX);
    mqk_backtest::BacktestBar {
        symbol: r.symbol,
        end_ts: r.end_ts,
        open_micros: r.open_micros,
        high_micros: r.high_micros,
        low_micros: r.low_micros,
        close_micros: r.close_micros,
        volume: r.volume,
        is_complete: r.is_complete,
        day_id,
        reject_window_id,
    }
}

fn epoch_secs_to_yyyymmdd(epoch_secs: i64) -> u32 {
    let days = epoch_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    (y * 10_000 + m * 100 + d).try_into().unwrap_or(19700101)
}

/// civil_from_days (public domain; Howard Hinnant)
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let y = (yoe as i32) + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// P4.6/P4.7: resolve the current observational market-regime context for
/// one strategy's ordered attributed close-event series. The "current"
/// symbol is the MOST RECENT event's exact durable symbol; every OTHER event
/// sharing that exact symbol must resolve to the exact same `timeframe_secs`
/// or the whole context fails closed to `"context_ambiguous"`. Never selects
/// a symbol/timeframe arbitrarily, never reads current config/registry
/// state, never calls a provider/broker/network API -- completed `md_bars`
/// only.
async fn resolve_strategy_regime_context(
    db: &PgPool,
    run_id: Uuid,
    events: &[AttributedCloseEvent],
) -> StrategyRegimeContext {
    if events.is_empty() {
        return unavailable_regime_context("context_unavailable");
    }

    let mut resolved: Vec<Option<(String, i64)>> = Vec::with_capacity(events.len());
    for e in events {
        match mqk_db::fetch_order_symbol_timeframe_context(db, run_id, &e.close_internal_order_id)
            .await
        {
            Ok(ctx) => resolved.push(ctx),
            Err(err) => {
                tracing::warn!(
                    error = %err, run_id = %run_id,
                    "strategy_performance_regime_context_query_failed"
                );
                return unavailable_regime_context("query_failed");
            }
        }
    }

    let Some((current_symbol, current_timeframe_secs)) = resolved.last().cloned().flatten() else {
        return unavailable_regime_context("context_unavailable");
    };

    for other in resolved.iter().flatten() {
        if other.0 == current_symbol && other.1 != current_timeframe_secs {
            return unavailable_regime_context("context_ambiguous");
        }
    }

    let Some(db_timeframe_label) = timeframe_secs_to_db_label(current_timeframe_secs) else {
        return unavailable_regime_context("context_unavailable");
    };

    let rows = match mqk_db::fetch_recent_completed_bars_for_strategy(
        db,
        &current_symbol,
        db_timeframe_label,
        REGIME_BAR_WINDOW,
    )
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(
                error = %err, run_id = %run_id, symbol = %current_symbol,
                "strategy_performance_regime_bars_query_failed"
            );
            return unavailable_regime_context("query_failed");
        }
    };

    let bars: Vec<mqk_backtest::BacktestBar> =
        rows.into_iter().map(md_bar_row_to_backtest_bar).collect();
    let input = MarketRegimeInput::from_bars(
        bars,
        Some(current_symbol.clone()),
        Some(db_timeframe_label.to_string()),
    );
    let classification = detect_market_regime(&input, &MarketRegimePolicy::conservative_defaults());

    let regime_truth_state = if classification.kind == MarketRegimeKind::InsufficientData {
        "insufficient_data"
    } else {
        "active_observational"
    };

    StrategyRegimeContext {
        regime_truth_state: regime_truth_state.to_string(),
        regime_authority: REGIME_AUTHORITY.to_string(),
        symbol: Some(current_symbol),
        timeframe_secs: Some(current_timeframe_secs),
        regime_kind: Some(classification.kind.code().to_string()),
        confidence: Some(classification.confidence.score),
        reason_codes: classification
            .reason_codes
            .iter()
            .map(|r| r.code().to_string())
            .collect(),
        input_bar_count: Some(classification.features.input_bar_count as i64),
        valid_bar_count: Some(classification.features.valid_bar_count as i64),
    }
}

/// P3.6 attribution coverage: deterministic fragment count + gross realized
/// P&L total per P2 attribution state, across ALL fragments (not just
/// `attributed`). `sum(bucket.gross_realized_pnl_micros)` always equals the
/// upstream projection's total gross realized P&L.
fn compute_attribution_coverage(fragments: &[ClosureFragment]) -> Vec<StrategyPerformanceCoverageBucket> {
    let mut buckets: BTreeMap<&'static str, (i64, i64)> = BTreeMap::new();
    for f in fragments {
        let entry = buckets.entry(f.attribution.as_str()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(f.gross_realized_pnl_micros);
    }
    buckets
        .into_iter()
        .map(|(state, (count, pnl))| StrategyPerformanceCoverageBucket {
            attribution_state: state.to_string(),
            fragment_count: count,
            gross_realized_pnl_micros: pnl,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// P5.1 - P5.5: read-only strategy risk visibility (pure, no I/O beyond the
// one suppression lookup the caller performs and passes in).
// ---------------------------------------------------------------------------

/// P5.3 closed-vocabulary precedence: `unavailable` > `suppressed` >
/// `insufficient_data` > `watch` > `normal`. `upstream_active` is always
/// `true` at the one call site in this file today (a row only exists when
/// the response's own `truth_state == "active"`) -- the parameter exists so
/// this pure function's full precedence order stays independently testable.
fn classify_risk_visibility_state(
    upstream_active: bool,
    suppression_active: bool,
    decay_state: &str,
) -> &'static str {
    if !upstream_active {
        return "unavailable";
    }
    if suppression_active {
        return "suppressed";
    }
    if decay_state == "insufficient_data" {
        return "insufficient_data";
    }
    if decay_state == "decay_observed" {
        return "watch";
    }
    "normal"
}

/// P5.5: text/visibility only -- NEVER invokes a mutation.
fn recommended_operator_action(risk_visibility_state: &str) -> &'static str {
    match risk_visibility_state {
        "suppressed" => "already_suppressed",
        "watch" => "review",
        "normal" => "none",
        // "unavailable" | "insufficient_data" | any unrecognized state fails
        // closed to the same conservative recommendation.
        _ => "insufficient_evidence",
    }
}

/// P5.4 required risk flags. The four attribution-coverage flags
/// (`semantic_identity_change_excluded_pnl`, `cross_strategy_closure_pnl`,
/// `incomplete_lineage_pnl`, `manual_mixed_closure_pnl`) are response-wide
/// facts about the resolved run's closures, not scoped to one exact
/// strategy row -- a cross-strategy or manual closure by definition cannot
/// be attributed to a single exact semantic-strategy identity without being
/// arbitrary, so these flags surface identically on every row.
/// `observational_high_volatility_context` is informational ONLY -- it must
/// never by itself change `risk_visibility_state` (see
/// `classify_risk_visibility_state`, which never reads regime context at
/// all).
#[allow(clippy::too_many_arguments)]
fn compute_risk_flags(
    suppression_active: bool,
    decay_state: &str,
    coverage_has_semantic_identity_changed: bool,
    coverage_has_cross_strategy: bool,
    coverage_has_incomplete_lineage: bool,
    coverage_has_manual_mixed: bool,
    regime_kind_is_high_volatility: bool,
) -> Vec<String> {
    let mut flags = Vec::new();
    if suppression_active {
        flags.push("active_strategy_suppression".to_string());
    }
    if decay_state == "decay_observed" {
        flags.push("gross_expectancy_sign_flip_negative".to_string());
    }
    if coverage_has_semantic_identity_changed {
        flags.push("semantic_identity_change_excluded_pnl".to_string());
    }
    if coverage_has_cross_strategy {
        flags.push("cross_strategy_closure_pnl".to_string());
    }
    if coverage_has_incomplete_lineage {
        flags.push("incomplete_lineage_pnl".to_string());
    }
    if coverage_has_manual_mixed {
        flags.push("manual_mixed_closure_pnl".to_string());
    }
    if regime_kind_is_high_volatility {
        flags.push("observational_high_volatility_context".to_string());
    }
    flags
}

/// Resolve the P5 risk-visibility surface for one exact semantic-strategy
/// row. The suppression lookup is the ONLY I/O this function performs, keyed
/// by `strategy_id` alone (never fingerprint) -- matching the real
/// admission-gate semantics; see `StrategyRiskVisibility::
/// active_strategy_suppression`'s doc. Never calls
/// `insert_strategy_suppression`/`clear_strategy_suppression`.
#[allow(clippy::too_many_arguments)]
async fn resolve_risk_visibility(
    db: &PgPool,
    strategy_id: &str,
    decay_state: &str,
    coverage_has_semantic_identity_changed: bool,
    coverage_has_cross_strategy: bool,
    coverage_has_incomplete_lineage: bool,
    coverage_has_manual_mixed: bool,
    regime_kind_is_high_volatility: bool,
) -> StrategyRiskVisibility {
    let suppression = match mqk_db::fetch_active_suppression_for_strategy(db, strategy_id).await {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                error = %err, strategy_id,
                "strategy_performance_suppression_query_failed"
            );
            None
        }
    };
    let suppression_active = suppression.is_some();

    let risk_visibility_state =
        classify_risk_visibility_state(true, suppression_active, decay_state);
    let recommended_operator_action = recommended_operator_action(risk_visibility_state);
    let risk_flags = compute_risk_flags(
        suppression_active,
        decay_state,
        coverage_has_semantic_identity_changed,
        coverage_has_cross_strategy,
        coverage_has_incomplete_lineage,
        coverage_has_manual_mixed,
        regime_kind_is_high_volatility,
    );

    StrategyRiskVisibility {
        risk_visibility_state: risk_visibility_state.to_string(),
        risk_flags,
        active_strategy_suppression: suppression_active,
        active_suppression_id: suppression.as_ref().map(|s| s.suppression_id.to_string()),
        active_suppression_trigger_domain: suppression.as_ref().map(|s| s.trigger_domain.clone()),
        active_suppression_trigger_reason: suppression.as_ref().map(|s| s.trigger_reason.clone()),
        recommended_operator_action: recommended_operator_action.to_string(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/performance
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_performance(
    State(st): State<Arc<AppState>>,
    Query(params): Query<RunIdParam>,
) -> Response {
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
        return unavailable_response(StatusCode::OK, "db_unavailable", None, None);
    };

    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::NotFound => {
            return unavailable_response(StatusCode::OK, "not_found", None, None);
        }
        RunResolution::QueryFailed => {
            return unavailable_response(StatusCode::OK, "query_failed", None, None);
        }
    };

    if run.mode != "PAPER" {
        return unavailable_response(
            StatusCode::OK,
            "unsupported_source",
            Some(run.run_id.to_string()),
            None,
        );
    }

    let view = resolve_authoritative_closed_trade_view(db, run.run_id).await;

    // P3.7: strategy performance metrics are authoritative ONLY when the
    // shared closed-trade authority is exactly "active" -- "incomplete"
    // (visible-but-non-authoritative fill history, e.g. a stale accounting
    // snapshot/watermark) must never be transformed into performance rows,
    // fabricated zero, or a partial attribution_coverage.
    if view.truth_state != "active" {
        return unavailable_response(
            StatusCode::OK,
            view.truth_state,
            Some(run.run_id.to_string()),
            view.accounting_provenance_state.map(str::to_string),
        );
    }

    let attribution_coverage = compute_attribution_coverage(&view.fragments);
    let total_gross_realized_pnl_micros: i64 = attribution_coverage
        .iter()
        .fold(0i64, |acc, b| acc.saturating_add(b.gross_realized_pnl_micros));

    // P5.4: response-wide attribution-coverage facts, computed once and
    // applied identically to every row (see `compute_risk_flags`'s doc).
    let has_coverage_bucket = |state: &str| {
        attribution_coverage
            .iter()
            .any(|b| b.attribution_state == state)
    };
    let coverage_has_semantic_identity_changed = has_coverage_bucket("semantic_identity_changed");
    let coverage_has_cross_strategy = has_coverage_bucket("cross_strategy");
    let coverage_has_incomplete_lineage = has_coverage_bucket("lineage_incomplete")
        || has_coverage_bucket("lineage_invalid")
        || has_coverage_bucket("lineage_missing");
    let coverage_has_manual_mixed = has_coverage_bucket("manual_or_mixed");

    let events_by_strategy = build_attributed_close_events(&view.fragments);
    let mut rows = Vec::with_capacity(events_by_strategy.len());
    for ((strategy_id, fingerprint), events) in events_by_strategy {
        // P4: decay monitoring is pure/no I/O; regime context resolution and
        // the P5 suppression lookup are read-only DB (never provider/broker/
        // network) and are awaited sequentially -- this route is
        // observational, not hot-path, and row counts are bounded by
        // distinct exact semantic-strategy identities.
        let decay_monitor = compute_decay_monitor(&events);
        let regime_context = resolve_strategy_regime_context(db, run.run_id, &events).await;
        let regime_kind_is_high_volatility = regime_context.regime_kind.as_deref() == Some("high_volatility");
        let risk_visibility = resolve_risk_visibility(
            db,
            &strategy_id,
            &decay_monitor.decay_state,
            coverage_has_semantic_identity_changed,
            coverage_has_cross_strategy,
            coverage_has_incomplete_lineage,
            coverage_has_manual_mixed,
            regime_kind_is_high_volatility,
        )
        .await;
        rows.push(compute_performance_row(
            strategy_id,
            fingerprint,
            &events,
            decay_monitor,
            regime_context,
            risk_visibility,
        ));
    }

    (
        StatusCode::OK,
        Json(StrategyPerformanceResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run.run_id.to_string()),
            accounting_provenance_state: view.accounting_provenance_state.map(str::to_string),
            pnl_basis: PNL_BASIS.to_string(),
            fee_allocation_state: FEE_ALLOCATION_STATE.to_string(),
            rows,
            attribution_coverage,
            total_gross_realized_pnl_micros: Some(total_gross_realized_pnl_micros),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        classify_risk_visibility_state, compute_max_realized_pnl_drawdown, recommended_operator_action,
    };

    /// P3-11: event pnl sequence +100, -40, -80, +20 must produce exact
    /// deterministic max realized-P&L drawdown.
    ///
    /// cumulative: 100, 60, -20, 0
    /// peak:       100, 100, 100, 100
    /// drawdown:     0,  40,  120, 100 -> max = 120
    #[test]
    fn drawdown_matches_hand_computed_sequence() {
        assert_eq!(
            compute_max_realized_pnl_drawdown(&[100, -40, -80, 20]),
            120
        );
    }

    #[test]
    fn drawdown_all_wins_is_zero() {
        assert_eq!(compute_max_realized_pnl_drawdown(&[10, 20, 30]), 0);
    }

    #[test]
    fn drawdown_empty_series_is_zero() {
        assert_eq!(compute_max_realized_pnl_drawdown(&[]), 0);
    }

    #[test]
    fn drawdown_never_recovers_max_is_final_trough() {
        assert_eq!(compute_max_realized_pnl_drawdown(&[50, -10, -10, -10]), 30);
    }

    /// P5-10: upstream P3 performance authority not active -> unavailable,
    /// unconditionally -- suppression/decay state must never override this
    /// top-priority precedence rule.
    #[test]
    fn risk_visibility_state_unavailable_when_upstream_not_active() {
        assert_eq!(
            classify_risk_visibility_state(false, true, "decay_observed"),
            "unavailable"
        );
        assert_eq!(
            classify_risk_visibility_state(false, false, "no_expectancy_sign_flip"),
            "unavailable"
        );
    }

    #[test]
    fn risk_visibility_state_precedence_suppressed_beats_decay() {
        // Even a decay_observed strategy must report "suppressed", not "watch",
        // once an active suppression exists.
        assert_eq!(
            classify_risk_visibility_state(true, true, "decay_observed"),
            "suppressed"
        );
    }

    #[test]
    fn risk_visibility_state_insufficient_data_and_watch_and_normal() {
        assert_eq!(
            classify_risk_visibility_state(true, false, "insufficient_data"),
            "insufficient_data"
        );
        assert_eq!(
            classify_risk_visibility_state(true, false, "decay_observed"),
            "watch"
        );
        assert_eq!(
            classify_risk_visibility_state(true, false, "no_expectancy_sign_flip"),
            "normal"
        );
        assert_eq!(
            classify_risk_visibility_state(true, false, "improvement_observed"),
            "normal"
        );
    }

    /// P5-11: recommended-action mapping is exact and exhaustive over the
    /// closed `risk_visibility_state` vocabulary.
    #[test]
    fn recommended_action_mapping_is_exact() {
        assert_eq!(recommended_operator_action("unavailable"), "insufficient_evidence");
        assert_eq!(recommended_operator_action("insufficient_data"), "insufficient_evidence");
        assert_eq!(recommended_operator_action("suppressed"), "already_suppressed");
        assert_eq!(recommended_operator_action("watch"), "review");
        assert_eq!(recommended_operator_action("normal"), "none");
    }
}
