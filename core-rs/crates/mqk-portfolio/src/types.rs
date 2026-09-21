use std::collections::BTreeMap;

pub use mqk_schemas::QtyMicros;

/// BUY or SELL for fills.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Whether `Fill.fee_micros` is the broker-confirmed final fee for this
/// fill, or a fill-time placeholder pending later attribution.
///
/// CRYPTO-FEE-ATTRIBUTION-01 (operator decision): a synchronous Alpaca
/// crypto fill carries no fee — Alpaca calculates and posts crypto trading
/// fees once, at day's end, as a separate account activity, never as part
/// of the fill itself. `fee_micros = 0` on such a fill must never be read
/// as "this trade had zero economic cost." Equity is the opposite case:
/// Alpaca genuinely charges no commission, so `fee_micros = 0` there IS the
/// final, confirmed economic truth. This status makes the distinction
/// explicit and inspectable rather than leaving `fee_micros = 0` ambiguous
/// between the two meanings.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FeeAttributionStatus {
    /// `fee_micros` is the final, broker-confirmed fee for this fill (or
    /// this asset class is known to genuinely carry no fee, e.g. equity).
    Confirmed,
    /// `fee_micros` is a placeholder (always `0` today); the real fee is
    /// not yet known and may arrive later via a separate broker account
    /// activity. Never report this fill's cost as proven zero.
    PendingAttribution,
}

/// True when `symbol` is in canonical crypto-pair wire format (contains
/// `/`, e.g. `"BTC/USD"`) rather than a bare equity ticker (e.g. `"AAPL"`).
///
/// This is this system's own canonical crypto-symbol convention (see
/// `mqk-broker-alpaca`'s identical `is_alpaca_crypto_symbol`, which the
/// broker-adapter layer uses for the same distinction), not a broker-wire
/// quirk local to one adapter — safe to use at this broker-agnostic layer
/// to decide [`FeeAttributionStatus`] for a fill whose broker is unknown
/// here.
pub fn symbol_is_crypto_pair_format(symbol: &str) -> bool {
    symbol.contains('/')
}

/// A single executed fill (the accounting atom).
///
/// CUTOVER-1C: `qty` is [`QtyMicros`] (fractional-capable, 1e-6 scale) —
/// previously a plain whole-unit `i64` share/contract count. A whole equity
/// share is `QtyMicros::from_whole_units(1)` (raw `1_000_000`); a fractional
/// Crypto fill (e.g. 0.0001 BTC) is exactly representable and never rounded
/// or truncated on this path.
///
/// qty is always positive.
/// price_micros is price per unit in micros (1e-6).
/// fee_micros is absolute cash fee in micros (>= 0). See
/// [`FeeAttributionStatus`] for whether that value is broker-confirmed or
/// merely a fill-time placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub symbol: String,
    pub side: Side,
    pub qty: QtyMicros,
    pub price_micros: i64,
    pub fee_micros: i64,
    pub fee_attribution: FeeAttributionStatus,
}

impl Fill {
    /// Construct a fill whose `fee_micros` is the final, broker-confirmed
    /// value (`FeeAttributionStatus::Confirmed`). Every pre-existing call
    /// site (equity execution, backtest, synthetic/test fixtures) keeps
    /// this exact meaning unchanged.
    pub fn new<S: Into<String>>(
        symbol: S,
        side: Side,
        qty: QtyMicros,
        price_micros: i64,
        fee_micros: i64,
    ) -> Self {
        Self::new_with_fee_attribution(
            symbol,
            side,
            qty,
            price_micros,
            fee_micros,
            FeeAttributionStatus::Confirmed,
        )
    }

    /// Construct a fill with an explicit [`FeeAttributionStatus`] — used by
    /// the production BrokerEvent-to-Fill conversion path
    /// (`mqk-runtime::orchestrator::apply`) for a crypto fill, whose
    /// `fee_micros = 0` at fill time is a placeholder, not a confirmed
    /// zero-cost claim.
    pub fn new_with_fee_attribution<S: Into<String>>(
        symbol: S,
        side: Side,
        qty: QtyMicros,
        price_micros: i64,
        fee_micros: i64,
        fee_attribution: FeeAttributionStatus,
    ) -> Self {
        debug_assert!(qty.is_positive(), "Fill.qty must be > 0");
        debug_assert!(price_micros >= 0, "Fill.price_micros must be >= 0");
        debug_assert!(fee_micros >= 0, "Fill.fee_micros must be >= 0");
        Self {
            symbol: symbol.into(),
            side,
            qty,
            price_micros,
            fee_micros,
            fee_attribution,
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
///
/// CUTOVER-1C: `qty_signed` is [`QtyMicros`] raw units — see [`Fill`]'s doc
/// for the scale convention. Negation uses `QtyMicros::checked_neg`, which
/// fails closed on `i64::MIN` (never reachable from a real quantity, but
/// never silently wrapped either).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lot {
    pub qty_signed: QtyMicros,
    pub entry_price_micros: i64,
}

impl Lot {
    pub fn long(qty: QtyMicros, entry_price_micros: i64) -> Self {
        debug_assert!(qty.is_positive());
        Self {
            qty_signed: qty,
            entry_price_micros,
        }
    }

    pub fn short(qty: QtyMicros, entry_price_micros: i64) -> Self {
        debug_assert!(qty.is_positive());
        Self {
            qty_signed: qty
                .checked_neg()
                .expect("lot quantity magnitude must be representable as its own negation"),
            entry_price_micros,
        }
    }

    pub fn is_long(&self) -> bool {
        self.qty_signed.is_positive()
    }

    pub fn is_short(&self) -> bool {
        self.qty_signed.is_negative()
    }

    pub fn abs_qty(&self) -> QtyMicros {
        self.qty_signed
            .checked_abs()
            .expect("lot quantity magnitude must be representable as its own absolute value")
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

    /// Signed position quantity (+long, -short, 0 flat). Checked summation:
    /// overflow across lots panics rather than silently wrapping (CUTOVER-1C
    /// #9/#10) -- unreachable for any realistic position size, but never
    /// assumed.
    pub fn qty_signed(&self) -> QtyMicros {
        self.lots.iter().fold(QtyMicros::ZERO, |acc, l| {
            acc.checked_add(l.qty_signed)
                .expect("position quantity overflowed QtyMicros summing lots")
        })
    }

    pub fn is_flat(&self) -> bool {
        self.qty_signed().is_zero()
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
