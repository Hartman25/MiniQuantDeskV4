//! Fill ordering policy — R3-2
//!
//! Defines the canonical sort order for fills before they are applied to the
//! ledger. Applying fills in canonical order is a mandatory invariant: the
//! same set of fills must always produce the same ledger state regardless of
//! the order in which they arrived from the broker or were replayed from the
//! audit log.
//!
//! # Canonical sort key
//!
//! `(seq_no, symbol, side_ord, qty)` ascending.
//! `side_ord`: `Buy = 0`, `Sell = 1` — buys precede sells on a tied
//! `(seq_no, symbol)` to ensure lots are opened before they are closed.
//!
//! # Usage
//!
//! ```ignore
//! use mqk_portfolio::{TaggedFill, Fill, Side, Ledger, apply_fills_canonical, MICROS_SCALE};
//!
//! let fills = vec![
//!     TaggedFill { seq_no: 2, fill: Fill::new("AAPL", Side::Buy, 5, 100 * MICROS_SCALE, 0) },
//!     TaggedFill { seq_no: 1, fill: Fill::new("AAPL", Side::Buy, 5, 100 * MICROS_SCALE, 0) },
//! ];
//! let mut ledger = Ledger::new(100_000 * MICROS_SCALE);
//! apply_fills_canonical(&mut ledger, fills).unwrap();
//! ```

use crate::{Fill, Ledger, LedgerError, Side};

// ---------------------------------------------------------------------------
// TaggedFill
// ---------------------------------------------------------------------------

/// A fill tagged with its canonical sequence number.
///
/// `seq_no` is the primary ordering key — typically a broker-assigned sequence
/// number or a monotonic microsecond timestamp. Callers assign this when
/// ingesting fills from the broker so that both live processing and replay
/// from the audit log produce identical sort order.
///
/// All `seq_no` values within a batch fed to [`apply_fills_canonical`] must
/// be unique; duplicates will cause [`LedgerError::OutOfOrderSeqNo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaggedFill {
    /// Canonical ordering key. Lower = applied first.
    pub seq_no: u64,
    /// The fill payload.
    pub fill: Fill,
}

// ---------------------------------------------------------------------------
// Canonical sort
// ---------------------------------------------------------------------------

/// Sort `fills` into canonical order **in place**.
///
/// Sort key (all ascending): `(seq_no, symbol, side_ord, qty)`.
///
/// This function is pure, stateless, and deterministic: identical inputs
/// always produce identical outputs.
pub fn sort_fills_canonical(fills: &mut [TaggedFill]) {
    fills.sort_by(|a, b| {
        let seq = a.seq_no.cmp(&b.seq_no);
        if seq != std::cmp::Ordering::Equal {
            return seq;
        }
        let sym = a.fill.symbol.cmp(&b.fill.symbol);
        if sym != std::cmp::Ordering::Equal {
            return sym;
        }
        let side_ord = |s: &Side| -> u8 {
            match s {
                Side::Buy => 0,
                Side::Sell => 1,
            }
        };
        let side = side_ord(&a.fill.side).cmp(&side_ord(&b.fill.side));
        if side != std::cmp::Ordering::Equal {
            return side;
        }
        a.fill.qty.cmp(&b.fill.qty)
    });
}

// ---------------------------------------------------------------------------
// Canonical apply
// ---------------------------------------------------------------------------

/// Sort `fills` into canonical order then apply them to `ledger`.
///
/// Each fill is applied via [`Ledger::append_fill_seq`] using its `seq_no` as
/// the ledger sequence number. Because `sort_fills_canonical` ensures fills are
/// in ascending `seq_no` order, the ledger's monotonicity invariant is
/// preserved when all `seq_no` values in the batch are unique.
///
/// # Errors
///
/// Returns [`LedgerError`] if any fill fails invariant validation, or if two
/// fills share the same `seq_no` (which would violate the ledger's strict
/// monotonicity requirement).
pub fn apply_fills_canonical(
    ledger: &mut Ledger,
    mut fills: Vec<TaggedFill>,
) -> Result<(), LedgerError> {
    sort_fills_canonical(&mut fills);
    for tf in fills {
        ledger.append_fill_seq(tf.fill, tf.seq_no)?;
    }
    Ok(())
}
