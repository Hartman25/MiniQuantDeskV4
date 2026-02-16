use std::collections::{BTreeMap, BTreeSet};

use crate::types::{ExecutionDecision, OrderIntent, Side, StrategyOutput};

use crate::PositionBook;

/// Convert target positions into order intents given current positions.
///
/// Rules (PATCH 05):
/// - Signed quantities: +long, -short
/// - delta = target - current
///   - delta > 0 => BUY delta
///   - delta < 0 => SELL -delta
/// - Deterministic ordering by symbol (lexicographic)
/// - No broker calls, no IO, no timestamps, no randomness
pub fn targets_to_order_intents(current: &PositionBook, output: &StrategyOutput) -> ExecutionDecision {
    // Build a deterministic target map; last write wins if strategy emits duplicates.
    let mut targets: BTreeMap<String, i64> = BTreeMap::new();
    for t in &output.targets {
        targets.insert(t.symbol.clone(), t.target_qty);
    }

    let mut symbols: BTreeSet<String> = BTreeSet::new();
    symbols.extend(current.keys().cloned());
    symbols.extend(targets.keys().cloned());

    let mut intents: Vec<OrderIntent> = Vec::new();

    for sym in symbols {
        let cur = *current.get(&sym).unwrap_or(&0);
        let tgt = *targets.get(&sym).unwrap_or(&0);
        let delta = tgt - cur;

        if delta > 0 {
            intents.push(OrderIntent::new(sym, Side::Buy, delta));
        } else if delta < 0 {
            intents.push(OrderIntent::new(sym, Side::Sell, -delta));
        }
    }

    ExecutionDecision { intents }
}
