use std::collections::BTreeMap;

use crate::types::{
    CashEntry, Fill, LedgerEntry, LifecycleAdjustment, Lot, PortfolioState, PositionState,
    QtyMicros, Side,
};

/// `qty_raw` (QtyMicros raw units, 1e-6 scale) * `price_micros` (1e-6 scale),
/// descaled back to a plain micros-scale cash value by dividing out the one
/// extra factor of `QTY_MICROS_SCALE` the quantity operand carries — the
/// same convention `mqk_portfolio::instrument_economics::checked_notional_micros`
/// already uses for its qty*price*multiplier product. Integer division
/// truncates toward zero, losing precision only at sub-micro-dollar
/// magnitudes no real fill can reach (same trade-off documented there).
fn mul_qty_price_micros(qty: QtyMicros, price_micros: i64) -> i128 {
    (qty.raw() as i128) * (price_micros as i128) / (mqk_schemas::QTY_MICROS_SCALE as i128)
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
        LedgerEntry::LifecycleAdjustment(a) => apply_lifecycle_adjustment(pf, a),
    }
    pf.ledger.push(entry);
}

/// Apply one options lifecycle adjustment (exercise/assignment/expiration).
///
/// - Option: removes contracts toward flat FIFO (negative delta removes long
///   lots, positive removes short lots); saturates at flat, never crossing into
///   the opposite side, no realized P&L, no cash.
/// - Underlying: opens/reduces lots through the ordinary FIFO at the strike
///   basis, with NO cash movement of its own.
/// - Cash: exactly the provider-signed `cash_delta_micros`.
pub fn apply_lifecycle_adjustment(pf: &mut PortfolioState, adj: &LifecycleAdjustment) {
    apply_lifecycle_adjustment_core(
        &mut pf.cash_micros,
        &mut pf.realized_pnl_micros,
        &mut pf.positions,
        adj,
    );
}

fn apply_lifecycle_adjustment_core(
    cash_micros: &mut i64,
    realized_pnl_micros: &mut i64,
    positions: &mut BTreeMap<String, PositionState>,
    adj: &LifecycleAdjustment,
) {
    // 1. option position removal (no P&L, no cash).
    if let Some(pos) = positions.get_mut(&adj.option_symbol) {
        remove_lots_toward_flat(pos, adj.option_qty_delta);
        if pos.is_flat() {
            positions.remove(&adj.option_symbol);
        }
    }

    // 2. underlying delivery at the strike basis (lots only).
    if let Some(u) = &adj.underlying {
        let pos = positions
            .entry(u.symbol.clone())
            .or_insert_with(|| PositionState::new(u.symbol.clone()));
        if u.qty_delta.is_positive() {
            buy_fifo(pos, realized_pnl_micros, u.qty_delta, u.basis_price_micros);
        } else if u.qty_delta.is_negative() {
            let qty = u.qty_delta.checked_abs().expect(
                "underlying delta magnitude must be representable as its own absolute value",
            );
            sell_fifo(pos, realized_pnl_micros, qty, u.basis_price_micros);
        }
        if pos.is_flat() {
            positions.remove(&u.symbol);
        }
    }

    // 3. the provider's own signed cash.
    *cash_micros = cash_micros.saturating_add(adj.cash_delta_micros);
}

/// Remove `delta`'s magnitude of lots FIFO from the side `delta` names
/// (negative: long lots, positive: short lots), stopping at flat.
fn remove_lots_toward_flat(pos: &mut PositionState, delta: QtyMicros) {
    let remove_long = delta.is_negative();
    let mut remaining = delta
        .checked_abs()
        .expect("lifecycle delta magnitude must be representable as its own absolute value");
    let mut i = 0usize;
    while remaining.is_positive() && i < pos.lots.len() {
        let matches_side = if remove_long {
            pos.lots[i].is_long()
        } else {
            pos.lots[i].is_short()
        };
        if !matches_side {
            i += 1;
            continue;
        }
        let take = pos.lots[i].abs_qty().min(remaining);
        let left = pos.lots[i]
            .abs_qty()
            .checked_sub(take)
            .expect("take is bounded by abs_qty(), so this subtraction cannot underflow");
        remaining = remaining
            .checked_sub(take)
            .expect("take is bounded by remaining (via .min()), so this cannot underflow");
        if left.is_zero() {
            pos.lots.remove(i);
        } else {
            pos.lots[i].qty_signed = if remove_long {
                left
            } else {
                left.checked_neg()
                    .expect("remaining lot magnitude must be representable as its own negation")
            };
            i += 1;
        }
    }
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
    debug_assert!(f.qty.is_positive());
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
///
/// CUTOVER-1C: `qty` is [`QtyMicros`]; every reduction/comparison uses
/// checked arithmetic. Overflow/underflow here would mean a lot's own
/// magnitude cannot represent its own quantity, which is unreachable for any
/// input that passed [`Lot::long`]/[`Lot::short`]'s construction, but is
/// still never assumed via an unchecked operator.
fn buy_fifo(
    pos: &mut PositionState,
    realized_pnl_micros: &mut i64,
    mut qty: QtyMicros,
    buy_px: i64,
) {
    // cover shorts FIFO
    let mut i = 0usize;
    while qty.is_positive() && i < pos.lots.len() {
        if !pos.lots[i].is_short() {
            i += 1;
            continue;
        }

        let coverable = pos.lots[i].abs_qty().min(qty);
        let entry_px = pos.lots[i].entry_price_micros;

        // realized PnL for short cover: (entry_short - buy_px) * coverable
        let pnl = (entry_px as i128 - buy_px as i128) * (coverable.raw() as i128)
            / (mqk_schemas::QTY_MICROS_SCALE as i128);
        *realized_pnl_micros = realized_pnl_micros.saturating_add(i128_to_i64_clamp(pnl));

        // reduce short lot quantity (remember qty_signed is negative)
        let remaining_abs = pos.lots[i]
            .abs_qty()
            .checked_sub(coverable)
            .expect("coverable is bounded by abs_qty(), so this subtraction cannot underflow");
        if remaining_abs.is_zero() {
            pos.lots.remove(i); // keep FIFO order; removing current preserves remaining order
        } else {
            pos.lots[i].qty_signed = remaining_abs
                .checked_neg()
                .expect("remaining_abs magnitude must be representable as its own negation");
            i += 1;
        }

        qty = qty.checked_sub(coverable).expect(
            "coverable is bounded by qty (via .min()), so this subtraction cannot underflow",
        );
    }

    // remaining opens new long lot
    if qty.is_positive() {
        pos.lots.push(Lot::long(qty, buy_px));
    }
}

/// Sell FIFO: reduces longs first, then opens short lot.
fn sell_fifo(
    pos: &mut PositionState,
    realized_pnl_micros: &mut i64,
    mut qty: QtyMicros,
    sell_px: i64,
) {
    // reduce longs FIFO
    let mut i = 0usize;
    while qty.is_positive() && i < pos.lots.len() {
        if !pos.lots[i].is_long() {
            i += 1;
            continue;
        }

        let sellable = pos.lots[i].abs_qty().min(qty);
        let entry_px = pos.lots[i].entry_price_micros;

        // realized PnL for long sell: (sell_px - entry_long) * sellable
        let pnl = (sell_px as i128 - entry_px as i128) * (sellable.raw() as i128)
            / (mqk_schemas::QTY_MICROS_SCALE as i128);
        *realized_pnl_micros = realized_pnl_micros.saturating_add(i128_to_i64_clamp(pnl));

        let remaining_abs = pos.lots[i]
            .abs_qty()
            .checked_sub(sellable)
            .expect("sellable is bounded by abs_qty(), so this subtraction cannot underflow");
        if remaining_abs.is_zero() {
            pos.lots.remove(i);
        } else {
            pos.lots[i].qty_signed = remaining_abs;
            i += 1;
        }

        qty = qty.checked_sub(sellable).expect(
            "sellable is bounded by qty (via .min()), so this subtraction cannot underflow",
        );
    }

    // remaining opens new short lot
    if qty.is_positive() {
        pos.lots.push(Lot::short(qty, sell_px));
    }
}

/// Recompute portfolio state from ledger (truth source), and return a fresh derived state.
///
/// Determinism invariant for PATCH 06:
/// incremental apply_entry must match recompute_from_ledger on the same ledger stream.
pub fn recompute_from_ledger(
    initial_cash_micros: i64,
    ledger: &[LedgerEntry],
) -> (i64, i64, BTreeMap<String, PositionState>) {
    let mut cash = initial_cash_micros;
    let mut realized = 0i64;
    let mut positions: BTreeMap<String, PositionState> = BTreeMap::new();

    for entry in ledger {
        match entry {
            LedgerEntry::Cash(c) => {
                cash = cash.saturating_add(c.amount_micros);
            }
            LedgerEntry::LifecycleAdjustment(a) => {
                apply_lifecycle_adjustment_core(&mut cash, &mut realized, &mut positions, a);
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
                        let proceeds =
                            i128_to_i64_clamp(mul_qty_price_micros(f.qty, f.price_micros));
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
