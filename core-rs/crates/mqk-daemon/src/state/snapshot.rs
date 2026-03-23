//! Pure reconcile/snapshot helper functions for mqk-daemon.
//!
//! Contains: parse_signed_qty, reconcile_side_from_schema,
//! reconcile_order_status_from_schema,
//! reconcile_local_snapshot_from_runtime_with_sides,
//! oms_execution_status_to_reconcile, outbox_json_symbol, outbox_json_qty,
//! outbox_json_side, broker_event_to_oms_event, broker_event_to_portfolio_fill,
//! oms_state_to_broker_status, synthesize_paper_broker_snapshot,
//! synthesize_broker_snapshot_from_execution, reconcile_broker_snapshot_from_schema,
//! reconcile_unknown_status, reconcile_last_run_at, reconcile_counts,
//! reconcile_status_from_report, reconcile_status_from_stale,
//! preserve_fail_closed_reconcile_status.

use std::collections::BTreeMap;

use chrono::Utc;
use mqk_execution::{
    oms::state_machine::{OmsEvent, OmsOrder, OrderState},
    BrokerEvent,
};
use mqk_portfolio::{apply_entry, LedgerEntry, PortfolioState};
use mqk_reconcile::{ReconcileDiff, SnapshotFreshness, SnapshotWatermark};

use super::initial_reconcile_status;
use super::types::ReconcileStatusSnapshot;

// ---------------------------------------------------------------------------
// Raw outbox JSON field accessors
// ---------------------------------------------------------------------------

pub(crate) fn outbox_json_symbol(json: &serde_json::Value) -> Option<String> {
    json.get("symbol")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn outbox_json_qty(json: &serde_json::Value) -> Option<i64> {
    let raw = json.get("qty").or_else(|| json.get("quantity"))?;
    let n = raw.as_i64()?;
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

pub(crate) fn outbox_json_side(json: &serde_json::Value) -> mqk_reconcile::Side {
    match json.get("side").and_then(|v| v.as_str()) {
        Some(s) if s.eq_ignore_ascii_case("sell") => mqk_reconcile::Side::Sell,
        _ => mqk_reconcile::Side::Buy,
    }
}

// ---------------------------------------------------------------------------
// Reconcile helpers
// ---------------------------------------------------------------------------

pub(crate) fn parse_signed_qty(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(value);
    }

    let (sign, magnitude) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1_i64, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1_i64, rest)
    } else {
        (1_i64, trimmed)
    };

    let (whole, frac) = magnitude.split_once('.')?;
    if frac.chars().any(|c| c != '0') {
        return None;
    }
    let base = whole.parse::<i64>().ok()?;
    Some(sign * base)
}

pub(crate) fn reconcile_side_from_schema(raw: &str) -> mqk_reconcile::Side {
    if raw.eq_ignore_ascii_case("sell") {
        mqk_reconcile::Side::Sell
    } else {
        mqk_reconcile::Side::Buy
    }
}

pub(crate) fn reconcile_order_status_from_schema(raw: &str) -> mqk_reconcile::OrderStatus {
    if raw.eq_ignore_ascii_case("new") {
        mqk_reconcile::OrderStatus::New
    } else if raw.eq_ignore_ascii_case("accepted") {
        mqk_reconcile::OrderStatus::Accepted
    } else if raw.eq_ignore_ascii_case("partially_filled")
        || raw.eq_ignore_ascii_case("partial_fill")
    {
        mqk_reconcile::OrderStatus::PartiallyFilled
    } else if raw.eq_ignore_ascii_case("filled") {
        mqk_reconcile::OrderStatus::Filled
    } else if raw.eq_ignore_ascii_case("canceled") || raw.eq_ignore_ascii_case("cancelled") {
        mqk_reconcile::OrderStatus::Canceled
    } else if raw.eq_ignore_ascii_case("rejected") {
        mqk_reconcile::OrderStatus::Rejected
    } else {
        mqk_reconcile::OrderStatus::Unknown
    }
}

/// DMON-05: build a local reconcile snapshot from an execution snapshot + side cache.
pub(crate) fn reconcile_local_snapshot_from_runtime_with_sides(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
) -> mqk_reconcile::LocalSnapshot {
    let positions = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| (pos.symbol.clone(), pos.net_qty))
        .collect();

    let orders = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side = sides
                .get(&order.order_id)
                .cloned()
                .unwrap_or(mqk_reconcile::Side::Buy);
            let status = oms_execution_status_to_reconcile(&order.status);
            let snap = mqk_reconcile::OrderSnapshot {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side,
                qty: order.total_qty,
                filled_qty: order.filled_qty,
                status,
            };
            (order.order_id.clone(), snap)
        })
        .collect();

    mqk_reconcile::LocalSnapshot { orders, positions }
}

pub(crate) fn oms_execution_status_to_reconcile(status: &str) -> mqk_reconcile::OrderStatus {
    let raw = status.to_ascii_lowercase();
    if raw == "filled" {
        mqk_reconcile::OrderStatus::Filled
    } else if raw == "canceled" || raw == "cancelled" {
        mqk_reconcile::OrderStatus::Canceled
    } else if raw == "rejected" {
        mqk_reconcile::OrderStatus::Rejected
    } else {
        mqk_reconcile::OrderStatus::Unknown
    }
}

pub(crate) fn broker_event_to_oms_event(event: &BrokerEvent) -> OmsEvent {
    match event {
        BrokerEvent::Ack { .. } => OmsEvent::Ack,
        BrokerEvent::PartialFill { delta_qty, .. } => OmsEvent::PartialFill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::Fill { delta_qty, .. } => OmsEvent::Fill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::CancelAck { .. } => OmsEvent::CancelAck,
        BrokerEvent::CancelReject { .. } => OmsEvent::CancelReject,
        BrokerEvent::ReplaceAck { new_total_qty, .. } => OmsEvent::ReplaceAck {
            new_total_qty: *new_total_qty,
        },
        BrokerEvent::ReplaceReject { .. } => OmsEvent::ReplaceReject,
        BrokerEvent::Reject { .. } => OmsEvent::Reject,
    }
}

pub(crate) fn broker_event_to_portfolio_fill(event: &BrokerEvent) -> Option<mqk_portfolio::Fill> {
    match event {
        BrokerEvent::Fill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        }
        | BrokerEvent::PartialFill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            let portfolio_side = match side {
                mqk_execution::types::Side::Buy => mqk_portfolio::Side::Buy,
                mqk_execution::types::Side::Sell => mqk_portfolio::Side::Sell,
            };
            Some(mqk_portfolio::Fill {
                symbol: symbol.clone(),
                side: portfolio_side,
                qty: *delta_qty,
                price_micros: *price_micros,
                fee_micros: *fee_micros,
            })
        }
        _ => None,
    }
}

pub(crate) fn oms_state_to_broker_status(state: &OrderState) -> &'static str {
    match state {
        OrderState::Open => "new",
        OrderState::PartiallyFilled => "partially_filled",
        OrderState::Filled => "filled",
        OrderState::CancelPending => "pending_cancel",
        OrderState::Cancelled => "canceled",
        OrderState::ReplacePending => "pending_replace",
        OrderState::Rejected => "rejected",
    }
}

/// DMON-01: Synthesize a `BrokerSnapshot` from recovered OMS + portfolio truth.
pub(crate) fn synthesize_paper_broker_snapshot(
    oms_orders: &BTreeMap<String, OmsOrder>,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    portfolio: &PortfolioState,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = oms_orders
        .values()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order.order_id.clone(),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: oms_state_to_broker_status(&order.state).to_string(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = portfolio
        .positions
        .iter()
        .filter_map(|(symbol, pos)| {
            let net: i64 = pos.lots.iter().map(|l| l.qty_signed).sum();
            if net == 0 {
                None
            } else {
                Some(mqk_schemas::BrokerPosition {
                    symbol: symbol.clone(),
                    qty: net.to_string(),
                    avg_price: "0".to_string(),
                })
            }
        })
        .collect();

    let cash_whole = portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

/// DMON-05 (tick): Synthesize a paper-broker snapshot from the latest execution
/// snapshot and side cache.
pub(crate) fn synthesize_broker_snapshot_from_execution(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order
                    .broker_order_id
                    .clone()
                    .unwrap_or_else(|| order.order_id.clone()),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: order.status.to_ascii_lowercase(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| mqk_schemas::BrokerPosition {
            symbol: pos.symbol.clone(),
            qty: pos.net_qty.to_string(),
            avg_price: "0".to_string(),
        })
        .collect();

    let cash_whole = snapshot.portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

pub(crate) fn reconcile_broker_snapshot_from_schema(
    snapshot: &mqk_schemas::BrokerSnapshot,
) -> Result<mqk_reconcile::BrokerSnapshot, &'static str> {
    let fetched_at_ms = snapshot.captured_at_utc.timestamp_millis(); // allow: ops-metadata
    if fetched_at_ms <= 0 {
        return Err("broker snapshot timestamp is invalid; refusing ambiguous broker truth");
    }

    let mut positions = BTreeMap::new();
    for position in &snapshot.positions {
        let qty = parse_signed_qty(&position.qty).ok_or(
            "broker snapshot contains non-integer position qty; refusing ambiguous broker truth",
        )?;
        positions.insert(position.symbol.clone(), qty);
    }

    let mut orders = BTreeMap::new();
    for order in &snapshot.orders {
        let qty = parse_signed_qty(&order.qty).ok_or(
            "broker snapshot contains non-integer order qty; refusing ambiguous broker truth",
        )?;
        let order_id = if order.client_order_id.trim().is_empty() {
            order.broker_order_id.clone()
        } else {
            order.client_order_id.clone()
        };
        orders.insert(
            order_id.clone(),
            mqk_reconcile::OrderSnapshot::new(
                order_id,
                order.symbol.clone(),
                reconcile_side_from_schema(&order.side),
                qty,
                0,
                reconcile_order_status_from_schema(&order.status),
            ),
        );
    }

    Ok(mqk_reconcile::BrokerSnapshot {
        orders,
        positions,
        fetched_at_ms,
    })
}

pub(crate) fn reconcile_unknown_status(note: impl Into<String>) -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        note: Some(note.into()),
        ..initial_reconcile_status()
    }
}

pub(crate) fn reconcile_last_run_at(fetched_at_ms: i64) -> Option<String> {
    chrono::DateTime::<Utc>::from_timestamp_millis(fetched_at_ms) // allow: ops-metadata
        .map(|ts| ts.to_rfc3339())
}

pub(crate) fn reconcile_counts(
    report: &mqk_reconcile::ReconcileReport,
) -> (usize, usize, usize, usize) {
    let mut mismatched_positions = 0;
    let mut mismatched_orders = 0;
    let mut mismatched_fills = 0;
    let mut unmatched_broker_events = 0;

    for diff in &report.diffs {
        match diff {
            ReconcileDiff::PositionQtyMismatch { .. } => mismatched_positions += 1,
            ReconcileDiff::OrderMismatch { .. }
            | ReconcileDiff::LocalOrderMissingAtBroker { .. } => mismatched_orders += 1,
            ReconcileDiff::UnknownOrder { .. } => {
                mismatched_orders += 1;
                unmatched_broker_events += 1;
            }
            ReconcileDiff::UnknownBrokerFill { .. } => {
                mismatched_fills += 1;
                unmatched_broker_events += 1;
            }
        }
    }

    (
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
    )
}

pub(crate) fn reconcile_status_from_report(
    report: &mqk_reconcile::ReconcileReport,
    broker: &mqk_reconcile::BrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (mismatched_positions, mismatched_orders, mismatched_fills, unmatched_broker_events) =
        reconcile_counts(report);

    ReconcileStatusSnapshot {
        status: if report.is_clean() {
            "ok".to_string()
        } else {
            "dirty".to_string()
        },
        last_run_at: reconcile_last_run_at(broker.fetched_at_ms),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
        note: if report.is_clean() {
            None
        } else {
            Some("monotonic reconcile detected drift; dispatch remains blocked".to_string())
        },
    }
}

pub(crate) fn reconcile_status_from_stale(
    stale: &mqk_reconcile::StaleBrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (last_run_at, note) = match stale.freshness {
        SnapshotFreshness::Stale {
            watermark_ms,
            got_ms,
        } => (
            reconcile_last_run_at(got_ms),
            format!(
                "stale broker snapshot rejected by reconcile watermark: watermark_ms={watermark_ms} got_ms={got_ms}"
            ),
        ),
        SnapshotFreshness::NoTimestamp => (
            None,
            "broker snapshot has no timestamp; reconcile ordering is ambiguous and remains fail-closed"
                .to_string(),
        ),
        SnapshotFreshness::Fresh => (
            None,
            "reconcile stale-state construction received a fresh snapshot unexpectedly"
                .to_string(),
        ),
    };

    ReconcileStatusSnapshot {
        status: "stale".to_string(),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        last_run_at,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(note),
    }
}

pub(crate) fn preserve_fail_closed_reconcile_status(
    previous: &ReconcileStatusSnapshot,
    note: impl Into<String>,
) -> ReconcileStatusSnapshot {
    let mut preserved = previous.clone();
    preserved.note = Some(note.into());
    preserved
}

/// Recover OMS orders, side cache, and portfolio from durable DB truth.
pub(crate) async fn recover_oms_and_portfolio(
    db: &sqlx::PgPool,
    run_id: uuid::Uuid,
    initial_equity_micros: i64,
) -> Result<
    (
        BTreeMap<String, OmsOrder>,
        BTreeMap<String, mqk_reconcile::Side>,
        PortfolioState,
    ),
    super::types::RuntimeLifecycleError,
> {
    let submitted = mqk_db::outbox_load_submitted_for_run(db, run_id)
        .await
        .map_err(|err| {
            super::types::RuntimeLifecycleError::internal("outbox_load_submitted_for_run", err)
        })?;
    let applied = mqk_db::inbox_load_all_applied_for_run(db, run_id)
        .await
        .map_err(|err| {
            super::types::RuntimeLifecycleError::internal("inbox_load_all_applied_for_run", err)
        })?;

    let mut oms_orders: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut sides: BTreeMap<String, mqk_reconcile::Side> = BTreeMap::new();
    for row in &submitted {
        let Some(symbol) = outbox_json_symbol(&row.order_json) else {
            continue;
        };
        let Some(qty) = outbox_json_qty(&row.order_json) else {
            continue;
        };
        let side = outbox_json_side(&row.order_json);
        let order_id = row.idempotency_key.clone();
        sides.insert(order_id.clone(), side);
        oms_orders.insert(order_id.clone(), OmsOrder::new(&order_id, symbol, qty));
    }

    let mut portfolio = PortfolioState::new(initial_equity_micros);

    for row in &applied {
        let event: BrokerEvent = match serde_json::from_value(row.message_json.clone()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let internal_id = event.internal_order_id().to_string();
        let oms_event = broker_event_to_oms_event(&event);
        if let Some(order) = oms_orders.get_mut(&internal_id) {
            let _ = order.apply(&oms_event, Some(&row.broker_message_id));
        }
        if let Some(fill) = broker_event_to_portfolio_fill(&event) {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
    }

    oms_orders.retain(|_, o| !o.state.is_terminal());
    sides.retain(|order_id, _| oms_orders.contains_key(order_id));

    Ok((oms_orders, sides, portfolio))
}
