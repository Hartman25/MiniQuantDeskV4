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

/// Why a lifecycle adjustment cannot be applied. The portfolio is never
/// mutated when one of these is returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleApplyError {
    /// The local ledger does not hold the exact option quantity, on the side the
    /// event names, that the event consumes. Never "whatever exists".
    OptionPositionInsufficient {
        symbol: String,
        required: QtyMicros,
        held_on_side: QtyMicros,
    },
    /// A quantity operation is not representable.
    QuantityArithmetic(&'static str),
    /// The signed cash delta does not fit the cash balance.
    CashOverflow,
    /// The realized P&L of the underlying delivery does not fit.
    RealizedPnlOverflow,
}

impl std::fmt::Display for LifecycleApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OptionPositionInsufficient {
                symbol,
                required,
                held_on_side,
            } => write!(
                f,
                "lifecycle adjustment refused: {symbol} requires {required} on the named side, \
                 local ledger holds {held_on_side}"
            ),
            Self::QuantityArithmetic(why) => {
                write!(
                    f,
                    "lifecycle adjustment refused: quantity arithmetic: {why}"
                )
            }
            Self::CashOverflow => write!(f, "lifecycle adjustment refused: cash overflow"),
            Self::RealizedPnlOverflow => {
                write!(f, "lifecycle adjustment refused: realized P&L overflow")
            }
        }
    }
}

impl std::error::Error for LifecycleApplyError {}

/// Apply a ledger entry to the portfolio (incremental).
///
/// Deterministic, pure logic, no IO.
/// This function also appends the entry to the portfolio ledger.
///
/// A lifecycle adjustment that cannot be applied exactly (see
/// [`LifecycleApplyError`]) is dropped with ZERO mutation -- not applied, not
/// recorded. Callers that must observe the refusal use [`try_apply_entry`].
pub fn apply_entry(pf: &mut PortfolioState, entry: LedgerEntry) {
    let _ = try_apply_entry(pf, entry);
}

/// [`apply_entry`] that reports a refused lifecycle adjustment. On `Err` the
/// portfolio (positions, cash, realized P&L, ledger) is exactly as before.
pub fn try_apply_entry(
    pf: &mut PortfolioState,
    entry: LedgerEntry,
) -> Result<(), LifecycleApplyError> {
    match &entry {
        LedgerEntry::Fill(f) => apply_fill(pf, f),
        LedgerEntry::Cash(c) => apply_cash(pf, c),
        LedgerEntry::LifecycleAdjustment(a) => try_apply_lifecycle_adjustment(pf, a)?,
    }
    pf.ledger.push(entry);
    Ok(())
}

/// Apply one options lifecycle adjustment (exercise/assignment/expiration),
/// all-or-nothing.
///
/// The whole adjustment is checked and computed on a scratch copy first; the
/// live portfolio is replaced only if every step succeeded.
///
/// - Option: removes EXACTLY `|option_qty_delta|` contracts FIFO from the side
///   the delta names (negative: long lots, positive: short lots). Fewer
///   contracts on that side is a refusal -- never a partial flatten. No
///   realized P&L, no cash.
/// - Underlying: opens/reduces lots through the ordinary FIFO at the strike
///   basis, with NO cash movement of its own.
/// - Cash: exactly the provider-signed `cash_delta_micros`, checked.
pub fn try_apply_lifecycle_adjustment(
    pf: &mut PortfolioState,
    adj: &LifecycleAdjustment,
) -> Result<(), LifecycleApplyError> {
    let mut cash = pf.cash_micros;
    let mut realized = pf.realized_pnl_micros;
    let mut positions = pf.positions.clone();
    apply_lifecycle_adjustment_checked(&mut cash, &mut realized, &mut positions, adj)?;
    pf.cash_micros = cash;
    pf.realized_pnl_micros = realized;
    pf.positions = positions;
    Ok(())
}

/// Checked lifecycle application over caller-owned scratch state. Partial
/// mutation of the scratch on `Err` is the caller's to discard.
fn apply_lifecycle_adjustment_checked(
    cash_micros: &mut i64,
    realized_pnl_micros: &mut i64,
    positions: &mut BTreeMap<String, PositionState>,
    adj: &LifecycleAdjustment,
) -> Result<(), LifecycleApplyError> {
    // 1. option position: the exact quantity, on the named side, must exist.
    let required =
        adj.option_qty_delta
            .checked_abs()
            .ok_or(LifecycleApplyError::QuantityArithmetic(
                "option delta magnitude",
            ))?;
    let remove_long = adj.option_qty_delta.is_negative();
    let held_on_side = positions
        .get(&adj.option_symbol)
        .map(|pos| side_qty(pos, remove_long))
        .unwrap_or(Ok(QtyMicros::ZERO))?;
    if held_on_side < required {
        return Err(LifecycleApplyError::OptionPositionInsufficient {
            symbol: adj.option_symbol.clone(),
            required,
            held_on_side,
        });
    }
    let pos =
        positions
            .get_mut(&adj.option_symbol)
            .ok_or(LifecycleApplyError::QuantityArithmetic(
                "option position vanished after the sufficiency check",
            ))?;
    if !remove_lots_exact(pos, remove_long, required)?.is_zero() {
        return Err(LifecycleApplyError::QuantityArithmetic(
            "option lots did not cover the checked quantity",
        ));
    }
    if pos.is_flat() {
        positions.remove(&adj.option_symbol);
    }

    // 2. underlying delivery at the strike basis (lots only).
    if let Some(u) = &adj.underlying {
        let pos = positions
            .entry(u.symbol.clone())
            .or_insert_with(|| PositionState::new(u.symbol.clone()));
        // Realized P&L of the delivery is accumulated from zero; a value at the
        // i64 rails means the FIFO clamped, which is a refusal here.
        let mut delivery_pnl = 0i64;
        if u.qty_delta.is_positive() {
            buy_fifo(pos, &mut delivery_pnl, u.qty_delta, u.basis_price_micros);
        } else if u.qty_delta.is_negative() {
            let qty = u
                .qty_delta
                .checked_abs()
                .ok_or(LifecycleApplyError::QuantityArithmetic(
                    "underlying delta magnitude",
                ))?;
            sell_fifo(pos, &mut delivery_pnl, qty, u.basis_price_micros);
        }
        if delivery_pnl == i64::MAX || delivery_pnl == i64::MIN {
            return Err(LifecycleApplyError::RealizedPnlOverflow);
        }
        *realized_pnl_micros = realized_pnl_micros
            .checked_add(delivery_pnl)
            .ok_or(LifecycleApplyError::RealizedPnlOverflow)?;
        if pos.is_flat() {
            positions.remove(&u.symbol);
        }
    }

    // 3. the provider's own signed cash.
    *cash_micros = cash_micros
        .checked_add(adj.cash_delta_micros)
        .ok_or(LifecycleApplyError::CashOverflow)?;
    Ok(())
}

/// Total magnitude of the lots on one side of a position (checked).
fn side_qty(pos: &PositionState, long: bool) -> Result<QtyMicros, LifecycleApplyError> {
    let mut total = QtyMicros::ZERO;
    for lot in &pos.lots {
        if (long && lot.is_long()) || (!long && lot.is_short()) {
            total = total
                .checked_add(lot.abs_qty())
                .ok_or(LifecycleApplyError::QuantityArithmetic("side quantity sum"))?;
        }
    }
    Ok(total)
}

/// Remove `qty` of lots FIFO from one side; returns what could NOT be removed
/// (zero after a passed sufficiency check).
fn remove_lots_exact(
    pos: &mut PositionState,
    remove_long: bool,
    qty: QtyMicros,
) -> Result<QtyMicros, LifecycleApplyError> {
    let mut remaining = qty;
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
            .ok_or(LifecycleApplyError::QuantityArithmetic("lot remainder"))?;
        remaining = remaining
            .checked_sub(take)
            .ok_or(LifecycleApplyError::QuantityArithmetic(
                "remaining quantity",
            ))?;
        if left.is_zero() {
            pos.lots.remove(i);
        } else {
            pos.lots[i].qty_signed = if remove_long {
                left
            } else {
                left.checked_neg()
                    .ok_or(LifecycleApplyError::QuantityArithmetic(
                        "short lot negation",
                    ))?
            };
            i += 1;
        }
    }
    Ok(remaining)
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
                // The incremental path never records a refused adjustment, so a
                // ledger built by it never contains one; apply on scratch so a
                // foreign stream that does is skipped, not half-applied.
                let (mut c, mut r, mut p) = (cash, realized, positions.clone());
                if apply_lifecycle_adjustment_checked(&mut c, &mut r, &mut p, a).is_ok() {
                    cash = c;
                    realized = r;
                    positions = p;
                }
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
