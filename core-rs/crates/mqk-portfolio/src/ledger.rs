//! Ledger abstraction — makes FIFO and PnL rules explicit and isolated.
//!
//! # Purpose
//! [`accounting`](crate::accounting) contains the raw FIFO/PnL mechanics.
//! This module wraps them behind a typed, append-only [`Ledger`] façade that:
//!
//! - Enforces ledger invariants on every append (no zero/negative qty, price,
//!   or fee; symbol must be non-empty).
//! - Exposes only the minimal write surface (`append_fill`, `append_cash`).
//! - Provides read-only snapshot views of cash, positions, and PnL.
//! - Keeps the FIFO and PnL rules in `accounting.rs` while this module owns
//!   the invariant-checking boundary — matching the architectural goal of
//!   "explicit/isolated" stated in the gap-fill spec.
//!
//! # Usage
//! ```ignore
//! let mut ledger = Ledger::new(100_000 * MICROS_SCALE);
//! ledger.append_fill(Fill::new("AAPL", Side::Buy, 10, 150_000_000, 0))?;
//! let snap = ledger.snapshot();
//! println!("equity ≈ {}", snap.cash_micros);
//! ```
//!
//! # Determinism
//! `Ledger` is deterministic and pure — no IO, no time, no randomness.
//! Two `Ledger` instances fed the same sequence of entries will always produce
//! identical state.

use std::collections::BTreeMap;

use crate::{
    accounting::{apply_fill, recompute_from_ledger},
    types::{CashEntry, Fill, LedgerEntry, PortfolioState, PositionState},
    MarkMap,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// All invariant violations that `Ledger` can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// `Fill.qty` must be strictly positive.
    NonPositiveQty { qty: i64 },
    /// `Fill.price_micros` must be strictly positive.
    NonPositivePrice { price_micros: i64 },
    /// `Fill.fee_micros` must be non-negative.
    NegativeFee { fee_micros: i64 },
    /// `Fill.symbol` (or cash entry reason) must be non-empty.
    EmptySymbol,
    /// The sequence number supplied is not strictly greater than the last.
    OutOfOrderSeqNo { supplied: u64, last: u64 },
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveQty { qty } => {
                write!(f, "ledger invariant: qty must be > 0, got {qty}")
            }
            Self::NonPositivePrice { price_micros } => {
                write!(
                    f,
                    "ledger invariant: price_micros must be > 0, got {price_micros}"
                )
            }
            Self::NegativeFee { fee_micros } => {
                write!(
                    f,
                    "ledger invariant: fee_micros must be >= 0, got {fee_micros}"
                )
            }
            Self::EmptySymbol => write!(f, "ledger invariant: symbol must not be empty"),
            Self::OutOfOrderSeqNo { supplied, last } => write!(
                f,
                "ledger invariant: seq_no {supplied} is not > last {last}"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

// ---------------------------------------------------------------------------
// Snapshot (read-only view)
// ---------------------------------------------------------------------------

/// A point-in-time read-only view of the ledger's derived state.
///
/// Cloned on every call to [`Ledger::snapshot`] — cheap because positions
/// are a `BTreeMap` clone (bounded by open symbol count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerSnapshot {
    /// Current cash balance in micros.
    pub cash_micros: i64,
    /// Accumulated realized PnL in micros.
    pub realized_pnl_micros: i64,
    /// Open positions keyed by symbol.
    pub positions: BTreeMap<String, PositionState>,
    /// Total number of entries appended (fills + cash).
    pub entry_count: usize,
    /// The last sequence number seen (0 if none supplied).
    pub last_seq_no: u64,
}

impl LedgerSnapshot {
    /// Signed net quantity for a symbol (0 if not held).
    pub fn qty_signed(&self, symbol: &str) -> i64 {
        self.positions
            .get(symbol)
            .map(|p| p.qty_signed())
            .unwrap_or(0)
    }

    /// Whether the portfolio is flat (no open positions).
    pub fn is_flat(&self) -> bool {
        self.positions.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// Append-only ledger façade with invariant enforcement.
///
/// Internally delegates all FIFO/PnL arithmetic to [`accounting`](crate::accounting).
/// The `Ledger` struct only owns the append boundary and the portfolio state.
#[derive(Clone, Debug)]
pub struct Ledger {
    state: PortfolioState,
    last_seq_no: u64,
}

impl Ledger {
    /// Create a new ledger with the given initial cash balance.
    ///
    /// # Panics
    /// Does not panic; negative initial cash is permitted (represents an
    /// overdrawn account — the caller decides policy).
    pub fn new(initial_cash_micros: i64) -> Self {
        Self {
            state: PortfolioState::new(initial_cash_micros),
            last_seq_no: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Write surface
    // -----------------------------------------------------------------------

    /// Append a fill entry, enforcing all invariants.
    ///
    /// # Errors
    /// Returns [`LedgerError`] if any invariant is violated.  The ledger is
    /// **not** mutated on error.
    pub fn append_fill(&mut self, fill: Fill) -> Result<(), LedgerError> {
        Self::validate_fill(&fill)?;
        apply_fill(&mut self.state, &fill);
        self.state.ledger.push(LedgerEntry::Fill(fill));
        Ok(())
    }

    /// Append a fill with an explicit monotonic sequence number.
    ///
    /// `seq_no` must be strictly greater than the last recorded sequence
    /// number, enforcing ordering invariants for callers that tag entries.
    pub fn append_fill_seq(&mut self, fill: Fill, seq_no: u64) -> Result<(), LedgerError> {
        if seq_no <= self.last_seq_no {
            return Err(LedgerError::OutOfOrderSeqNo {
                supplied: seq_no,
                last: self.last_seq_no,
            });
        }
        Self::validate_fill(&fill)?;
        apply_fill(&mut self.state, &fill);
        self.state.ledger.push(LedgerEntry::Fill(fill));
        self.last_seq_no = seq_no;
        Ok(())
    }

    /// Append a cash adjustment entry (positive = credit, negative = debit).
    ///
    /// Reason must be non-empty; amount may be any signed value.
    pub fn append_cash(
        &mut self,
        amount_micros: i64,
        reason: impl Into<String>,
    ) -> Result<(), LedgerError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(LedgerError::EmptySymbol);
        }
        let entry = CashEntry::new(amount_micros, reason);
        self.state.cash_micros = self.state.cash_micros.saturating_add(amount_micros);
        self.state.ledger.push(LedgerEntry::Cash(entry));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Read surface
    // -----------------------------------------------------------------------

    /// Return a cloned snapshot of the current ledger state.
    pub fn snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot {
            cash_micros: self.state.cash_micros,
            realized_pnl_micros: self.state.realized_pnl_micros,
            positions: self.state.positions.clone(),
            entry_count: self.state.ledger.len(),
            last_seq_no: self.last_seq_no,
        }
    }

    /// Current cash balance in micros.
    pub fn cash_micros(&self) -> i64 {
        self.state.cash_micros
    }

    /// Accumulated realized PnL in micros.
    pub fn realized_pnl_micros(&self) -> i64 {
        self.state.realized_pnl_micros
    }

    /// Number of entries in the ledger (fills + cash).
    pub fn entry_count(&self) -> usize {
        self.state.ledger.len()
    }

    /// Signed net quantity for a symbol (0 if flat / not held).
    pub fn qty_signed(&self, symbol: &str) -> i64 {
        self.state
            .positions
            .get(symbol)
            .map(|p| p.qty_signed())
            .unwrap_or(0)
    }

    /// `true` if no open positions exist.
    pub fn is_flat(&self) -> bool {
        self.state.positions.is_empty()
    }

    /// Recompute state from the stored ledger entries and verify it matches
    /// the running incremental state.  Returns `true` if consistent.
    ///
    /// This is an **integrity check** — expensive (O(n) replay) — for use in
    /// tests, startup verification, or audit flows only.
    pub fn verify_integrity(&self) -> bool {
        let (cash, realized, positions) =
            recompute_from_ledger(self.state.initial_cash_micros, &self.state.ledger);
        cash == self.state.cash_micros
            && realized == self.state.realized_pnl_micros
            && positions == self.state.positions
    }

    /// Compute mark-to-market equity: `cash + Σ(qty × mark)`.
    ///
    /// Requires a caller-supplied mark map; the ledger itself is mark-free.
    pub fn equity_micros(&self, marks: &MarkMap) -> i64 {
        crate::metrics::compute_equity_micros(self.state.cash_micros, &self.state.positions, marks)
    }

    /// Compute unrealized PnL from FIFO lots and marks.
    pub fn unrealized_pnl_micros(&self, marks: &MarkMap) -> i64 {
        crate::metrics::compute_unrealized_pnl_micros(&self.state.positions, marks)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn validate_fill(fill: &Fill) -> Result<(), LedgerError> {
        if fill.symbol.trim().is_empty() {
            return Err(LedgerError::EmptySymbol);
        }
        if fill.qty <= 0 {
            return Err(LedgerError::NonPositiveQty { qty: fill.qty });
        }
        if fill.price_micros <= 0 {
            return Err(LedgerError::NonPositivePrice {
                price_micros: fill.price_micros,
            });
        }
        if fill.fee_micros < 0 {
            return Err(LedgerError::NegativeFee {
                fee_micros: fill.fee_micros,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{marks, Side, MICROS_SCALE};

    const M: i64 = MICROS_SCALE;

    fn fill(symbol: &str, side: Side, qty: i64, price: i64, fee: i64) -> Fill {
        Fill::new(symbol, side, qty, price * M, fee * M)
    }

    // Helper: construct a Fill bypassing Fill::new()'s debug_assert! guards,
    // so we can hand malformed values to Ledger::validate_fill for testing.
    fn bad_fill(symbol: &str, side: Side, qty: i64, price_micros: i64, fee_micros: i64) -> Fill {
        Fill {
            symbol: symbol.to_string(),
            side,
            qty,
            price_micros,
            fee_micros,
        }
    }

    // --- Invariant enforcement ---

    #[test]
    fn rejects_zero_qty() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(bad_fill("AAPL", Side::Buy, 0, 100 * M, 0));
        assert_eq!(err, Err(LedgerError::NonPositiveQty { qty: 0 }));
        assert_eq!(l.entry_count(), 0); // ledger not mutated
    }

    #[test]
    fn rejects_negative_qty() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(bad_fill("AAPL", Side::Buy, -1, 100 * M, 0));
        assert_eq!(err, Err(LedgerError::NonPositiveQty { qty: -1 }));
    }

    #[test]
    fn rejects_zero_price() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(bad_fill("AAPL", Side::Buy, 10, 0, 0));
        assert_eq!(err, Err(LedgerError::NonPositivePrice { price_micros: 0 }));
    }

    #[test]
    fn rejects_negative_price() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(bad_fill("AAPL", Side::Buy, 10, -1, 0));
        assert_eq!(err, Err(LedgerError::NonPositivePrice { price_micros: -1 }));
    }

    #[test]
    fn rejects_negative_fee() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(bad_fill("AAPL", Side::Buy, 10, 100 * M, -1));
        assert_eq!(err, Err(LedgerError::NegativeFee { fee_micros: -1 }));
    }

    #[test]
    fn rejects_empty_symbol() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(Fill::new("", Side::Buy, 10, 100 * M, 0));
        assert_eq!(err, Err(LedgerError::EmptySymbol));
    }

    #[test]
    fn rejects_whitespace_symbol() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_fill(Fill::new("  ", Side::Buy, 10, 100 * M, 0));
        assert_eq!(err, Err(LedgerError::EmptySymbol));
    }

    #[test]
    fn rejects_empty_cash_reason() {
        let mut l = Ledger::new(100_000 * M);
        let err = l.append_cash(1000, "");
        assert_eq!(err, Err(LedgerError::EmptySymbol));
    }

    // --- Sequence number enforcement ---

    #[test]
    fn seq_no_must_be_strictly_increasing() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill_seq(fill("AAPL", Side::Buy, 1, 100, 0), 5)
            .unwrap();
        let err = l.append_fill_seq(fill("AAPL", Side::Buy, 1, 100, 0), 5);
        assert_eq!(
            err,
            Err(LedgerError::OutOfOrderSeqNo {
                supplied: 5,
                last: 5
            })
        );
    }

    #[test]
    fn seq_no_advances_correctly() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill_seq(fill("AAPL", Side::Buy, 1, 100, 0), 1)
            .unwrap();
        l.append_fill_seq(fill("AAPL", Side::Buy, 1, 100, 0), 2)
            .unwrap();
        assert_eq!(l.snapshot().last_seq_no, 2);
    }

    // --- FIFO PnL correctness via Ledger ---

    #[test]
    fn buy_then_sell_realized_pnl() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill(fill("TSLA", Side::Buy, 10, 200, 0)).unwrap();
        l.append_fill(fill("TSLA", Side::Sell, 10, 210, 0)).unwrap();

        // realized = (210-200)*10 = $100
        assert_eq!(l.realized_pnl_micros(), 100 * M);
        assert!(l.is_flat());
    }

    #[test]
    fn partial_sell_leaves_open_position() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill(fill("MSFT", Side::Buy, 20, 300, 0)).unwrap();
        l.append_fill(fill("MSFT", Side::Sell, 5, 310, 0)).unwrap();

        assert_eq!(l.qty_signed("MSFT"), 15);
        // realized = (310-300)*5 = $50
        assert_eq!(l.realized_pnl_micros(), 50 * M);
    }

    #[test]
    fn fees_reduce_cash() {
        let mut l = Ledger::new(100_000 * M);
        // Buy 10 @ $100 with $1 fee (= 1_000_000 micros)
        l.append_fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, M))
            .unwrap();

        // cash = 100_000 - 10*100 - 1 = 98_999
        assert_eq!(l.cash_micros(), 98_999 * M);
    }

    // --- Cash entries ---

    #[test]
    fn cash_credit_increases_balance() {
        let mut l = Ledger::new(50_000 * M);
        l.append_cash(5_000 * M, "dividend").unwrap();
        assert_eq!(l.cash_micros(), 55_000 * M);
        assert_eq!(l.entry_count(), 1);
    }

    #[test]
    fn cash_debit_decreases_balance() {
        let mut l = Ledger::new(50_000 * M);
        l.append_cash(-1_000 * M, "borrow_cost").unwrap();
        assert_eq!(l.cash_micros(), 49_000 * M);
    }

    // --- Snapshot ---

    #[test]
    fn snapshot_reflects_current_state() {
        let mut l = Ledger::new(10_000 * M);
        l.append_fill(fill("AAPL", Side::Buy, 5, 100, 0)).unwrap();

        let snap = l.snapshot();
        assert_eq!(snap.cash_micros, 10_000 * M - 5 * 100 * M);
        assert_eq!(snap.entry_count, 1);
        assert_eq!(snap.qty_signed("AAPL"), 5);
        assert!(!snap.is_flat());
    }

    // --- Mark-to-market helpers ---

    #[test]
    fn equity_micros_includes_unrealized() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill(fill("AAPL", Side::Buy, 10, 100, 0)).unwrap();

        // cash = 99_000, position = 10 @ $100, mark @ $110
        let mk = marks([("AAPL", 110 * M)]);
        // equity = 99_000 + 10*110 = 100_100
        assert_eq!(l.equity_micros(&mk), 100_100 * M);
    }

    #[test]
    fn unrealized_pnl_long_position() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill(fill("AAPL", Side::Buy, 10, 100, 0)).unwrap();

        let mk = marks([("AAPL", 115 * M)]);
        // unrealized = (115-100)*10 = $150
        assert_eq!(l.unrealized_pnl_micros(&mk), 150 * M);
    }

    // --- Integrity verification ---

    #[test]
    fn verify_integrity_passes_after_normal_operations() {
        let mut l = Ledger::new(100_000 * M);
        l.append_fill(fill("AAPL", Side::Buy, 10, 100, 0)).unwrap();
        l.append_fill(fill("AAPL", Side::Sell, 5, 110, 0)).unwrap();
        l.append_cash(500 * M, "dividend").unwrap();

        assert!(l.verify_integrity());
    }

    #[test]
    fn fresh_ledger_is_flat_and_consistent() {
        let l = Ledger::new(50_000 * M);
        assert!(l.is_flat());
        assert_eq!(l.entry_count(), 0);
        assert_eq!(l.cash_micros(), 50_000 * M);
        assert!(l.verify_integrity());
    }

    // --- LedgerSnapshot helpers ---

    #[test]
    fn snapshot_qty_signed_zero_for_unknown_symbol() {
        let l = Ledger::new(1_000 * M);
        let snap = l.snapshot();
        assert_eq!(snap.qty_signed("UNKNOWN"), 0);
    }
}
