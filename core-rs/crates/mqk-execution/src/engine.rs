#![forbid(unsafe_code)]

//! Order/execution intent generation from target positions.
//!
//! Deterministic: converts desired target positions into a set of `ExecutionIntent`s
//! to move current positions toward target. No broker calls, no randomness.

use std::collections::BTreeMap;

use crate::types::{ExecutionDecision, ExecutionIntent, Side, TargetPosition};

/// Convert target positions into execution intents.
///
/// `current_qty` is signed quantity per symbol.
/// Targets are signed quantity (+long, -short).
pub fn targets_to_order_intents(
    targets_in: &[TargetPosition],
    current_qty: &BTreeMap<String, i64>,
) -> ExecutionDecision {
    // Build target map (symbol -> target qty).
    let mut targets: BTreeMap<String, i64> = BTreeMap::new();
    for t in targets_in {
        targets.insert(t.symbol.clone(), t.qty);
    }

    // Union of symbols in current + target.
    let mut all: BTreeMap<String, ()> = BTreeMap::new();
    for sym in targets.keys() {
        all.insert(sym.clone(), ());
    }
    for sym in current_qty.keys() {
        all.insert(sym.clone(), ());
    }

    let mut intents: Vec<ExecutionIntent> = Vec::new();

    for (sym, _) in all {
        let cur = *current_qty.get(&sym).unwrap_or(&0);
        let tgt = *targets.get(&sym).unwrap_or(&0);
        let delta = tgt - cur;

        if delta == 0 {
            continue;
        }

        let (side, qty): (Side, i64) = if delta > 0 {
            (Side::Buy, delta)
        } else {
            (Side::Sell, -delta)
        };

        // Deterministic client order id.
        // Must be stable across re-runs for the same inputs.
        // (Symbol has no ":" today in your system; if it ever does, this is still fine.)
        let client_order_id = format!("tgt:{}:{:?}:{}", sym, side, qty);

        intents.push(ExecutionIntent {
            client_order_id,
            symbol: sym,
            side,
            qty,
            limit_price_micros: None,
            stop_price_micros: None,
            time_in_force: "day".to_string(),
        });
    }

    if intents.is_empty() {
        ExecutionDecision::Noop
    } else {
        ExecutionDecision::PlaceOrders(intents)
    }
}
