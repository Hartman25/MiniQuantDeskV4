use std::collections::BTreeMap;

use crate::types::{CashEntry, Fill, LedgerEntry, Lot, PortfolioState, PositionState, Side};

fn mul_qty_price_micros(qty: i64, price_micros: i64) -> i128 {
    (qty as i128) * (price_micros as i128)
}

fn i128_to_i64_clamp(x: i128) -> i64 {
    if x > i64::MAX as i128 {
        i64::MAX
    } else if x < i64::MIN as i128 {
        i64::MIN
    } else {
        x as i64
    }
}

/// Apply a ledger entry to the portfolio (incremental).
///
/// Deterministic, pure logic, no IO.
/// This function also appends the entry to the portfolio ledger.
pub fn apply_entry(pf: &mut PortfolioState, entry: LedgerEntry) {
    match &entry {
        LedgerEntry::Fill(f) => apply_fill(pf, f),
        LedgerEntry::Cash(c) => apply_cash(pf, c),
    }
    pf.ledger.push(entry);
}

/// Apply a cash entry: just affects cash.
fn apply_cash(pf: &mut PortfolioState, c: &CashEntry) {
    // cash adjustment: positive or negative
    pf.cash_micros = pf.cash_micros.saturating_add(c.amount_micros);
}

/// Apply a fill with FIFO lots.
///
/// Rules:
/// - Fill.qty is positive.
/// - For Buy:
///   - covers short lots FIFO first (realized pnl = (entry_short - buy_price)*covered_qty)
///   - remaining opens long lot
///   - cash -= qty*price + fee
/// - For Sell:
///   - reduces long lots FIFO first (realized pnl = (sell_price - entry_long)*sold_qty)
///   - remaining opens short lot
///   - cash += qty*price - fee
pub fn apply_fill(pf: &mut PortfolioState, f: &Fill) {
    debug_assert!(f.qty > 0);
    debug_assert!(f.price_micros >= 0);
    debug_assert!(f.fee_micros >= 0);

    let sym = f.symbol.clone();
    let pos = pf
        .positions
        .entry(sym.clone())
        .or_insert_with(|| PositionState::new(sym.clone()));

    // cash movement first (deterministic, fee included)
    match f.side {
        Side::Buy => {
            let cost = mul_qty_price_micros(f.qty, f.price_micros);
            let cost_i64 = i128_to_i64_clamp(cost);
            pf.cash_micros = pf.cash_micros.saturating_sub(cost_i64);
            pf.cash_micros = pf.cash_micros.saturating_sub(f.fee_micros);
        }
        Side::Sell => {
            let proceeds = mul_qty_price_micros(f.qty, f.price_micros);
            let proceeds_i64 = i128_to_i64_clamp(proceeds);
            pf.cash_micros = pf.cash_micros.saturating_add(proceeds_i64);
            pf.cash_micros = pf.cash_micros.saturating_sub(f.fee_micros);
        }
    }

    // lot consumption/creation
    match f.side {
        Side::Buy => buy_fifo(pos, &mut pf.realized_pnl_micros, f.qty, f.price_micros),
        Side::Sell => sell_fifo(pos, &mut pf.realized_pnl_micros, f.qty, f.price_micros),
    }

    // if flat, drop the position to keep state minimal/deterministic
    if pos.is_flat() {
        pf.positions.remove(&sym);
    }
}

/// Buy FIFO: covers shorts first, then opens long lot.
fn buy_fifo(pos: &mut PositionState, realized_pnl_micros: &mut i64, mut qty: i64, buy_px: i64) {
    // cover shorts FIFO
    let mut i = 0usize;
    while qty > 0 && i < pos.lots.len() {
        if !pos.lots[i].is_short() {
            i += 1;
            continue;
        }

        let coverable = pos.lots[i].abs_qty().min(qty);
        let entry_px = pos.lots[i].entry_price_micros;

        // realized PnL for short cover: (entry_short - buy_px) * coverable
        let pnl = (entry_px as i128 - buy_px as i128) * (coverable as i128);
        *realized_pnl_micros = realized_pnl_micros.saturating_add(i128_to_i64_clamp(pnl));

        // reduce short lot quantity (remember qty_signed is negative)
        let remaining_abs = pos.lots[i].abs_qty() - coverable;
        if remaining_abs == 0 {
            pos.lots.remove(i); // keep FIFO order; removing current preserves remaining order
        } else {
            pos.lots[i].qty_signed = -(remaining_abs);
            i += 1;
        }

        qty -= coverable;
    }

    // remaining opens new long lot
    if qty > 0 {
        pos.lots.push(Lot::long(qty, buy_px));
    }
}

/// Sell FIFO: reduces longs first, then opens short lot.
fn sell_fifo(pos: &mut PositionState, realized_pnl_micros: &mut i64, mut qty: i64, sell_px: i64) {
    // reduce longs FIFO
    let mut i = 0usize;
    while qty > 0 && i < pos.lots.len() {
        if !pos.lots[i].is_long() {
            i += 1;
            continue;
        }

        let sellable = pos.lots[i].abs_qty().min(qty);
        let entry_px = pos.lots[i].entry_price_micros;

        // realized PnL for long sell: (sell_px - entry_long) * sellable
        let pnl = (sell_px as i128 - entry_px as i128) * (sellable as i128);
        *realized_pnl_micros = realized_pnl_micros.saturating_add(i128_to_i64_clamp(pnl));

        let remaining_abs = pos.lots[i].abs_qty() - sellable;
        if remaining_abs == 0 {
            pos.lots.remove(i);
        } else {
            pos.lots[i].qty_signed = remaining_abs;
            i += 1;
        }

        qty -= sellable;
    }

    // remaining opens new short lot
    if qty > 0 {
        pos.lots.push(Lot::short(qty, sell_px));
    }
}

/// Recompute portfolio state from ledger (truth source), and return a fresh derived state.
///
/// Determinism invariant for PATCH 06:
/// incremental apply_entry must match recompute_from_ledger on the same ledger stream.
pub fn recompute_from_ledger(initial_cash_micros: i64, ledger: &[LedgerEntry]) -> (i64, i64, BTreeMap<String, PositionState>) {
    let mut cash = initial_cash_micros;
    let mut realized = 0i64;
    let mut positions: BTreeMap<String, PositionState> = BTreeMap::new();

    for entry in ledger {
        match entry {
            LedgerEntry::Cash(c) => {
                cash = cash.saturating_add(c.amount_micros);
            }
            LedgerEntry::Fill(f) => {
                // cash move
                match f.side {
                    Side::Buy => {
                        let cost = i128_to_i64_clamp(mul_qty_price_micros(f.qty, f.price_micros));
                        cash = cash.saturating_sub(cost);
                        cash = cash.saturating_sub(f.fee_micros);
                    }
                    Side::Sell => {
                        let proceeds = i128_to_i64_clamp(mul_qty_price_micros(f.qty, f.price_micros));
                        cash = cash.saturating_add(proceeds);
                        cash = cash.saturating_sub(f.fee_micros);
                    }
                }

                // lot logic
                let sym = f.symbol.clone();
                let pos = positions
                    .entry(sym.clone())
                    .or_insert_with(|| PositionState::new(sym.clone()));

                match f.side {
                    Side::Buy => buy_fifo(pos, &mut realized, f.qty, f.price_micros),
                    Side::Sell => sell_fifo(pos, &mut realized, f.qty, f.price_micros),
                }

                if pos.is_flat() {
                    positions.remove(&sym);
                }
            }
        }
    }

    (cash, realized, positions)
}
