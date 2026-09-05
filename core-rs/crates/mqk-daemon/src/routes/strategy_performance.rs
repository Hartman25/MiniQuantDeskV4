// core-rs/crates/mqk-daemon/src/routes/strategy_performance.rs
//
// WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01 (P3): deterministic, read-only
// exact-semantic-strategy performance analytics built on top of the P2
// closed-trade authority (`resolve_authoritative_closed_trade_view`).
//
// GET /api/v1/strategy/performance?run_id=<uuid>
//
// Read-only: no DB write, no order/broker/OMS path touched, no promotion or
// suppression state read or written. Reuses the exact run-resolution seam
// `routes/durable_portfolio.rs` defines (`resolve_run`/`parse_explicit_run_id`)
// and the exact shared closed-trade authority `routes/paper_journal.rs`'s
// `closed_trades_lane` also consumes -- this route never hand-rolls a second
// provenance classifier, snapshot-authority validator, canonical replay, or
// durable-accounting comparison.
//
// Analytics identity is `(strategy_id, strategy_semantic_fingerprint)` --
// never `strategy_id` alone. Only P2 `"attributed"` closure fragments ever
// contribute to a performance row; every other attribution state is visible
// only via `attribution_coverage`, never folded into exact strategy metrics.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::durable_portfolio::{parse_explicit_run_id, resolve_run, RunIdParam, RunResolution};
use crate::api_types::{
    StrategyPerformanceCoverageBucket, StrategyPerformanceResponse, StrategyPerformanceRow,
};
use crate::state::{resolve_authoritative_closed_trade_view, AppState, ClosureAttribution, ClosureFragment};

const CANONICAL: &str = "/api/v1/strategy/performance";

/// Every P&L field in this response is gross trading P&L -- fees are never
/// netted in. See `FEE_ALLOCATION_STATE`.
const PNL_BASIS: &str = "gross_realized_before_fees";

/// Fees are not currently allocated deterministically across FIFO closure
/// fragments/close events. No `net_pnl`/`net_expectancy`/`after_cost_*`
/// field exists anywhere in this response.
const FEE_ALLOCATION_STATE: &str = "not_allocated_to_strategy_close_events";

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
    for ((strategy_id, fingerprint, _close_inbox_id, _close_internal_order_id), (qty, pnl, frag_count)) in
        event_map
    {
        by_strategy
            .entry((strategy_id, fingerprint))
            .or_default()
            .push(AttributedCloseEvent {
                qty,
                gross_pnl_micros: pnl,
                fragment_count: frag_count,
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

/// Build one exact semantic-strategy performance row (P3.5) from its ordered
/// attributed close-event series.
fn compute_performance_row(
    strategy_id: String,
    strategy_semantic_fingerprint: String,
    events: &[AttributedCloseEvent],
) -> StrategyPerformanceRow {
    let attributed_fragment_count: i64 = events.iter().map(|e| e.fragment_count).sum();
    let attributed_close_event_count: i64 = events.len() as i64;
    let attributed_closed_qty: i64 = events.iter().map(|e| e.qty).sum();
    let gross_realized_pnl_micros: i64 = events
        .iter()
        .fold(0i64, |acc, e| acc.saturating_add(e.gross_pnl_micros));

    let gross_profit_micros: i64 = events
        .iter()
        .filter(|e| e.gross_pnl_micros > 0)
        .fold(0i64, |acc, e| acc.saturating_add(e.gross_pnl_micros));
    let gross_loss_abs_micros: i64 = events
        .iter()
        .filter(|e| e.gross_pnl_micros < 0)
        .fold(0i64, |acc, e| acc.saturating_add(e.gross_pnl_micros.unsigned_abs() as i64));

    let winning_close_event_count = events.iter().filter(|e| e.gross_pnl_micros > 0).count() as i64;
    let losing_close_event_count = events.iter().filter(|e| e.gross_pnl_micros < 0).count() as i64;
    let flat_close_event_count = events.iter().filter(|e| e.gross_pnl_micros == 0).count() as i64;

    let hit_rate_denominator = winning_close_event_count + losing_close_event_count;
    let hit_rate = if hit_rate_denominator == 0 {
        None
    } else {
        Some(winning_close_event_count as f64 / hit_rate_denominator as f64)
    };

    let gross_expectancy_micros_per_close_event = if attributed_close_event_count == 0 {
        None
    } else {
        Some(gross_realized_pnl_micros as f64 / attributed_close_event_count as f64)
    };

    let average_win_micros = if winning_close_event_count == 0 {
        None
    } else {
        Some(gross_profit_micros as f64 / winning_close_event_count as f64)
    };
    let average_loss_abs_micros = if losing_close_event_count == 0 {
        None
    } else {
        Some(gross_loss_abs_micros as f64 / losing_close_event_count as f64)
    };

    // NEVER infinity/NaN/a fabricated sentinel: `None` when there is no loss
    // to divide by.
    let profit_factor = if gross_loss_abs_micros == 0 {
        None
    } else {
        Some(gross_profit_micros as f64 / gross_loss_abs_micros as f64)
    };

    let event_pnls: Vec<i64> = events.iter().map(|e| e.gross_pnl_micros).collect();
    let max_realized_pnl_drawdown_micros = compute_max_realized_pnl_drawdown(&event_pnls);

    StrategyPerformanceRow {
        strategy_id,
        strategy_semantic_fingerprint,
        attributed_fragment_count,
        attributed_close_event_count,
        attributed_closed_qty,
        gross_realized_pnl_micros,
        gross_profit_micros,
        gross_loss_abs_micros,
        winning_close_event_count,
        losing_close_event_count,
        flat_close_event_count,
        hit_rate,
        gross_expectancy_micros_per_close_event,
        average_win_micros,
        average_loss_abs_micros,
        profit_factor,
        max_realized_pnl_drawdown_micros,
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

    let events_by_strategy = build_attributed_close_events(&view.fragments);
    let rows = events_by_strategy
        .into_iter()
        .map(|((strategy_id, fingerprint), events)| {
            compute_performance_row(strategy_id, fingerprint, &events)
        })
        .collect();

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
    use super::compute_max_realized_pnl_drawdown;

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
}
