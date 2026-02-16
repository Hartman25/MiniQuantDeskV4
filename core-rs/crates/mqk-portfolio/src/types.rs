use std::collections::BTreeMap;

/// BUY or SELL for fills.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// A single executed fill (the accounting atom).
///
/// qty is always positive.
/// price_micros is price per unit in micros (1e-6).
/// fee_micros is absolute cash fee in micros (>= 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
    pub price_micros: i64,
    pub fee_micros: i64,
}

impl Fill {
    pub fn new<S: Into<String>>(
        symbol: S,
        side: Side,
        qty: i64,
        price_micros: i64,
        fee_micros: i64,
    ) -> Self {
        debug_assert!(qty > 0, "Fill.qty must be > 0");
        debug_assert!(price_micros >= 0, "Fill.price_micros must be >= 0");
        debug_assert!(fee_micros >= 0, "Fill.fee_micros must be >= 0");
        Self {
            symbol: symbol.into(),
            side,
            qty,
            price_micros,
            fee_micros,
        }
    }
}

/// A cash-only entry (for fees/dividends/adjustments).
///
/// amount_micros may be positive or negative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CashEntry {
    pub amount_micros: i64,
    pub reason: String,
}

impl CashEntry {
    pub fn new<S: Into<String>>(amount_micros: i64, reason: S) -> Self {
        Self {
            amount_micros,
            reason: reason.into(),
        }
    }
}

/// Ledger entry types. PATCH 06 uses Fill and cash adjustments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerEntry {
    Fill(Fill),
    Cash(CashEntry),
}

/// A FIFO lot. qty_signed carries direction:
/// +qty = long lot, -qty = short lot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lot {
    pub qty_signed: i64,
    pub entry_price_micros: i64,
}

impl Lot {
    pub fn long(qty: i64, entry_price_micros: i64) -> Self {
        debug_assert!(qty > 0);
        Self {
            qty_signed: qty,
            entry_price_micros,
        }
    }

    pub fn short(qty: i64, entry_price_micros: i64) -> Self {
        debug_assert!(qty > 0);
        Self {
            qty_signed: -qty,
            entry_price_micros,
        }
    }

    pub fn is_long(&self) -> bool {
        self.qty_signed > 0
    }

    pub fn is_short(&self) -> bool {
        self.qty_signed < 0
    }

    pub fn abs_qty(&self) -> i64 {
        self.qty_signed.abs()
    }
}

/// Derived position state for a symbol (from ledger).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionState {
    pub symbol: String,
    /// FIFO lots in chronological order.
    pub lots: Vec<Lot>,
}

impl PositionState {
    pub fn new<S: Into<String>>(symbol: S) -> Self {
        Self {
            symbol: symbol.into(),
            lots: Vec::new(),
        }
    }

    /// Signed position quantity (+long, -short, 0 flat).
    pub fn qty_signed(&self) -> i64 {
        self.lots.iter().map(|l| l.qty_signed).sum()
    }

    pub fn is_flat(&self) -> bool {
        self.qty_signed() == 0
    }
}

/// The portfolio state derived from a ledger stream.
///
/// In PATCH 06 we keep both:
/// - `ledger`: source of truth (append-only in practice)
/// - `positions`: derived, maintained incrementally by apply_entry/apply_fill
/// - `cash_micros`: derived cash balance
/// - `realized_pnl_micros`: derived realized PnL (explicit accumulator)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioState {
    pub initial_cash_micros: i64,
    pub cash_micros: i64,
    pub realized_pnl_micros: i64,
    pub ledger: Vec<LedgerEntry>,
    pub positions: BTreeMap<String, PositionState>,
}

impl PortfolioState {
    pub fn new(initial_cash_micros: i64) -> Self {
        Self {
            initial_cash_micros,
            cash_micros: initial_cash_micros,
            realized_pnl_micros: 0,
            ledger: Vec::new(),
            positions: BTreeMap::new(),
        }
    }
}
