#![forbid(unsafe_code)]

//! Order/execution intent generation from target positions.
//!
//! Deterministic: converts desired target positions into a set of `ExecutionIntent`s
//! to move current positions toward target. No broker calls, no randomness.

use std::collections::BTreeMap;

use mqk_schemas::QtyMicros;

use crate::types::{ExecutionDecision, ExecutionIntent, Side, TargetPosition};

/// Convert target positions into execution intents.
///
/// `current_qty` is signed quantity per symbol.
/// Targets are signed quantity (+long, -short).
///
/// All delta arithmetic is checked; an unrepresentable delta fails closed as
/// `HaltAndDisarm` rather than wrapping or truncating.
pub fn targets_to_order_intents(
    targets_in: &[TargetPosition],
    current_qty: &BTreeMap<String, QtyMicros>,
) -> ExecutionDecision {
    // Build target map (symbol -> target qty).
    let mut targets: BTreeMap<String, QtyMicros> = BTreeMap::new();
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
        let cur = current_qty.get(&sym).copied().unwrap_or(QtyMicros::ZERO);
        let tgt = targets.get(&sym).copied().unwrap_or(QtyMicros::ZERO);
        let Some(delta) = tgt.checked_sub(cur) else {
            return overflow_halt(&sym, "delta subtraction");
        };

        if delta.is_zero() {
            continue;
        }

        let (side, qty): (Side, QtyMicros) = if delta.is_positive() {
            (Side::Buy, delta)
        } else {
            match delta.checked_neg() {
                Some(q) => (Side::Sell, q),
                None => return overflow_halt(&sym, "delta negation"),
            }
        };

        // Deterministic client order id.
        // Must be stable across re-runs for the same inputs. `QtyMicros`
        // Display renders whole quantities as bare integers (`10`), so
        // whole-unit ids are byte-identical to the prior i64 form.
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

fn overflow_halt(symbol: &str, what: &str) -> ExecutionDecision {
    ExecutionDecision::HaltAndDisarm {
        reason: format!("quantity overflow in target->delta conversion ({what}) for {symbol}"),
    }
}
