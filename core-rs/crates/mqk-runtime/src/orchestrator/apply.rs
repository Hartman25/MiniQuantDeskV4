//! Broker-event apply path: OMS transitions, portfolio mutations,
//! canonical apply queue construction, and capital invariant checks.
//!
//! All functions here are pure (no DB side effects).  The one exception is
//! `remove_broker_mapping_from_memory` which mutates an in-memory map.
//!
//! # Exports
//!
//! - `broker_event_to_oms_event` — map a `BrokerEvent` to its `OmsEvent`.
//! - `broker_event_to_fill` — extract a portfolio `Fill` from a fill event.
//! - `ApplyQueueEntry` — type alias for the canonical apply queue element.
//! - `build_canonical_apply_queue` — sort and validate unapplied inbox rows.
//! - `AppliedBrokerEventOutcome` — result of `apply_broker_event_step`.
//! - `remove_broker_mapping_from_memory` — deregister a terminal order.
//! - `apply_broker_event_step` — apply OMS transition and return fill outcome.
//! - `apply_fill_step` — inner apply logic (Section C invariant enforcement).
//! - `check_capital_invariants` — verify incremental portfolio vs full recompute.

use anyhow::anyhow;
use mqk_db::InboxRow;
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{BrokerEvent, BrokerOrderMap};
use mqk_portfolio::{recompute_from_ledger, Fill, PortfolioState};
use sqlx::types::chrono;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// OMS event mapping
// ---------------------------------------------------------------------------

/// Map a `BrokerEvent` to the corresponding `OmsEvent`.
pub(super) fn broker_event_to_oms_event(event: &BrokerEvent) -> OmsEvent {
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

// ---------------------------------------------------------------------------
// Fill extraction
// ---------------------------------------------------------------------------

/// Extract a portfolio `Fill` from a `BrokerEvent`, if the event carries fill data.
///
/// Returns `None` for non-fill events (Ack, CancelAck, etc.).
/// Degenerate fills with `delta_qty <= 0` are rejected.
pub(super) fn broker_event_to_fill(event: &BrokerEvent) -> Option<Fill> {
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
            if *delta_qty <= 0 {
                return None;
            }
            let portfolio_side = match side {
                mqk_execution::Side::Buy => mqk_portfolio::Side::Buy,
                mqk_execution::Side::Sell => mqk_portfolio::Side::Sell,
            };
            Some(Fill::new(
                symbol.clone(),
                portfolio_side,
                *delta_qty,
                *price_micros,
                *fee_micros,
            ))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Canonical apply queue
// ---------------------------------------------------------------------------

/// Type alias for the canonical apply queue element.
///
/// `(inbox_id, broker_message_id, event, fill_received_at_utc)`
pub(super) type ApplyQueueEntry = (i64, String, BrokerEvent, chrono::DateTime<chrono::Utc>);

pub(super) fn build_canonical_apply_queue(
    unapplied: Vec<InboxRow>,
) -> anyhow::Result<Vec<ApplyQueueEntry>> {
    let mut apply_queue: Vec<ApplyQueueEntry> = Vec::with_capacity(unapplied.len());
    for row in unapplied {
        let inbox_id = row.inbox_id;
        let msg_id = row.broker_message_id;
        let received_at = row.received_at_utc;
        let event: BrokerEvent = serde_json::from_value(row.message_json)?;
        apply_queue.push((inbox_id, msg_id, event, received_at));
    }
    apply_queue.sort_by_key(|(inbox_id, _, _, _)| *inbox_id);

    for pair in apply_queue.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(anyhow!(
                "AMBIGUOUS_CANONICAL_ORDER: duplicate inbox_id {} in apply queue",
                pair[0].0
            ));
        }
    }
    Ok(apply_queue)
}

// ---------------------------------------------------------------------------
// Apply step
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(super) struct AppliedBrokerEventOutcome {
    pub(super) fill: Option<Fill>,
    pub(super) terminal_apply_succeeded: bool,
}

pub(super) fn remove_broker_mapping_from_memory(order_map: &mut BrokerOrderMap, internal_id: &str) {
    order_map.deregister(internal_id);
}

/// Apply OMS transition and return the portfolio fill outcome (if any).
///
/// Wraps `apply_fill_step` and additionally tracks whether this tick crossed
/// a terminal OMS state boundary so that the caller can clean up the
/// broker-order mapping.
pub(super) fn apply_broker_event_step(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    internal_id: &str,
    event: &BrokerEvent,
    msg_id: &str,
) -> anyhow::Result<AppliedBrokerEventOutcome> {
    let pre_state = oms_orders.get(internal_id).map(|order| order.state.clone());
    let fill = apply_fill_step(oms_orders, internal_id, event, msg_id)?;
    let terminal_apply_succeeded = match (pre_state.as_ref(), oms_orders.get(internal_id)) {
        (Some(pre_state), Some(order)) => !pre_state.is_terminal() && order.state.is_terminal(),
        _ => false,
    };

    Ok(AppliedBrokerEventOutcome {
        fill,
        terminal_apply_succeeded,
    })
}

/// Section C - Restart / Unknown-Order Fill Safety.
///
/// Apply OMS transition and return the portfolio fill to apply (if any).
///
/// # Invariants enforced
///
/// - Fill events (`PartialFill`, `Fill`) **must** have a corresponding OMS
///   order in memory. If none is found, `Err` is returned immediately -
///   the caller must halt and disarm before propagating.
///
/// - Non-fill events (Ack, CancelAck, etc.) for unknown orders are silently
///   skipped. They carry no portfolio effect and can arrive after a crash
///   before the in-memory order map has been rebuilt.
///
/// - Duplicate fill replays are detected by comparing `order.filled_qty`
///   before and after `apply()`. If `filled_qty` did not advance on a fill
///   event, the OMS applied a silent no-op (duplicate `event_id` or late fill
///   on a terminal order). `Ok(None)` is returned to prevent a double
///   portfolio mutation.
///
/// The caller is responsible for halting and disarming on `Err`.
pub(super) fn apply_fill_step(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    internal_id: &str,
    event: &BrokerEvent,
    msg_id: &str,
) -> anyhow::Result<Option<Fill>> {
    let is_fill = matches!(
        event,
        BrokerEvent::PartialFill { .. } | BrokerEvent::Fill { .. }
    );
    let oms_event = broker_event_to_oms_event(event);
    match oms_orders.get_mut(internal_id) {
        Some(order) => {
            let pre_qty = order.filled_qty;
            let economic_event_id = event.broker_fill_id().unwrap_or(msg_id);
            order
                .apply(&oms_event, Some(economic_event_id))
                .map_err(|e| anyhow!("OMS transition error for '{}': {}", internal_id, e))?;
            // No-op detection: if this is a fill event and filled_qty has not
            // advanced, OMS applied a silent no-op (duplicate event_id or fill
            // arriving after the order reached a terminal state).  Skip the
            // portfolio mutation to prevent double-counting.
            if is_fill && order.filled_qty == pre_qty {
                return Ok(None);
            }
        }
        None if is_fill => {
            // Section C invariant: fill events must not reach portfolio without
            // a proven OMS order context in memory.  Fail closed.
            return Err(anyhow!(
                "UNKNOWN_ORDER_FILL: broker_message_id='{}' internal_order_id='{}' \
                 - fill event has no OMS order context in memory; \
                 refusing portfolio mutation (Section C)",
                msg_id,
                internal_id
            ));
        }
        None => {
            // Non-fill event for unknown order - silently skipped.
            // No OMS transition, no portfolio effect.
        }
    }
    Ok(broker_event_to_fill(event))
}

// ---------------------------------------------------------------------------
// Capital invariant check
// ---------------------------------------------------------------------------

/// Assert that the incremental portfolio state is consistent with a full
/// recompute from the ledger.
///
/// Checks:
/// - `cash_micros` matches ledger recompute.
/// - `realized_pnl_micros` matches ledger recompute.
/// - `positions` map matches ledger recompute.
///
/// A mismatch indicates a bug in the incremental apply path and must be
/// treated as a halt condition by the caller.
pub(super) fn check_capital_invariants(portfolio: &PortfolioState) -> anyhow::Result<()> {
    let (recomputed_cash, recomputed_pnl, recomputed_positions) =
        recompute_from_ledger(portfolio.initial_cash_micros, &portfolio.ledger);
    if recomputed_cash != portfolio.cash_micros {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: cash_micros mismatch: recomputed={} state={}",
            recomputed_cash,
            portfolio.cash_micros
        ));
    }
    if recomputed_pnl != portfolio.realized_pnl_micros {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: realized_pnl_micros mismatch: recomputed={} state={}",
            recomputed_pnl,
            portfolio.realized_pnl_micros
        ));
    }
    if recomputed_positions != portfolio.positions {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: positions map mismatch between ledger recompute and state"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Test-only helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(super) fn event_kind_rank(event: &BrokerEvent) -> u8 {
    match event {
        BrokerEvent::Ack { .. } => 0,
        BrokerEvent::PartialFill { .. } => 1,
        BrokerEvent::Fill { .. } => 2,
        BrokerEvent::CancelAck { .. } => 3,
        BrokerEvent::CancelReject { .. } => 4,
        BrokerEvent::ReplaceAck { .. } => 5,
        BrokerEvent::ReplaceReject { .. } => 6,
        BrokerEvent::Reject { .. } => 7,
    }
}
