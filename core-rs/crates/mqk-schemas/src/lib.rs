use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub event_id: Uuid,
    pub run_id: Uuid,
    pub engine_id: String,
    pub ts_utc: DateTime<Utc>,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub topic: String,
    pub event_type: String,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    pub ts_close_utc: DateTime<Utc>,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerOrder {
    pub broker_order_id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub r#type: String,
    pub status: String,
    pub qty: String,
    /// Cumulative filled quantity as reported by the broker, verbatim
    /// (F1-RECONCILE-FILLED-QTY-WIRING-01). Never fabricated — a snapshot
    /// producer that cannot represent the actual broker-reported value must
    /// fail closed rather than default this to `"0"`.
    pub filled_qty: String,
    pub limit_price: Option<String>,
    pub stop_price: Option<String>,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerFill {
    pub broker_fill_id: String,
    pub broker_order_id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: String,
    pub price: String,
    pub fee: String,
    pub ts_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerPosition {
    pub symbol: String,
    pub qty: String,
    pub avg_price: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerAccount {
    pub equity: String,
    pub cash: String,
    pub currency: String,
    /// LIVE-ACCOUNT-TRUTH-01: real broker-reported buying power (margin-aware;
    /// may exceed `cash` for a margin account). `None` when the broker
    /// snapshot did not carry this field -- never aliased to `cash`.
    #[serde(default)]
    pub buying_power: Option<String>,
    /// LIVE-ACCOUNT-TRUTH-01: real broker-reported day-trading buying power.
    /// Same `None`-means-unavailable contract as `buying_power`.
    #[serde(default)]
    pub daytrading_buying_power: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerSnapshot {
    pub captured_at_utc: DateTime<Utc>,
    pub account: BrokerAccount,
    pub orders: Vec<BrokerOrder>,
    pub fills: Vec<BrokerFill>,
    pub positions: Vec<BrokerPosition>,
}

// ---------------------------------------------------------------------------
// Multi-asset primitives (forward-compatible)
// ---------------------------------------------------------------------------

/// Supported asset classes.
///
/// This is intentionally small and stable; additional classes can be added
/// later without changing the core execution semantics.
///
/// **Canonical status (ASSET-CORE-01A):** this is the canonical domain
/// asset-class type — it is the type actually checked by the live broker-submit
/// gate (`mqk_execution::gateway::BrokerGateway::submit_with_context`,
/// `M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01`), which rejects any value the
/// configured broker adapter does not declare support for
/// (`mqk_execution::BrokerAdapter::supports_asset_class`) before any broker
/// adapter is invoked. A second, independent type,
/// `mqk_md::provider::ProviderAssetClass`, exists for an unrelated purpose
/// (declaring what asset classes a market-data *provider* can serve) and is
/// not unified with this enum — `mqk-md` has no dependency on `mqk-schemas`.
/// See `mqk_md::provider::provider_asset_class_trading_class` for the
/// explicit, exhaustively-tested mapping from that type to this one's
/// vocabulary (lower-cased, singular: `"equity"`, `"option"`, `"future"`,
/// `"crypto"`, `"forex"`).
///
/// ETF is deliberately **not** a variant here: an ETF trades as `Equity` for
/// every execution-path purpose today, and is tagged only as instrument-level
/// metadata (`instrument_kind = "etf"`) in the instrument registry
/// (`mqk_md::instrument_registry::TrackedInstrument`, ETF-REGISTRY-01).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetClass {
    Equity,
    Option,
    Future,
    Crypto,
    Forex,
}

/// Scale factor for [`QtyMicros`]: 1.0 unit = `QTY_MICROS_SCALE` `QtyMicros`.
pub const QTY_MICROS_SCALE: i64 = 1_000_000;

/// A deterministic fixed-point quantity type at 1e-6 scale.
///
/// Motivation: equities start as integer shares, but future assets (crypto,
/// some brokers, fractional equity) require fractional quantities.
///
/// Scale: 1.0 unit = 1_000_000 `QtyMicros`.
///
/// **Equity invariant** (recommended): quantities should be multiples of
/// 1_000_000 when `asset_class == Equity`.
///
/// All arithmetic is exposed only via `checked_*` methods (never `Add`/`Sub`
/// operator overloads): integer overflow silently wraps in release builds
/// under the standard operators, which would violate this repo's fail-closed
/// invariant for a quantity type. Callers must handle `None` explicitly.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QtyMicros(i64);

impl QtyMicros {
    pub const ZERO: QtyMicros = QtyMicros(0);

    /// Construct from raw 1e-6 units.
    #[inline]
    pub const fn new(raw: i64) -> Self {
        QtyMicros(raw)
    }

    /// Extract the underlying raw i64.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// True if this is an exact whole unit (multiple of 1_000_000).
    #[inline]
    pub const fn is_whole(self) -> bool {
        self.0 % QTY_MICROS_SCALE == 0
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    #[inline]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// -1 / 0 / 1, mirroring `i64::signum`.
    #[inline]
    pub const fn signum(self) -> i64 {
        if self.0 > 0 {
            1
        } else if self.0 < 0 {
            -1
        } else {
            0
        }
    }

    /// Construct from a whole-unit integer quantity (e.g. equity shares).
    /// Fails closed (`None`) on overflow rather than wrapping.
    #[inline]
    pub const fn from_whole_units(units: i64) -> Option<Self> {
        match units.checked_mul(QTY_MICROS_SCALE) {
            Some(v) => Some(QtyMicros(v)),
            None => None,
        }
    }

    /// Extract as a whole-unit integer. `None` if this value carries a
    /// fractional remainder (`!is_whole()`) — never truncates silently.
    #[inline]
    pub const fn to_whole_units_checked(self) -> Option<i64> {
        if self.is_whole() {
            Some(self.0 / QTY_MICROS_SCALE)
        } else {
            None
        }
    }

    #[inline]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(QtyMicros(v)),
            None => None,
        }
    }

    #[inline]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(v) => Some(QtyMicros(v)),
            None => None,
        }
    }

    #[inline]
    pub const fn checked_neg(self) -> Option<Self> {
        match self.0.checked_neg() {
            Some(v) => Some(QtyMicros(v)),
            None => None,
        }
    }

    /// Absolute value. Fails closed (`None`) on `i64::MIN`, the one value
    /// whose magnitude cannot be represented as a positive `i64`.
    #[inline]
    pub const fn checked_abs(self) -> Option<Self> {
        match self.0.checked_abs() {
            Some(v) => Some(QtyMicros(v)),
            None => None,
        }
    }

    /// Round a fractional quantity down toward zero to the nearest multiple
    /// of `increment_micros`. Returns `None` if `increment_micros <= 0`.
    /// This never rounds a whole-share equity quantity — `is_whole()` values
    /// with `increment_micros == QTY_MICROS_SCALE` round-trip unchanged.
    #[inline]
    pub const fn floor_to_increment(self, increment_micros: i64) -> Option<Self> {
        if increment_micros <= 0 {
            return None;
        }
        let sign = self.signum();
        let magnitude = match self.0.checked_abs() {
            Some(v) => v,
            None => return None,
        };
        let floored = (magnitude / increment_micros) * increment_micros;
        if sign < 0 {
            match floored.checked_neg() {
                Some(v) => Some(QtyMicros(v)),
                None => None,
            }
        } else {
            Some(QtyMicros(floored))
        }
    }
}

/// Error returned by [`QtyMicros`]'s `FromStr` implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QtyMicrosParseError(pub String);

impl std::fmt::Display for QtyMicrosParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid QtyMicros decimal value: '{}'", self.0)
    }
}

impl std::error::Error for QtyMicrosParseError {}

/// Canonical decimal rendering: trims trailing fractional zeros, always emits
/// at least the whole-unit digits (e.g. `1000000` -> `"1"`, `1500000` ->
/// `"1.5"`, `-100` -> `"-0.0001"`, `0` -> `"0"`). Deterministic and
/// round-trips exactly through `FromStr` (DETERMINISM: canonical
/// serialization).
impl std::fmt::Display for QtyMicros {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let neg = self.0 < 0;
        let abs = self.0.unsigned_abs();
        let whole = abs / QTY_MICROS_SCALE as u64;
        let frac = abs % QTY_MICROS_SCALE as u64;
        if neg {
            write!(f, "-")?;
        }
        if frac == 0 {
            write!(f, "{whole}")
        } else {
            let mut frac_str = format!("{frac:06}");
            while frac_str.ends_with('0') {
                frac_str.pop();
            }
            write!(f, "{whole}.{frac_str}")
        }
    }
}

/// Parses a plain-ASCII decimal string (optional leading `+`/`-`, optional
/// `.` followed by 1-6 fraction digits) into `QtyMicros`. Rejects empty
/// input, non-digit characters, more than 6 fraction digits (precision loss
/// would be silent), and any value that would overflow `i64` — fails closed
/// rather than approximating.
impl std::str::FromStr for QtyMicros {
    type Err = QtyMicrosParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || QtyMicrosParseError(s.to_string());
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(err());
        }
        let (neg, rest) = match trimmed.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
        };
        if rest.is_empty() {
            return Err(err());
        }
        let mut parts = rest.splitn(2, '.');
        let int_part = parts.next().unwrap_or_default();
        let frac_part = parts.next();

        if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        // IR-B1-04: `whole`/`frac_micros`/`magnitude` are computed in `u64`,
        // not `i64`. `i64::MIN`'s magnitude (9_223_372_036_854_775_808) is
        // exactly one past `i64::MAX` and has no positive `i64`
        // representation — an `i64` intermediate would reject it even though
        // `QtyMicros(i64::MIN)` is a valid value whose own `Display` output
        // must round-trip. `u64` covers the full magnitude range for every
        // representable `QtyMicros` (`i64::MIN..=i64::MAX`).
        let whole: u64 = int_part.parse().map_err(|_| err())?;

        let frac_micros: u64 = match frac_part {
            None => 0,
            Some(f) => {
                if f.is_empty() || f.len() > 6 || !f.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(err());
                }
                let mut padded = f.to_string();
                while padded.len() < 6 {
                    padded.push('0');
                }
                padded.parse().map_err(|_| err())?
            }
        };

        let magnitude: u64 = whole
            .checked_mul(QTY_MICROS_SCALE as u64)
            .and_then(|w| w.checked_add(frac_micros))
            .ok_or_else(err)?;

        if neg {
            if magnitude == i64::MIN.unsigned_abs() {
                Ok(QtyMicros(i64::MIN))
            } else {
                i64::try_from(magnitude)
                    .map(|v| QtyMicros(-v))
                    .map_err(|_| err())
            }
        } else {
            i64::try_from(magnitude).map(QtyMicros).map_err(|_| err())
        }
    }
}

#[cfg(test)]
mod qty_micros_tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn new_and_raw_round_trip() {
        assert_eq!(QtyMicros::new(1_500_000).raw(), 1_500_000);
        assert_eq!(QtyMicros::ZERO.raw(), 0);
    }

    #[test]
    fn is_whole_true_for_exact_multiples_only() {
        assert!(QtyMicros::new(0).is_whole());
        assert!(QtyMicros::new(1_000_000).is_whole());
        assert!(QtyMicros::new(-2_000_000).is_whole());
        assert!(!QtyMicros::new(1_500_000).is_whole());
        assert!(!QtyMicros::new(-1).is_whole());
    }

    #[test]
    fn sign_predicates() {
        assert!(QtyMicros::new(5).is_positive());
        assert!(!QtyMicros::new(5).is_negative());
        assert!(QtyMicros::new(-5).is_negative());
        assert!(QtyMicros::ZERO.is_zero());
        assert_eq!(QtyMicros::new(5).signum(), 1);
        assert_eq!(QtyMicros::new(-5).signum(), -1);
        assert_eq!(QtyMicros::ZERO.signum(), 0);
    }

    #[test]
    fn from_whole_units_scales_by_1e6() {
        assert_eq!(
            QtyMicros::from_whole_units(1),
            Some(QtyMicros::new(1_000_000))
        );
        assert_eq!(
            QtyMicros::from_whole_units(-3),
            Some(QtyMicros::new(-3_000_000))
        );
        assert_eq!(QtyMicros::from_whole_units(0), Some(QtyMicros::ZERO));
    }

    #[test]
    fn from_whole_units_fails_closed_on_overflow() {
        assert_eq!(QtyMicros::from_whole_units(i64::MAX), None);
        assert_eq!(QtyMicros::from_whole_units(i64::MIN), None);
    }

    #[test]
    fn to_whole_units_checked_round_trips_equity_quantities() {
        assert_eq!(
            QtyMicros::new(100_000_000).to_whole_units_checked(),
            Some(100)
        );
        assert_eq!(
            QtyMicros::new(-1_000_000).to_whole_units_checked(),
            Some(-1)
        );
    }

    #[test]
    fn to_whole_units_checked_none_on_fractional_remainder() {
        assert_eq!(QtyMicros::new(1_500_000).to_whole_units_checked(), None);
        assert_eq!(QtyMicros::new(1).to_whole_units_checked(), None);
    }

    #[test]
    fn checked_add_sub_neg_abs_happy_path() {
        let a = QtyMicros::new(1_500_000);
        let b = QtyMicros::new(500_000);
        assert_eq!(a.checked_add(b), Some(QtyMicros::new(2_000_000)));
        assert_eq!(a.checked_sub(b), Some(QtyMicros::new(1_000_000)));
        assert_eq!(a.checked_neg(), Some(QtyMicros::new(-1_500_000)));
        assert_eq!(QtyMicros::new(-7).checked_abs(), Some(QtyMicros::new(7)));
    }

    #[test]
    fn checked_arithmetic_fails_closed_on_overflow() {
        let max = QtyMicros::new(i64::MAX);
        assert_eq!(max.checked_add(QtyMicros::new(1)), None);
        let min = QtyMicros::new(i64::MIN);
        assert_eq!(min.checked_sub(QtyMicros::new(1)), None);
        assert_eq!(min.checked_neg(), None);
        assert_eq!(min.checked_abs(), None);
    }

    #[test]
    fn floor_to_increment_respects_positive_and_negative_sign() {
        // Alpaca-style BTC min increment: 0.0001 = 100 QtyMicros.
        let increment = 100;
        assert_eq!(
            QtyMicros::new(12_345).floor_to_increment(increment),
            Some(QtyMicros::new(12_300))
        );
        assert_eq!(
            QtyMicros::new(-12_345).floor_to_increment(increment),
            Some(QtyMicros::new(-12_300))
        );
        // Equity: flooring a whole-share qty to a 1-unit increment is a no-op.
        assert_eq!(
            QtyMicros::new(3_000_000).floor_to_increment(QTY_MICROS_SCALE),
            Some(QtyMicros::new(3_000_000))
        );
    }

    #[test]
    fn floor_to_increment_rejects_non_positive_increment() {
        assert_eq!(QtyMicros::new(100).floor_to_increment(0), None);
        assert_eq!(QtyMicros::new(100).floor_to_increment(-5), None);
    }

    #[test]
    fn display_canonical_decimal_rendering() {
        assert_eq!(QtyMicros::new(1_000_000).to_string(), "1");
        assert_eq!(QtyMicros::new(1_500_000).to_string(), "1.5");
        assert_eq!(QtyMicros::new(-100).to_string(), "-0.0001");
        assert_eq!(QtyMicros::ZERO.to_string(), "0");
        assert_eq!(QtyMicros::new(-1_000_000).to_string(), "-1");
    }

    #[test]
    fn from_str_round_trips_through_display() {
        for raw in [
            0i64, 1_000_000, 1_500_000, -100, -1_000_000, 999_999, -999_999,
        ] {
            let v = QtyMicros::new(raw);
            let parsed = QtyMicros::from_str(&v.to_string()).expect("parses own display output");
            assert_eq!(parsed, v, "round-trip failed for raw={raw}");
        }
    }

    /// IR-B1-04: `Display -> FromStr` must round-trip exactly at every
    /// representable boundary, including `i64::MIN`. `i64::MIN`'s magnitude
    /// (9_223_372_036_854_775_808) has no positive `i64` representation, so
    /// a parser whose intermediate arithmetic stays in `i64` fails closed on
    /// its own `Display` output — that is the exact defect this proves
    /// fixed (see the `u64`-intermediate rewrite of `FromStr`).
    #[test]
    fn from_str_round_trips_i64_extremes_and_micro_boundaries() {
        for raw in [
            i64::MIN,
            i64::MIN + 1,
            i64::MAX,
            i64::MAX - 1,
            0,
            1,
            -1,
            999_999,
            -999_999,
            123_456_789_012,
            -123_456_789_012,
        ] {
            let v = QtyMicros::new(raw);
            let rendered = v.to_string();
            let parsed = QtyMicros::from_str(&rendered).unwrap_or_else(|e| {
                panic!("failed to parse own Display output {rendered:?} for raw={raw}: {e}")
            });
            assert_eq!(
                parsed.raw(),
                raw,
                "round-trip must preserve the exact raw value for raw={raw} (rendered={rendered:?})"
            );
        }
    }

    /// IR-B1-04: values whose magnitude exceeds what any `i64` can hold on
    /// either sign must still fail closed, not panic and not wrap — the
    /// `u64` intermediate widens the representable range up to (and
    /// including) `i64::MIN`'s magnitude, never past it.
    #[test]
    fn from_str_still_fails_closed_one_past_i64_min_magnitude() {
        // i64::MIN magnitude is 9_223_372_036_854_775_808; one more than
        // that has no QtyMicros representation on either sign.
        assert!(QtyMicros::from_str("-9223372036854.775809").is_err());
        assert!(QtyMicros::from_str("9223372036854.775808").is_err()); // positive side: max is i64::MAX
    }

    #[test]
    fn from_str_accepts_plain_forms() {
        assert_eq!(QtyMicros::from_str("1").unwrap(), QtyMicros::new(1_000_000));
        assert_eq!(
            QtyMicros::from_str("+1").unwrap(),
            QtyMicros::new(1_000_000)
        );
        assert_eq!(QtyMicros::from_str("0.0001").unwrap(), QtyMicros::new(100));
        assert_eq!(
            QtyMicros::from_str("-0.5").unwrap(),
            QtyMicros::new(-500_000)
        );
        assert_eq!(
            QtyMicros::from_str("  2.25  ").unwrap(),
            QtyMicros::new(2_250_000)
        );
    }

    #[test]
    fn from_str_fails_closed_on_excess_precision() {
        // 7 fraction digits would silently lose precision at 1e-6 scale.
        assert!(QtyMicros::from_str("1.1234567").is_err());
    }

    #[test]
    fn from_str_fails_closed_on_garbage() {
        for bad in ["", "-", "+", ".", "1.", "abc", "1.2.3", "1,5", "--1"] {
            assert!(
                QtyMicros::from_str(bad).is_err(),
                "expected error for '{bad}'"
            );
        }
    }

    #[test]
    fn from_str_fails_closed_on_overflow() {
        assert!(QtyMicros::from_str("99999999999999999999").is_err());
    }
}

/// A unique instrument identifier.
///
/// Today we key most things by `symbol` (equities). This type lets us expand
/// to derivatives/crypto while staying explicit about what is being traded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Instrument {
    /// Human-facing symbol (e.g. "AAPL", "SPY", "BTC/USD").
    pub symbol: String,
    /// Coarse asset class.
    pub asset_class: AssetClass,
    /// Venue/exchange identifier (optional but recommended for futures/crypto).
    pub venue: Option<String>,
    /// ISO currency code (e.g. "USD").
    pub currency: String,
    /// Contract specification for derivatives.
    pub contract: ContractSpec,
    /// Durable, broker-confirmed contract identity — Wave C1
    /// (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION).
    /// `None` for a research-only, backtest, or not-yet-broker-resolved
    /// instrument: this field is the ONLY place broker-confirmed contract
    /// provenance is represented, so a continuous research series (which
    /// never goes through broker contract resolution) is structurally
    /// incapable of acquiring an executable broker identity merely by
    /// existing — it would have to earn a `Some(..)` here, which nothing in
    /// the research/backtest path ever constructs.
    #[serde(default)]
    pub provenance: Option<BrokerContractProvenance>,
}

/// Durable, provider-neutral broker-confirmed contract identity — Wave C1.
///
/// Populated only once a specific broker has actually resolved a contract
/// (e.g. IBKR's `conId`/`localSymbol` for a specific dated future or FX
/// pair, confirmed via that broker's own contract-details lookup). This is
/// evidence *about* an already-constructed [`Instrument`], never a second,
/// competing identity: [`ContractSpec`] (root/expiry, base/quote, strike,
/// etc.) remains the canonical shape every asset-neutral consumer matches
/// on; `provider`/`native_contract_id`/`local_symbol`/`exchange` here are
/// purely the broker's own confirmation of a specific tradable instance of
/// that shape, and never substitute for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BrokerContractProvenance {
    /// Stable per-broker provider identifier (e.g. `"ibkr"`, `"alpaca"`).
    /// Not the same vocabulary as `mqk_md` market-data provider ids — this
    /// names the *execution* broker that confirmed this contract.
    pub provider: String,
    /// The broker's own opaque native contract identifier (e.g. IBKR's
    /// `conId`). Broker-specific format; never parsed or interpreted here.
    pub native_contract_id: String,
    /// The broker's own exact symbol spelling for this specific contract
    /// (e.g. IBKR's `localSymbol`, which for a dated future disambiguates
    /// month/year in that broker's own notation).
    pub local_symbol: String,
    /// The specific exchange the broker resolved this contract against
    /// (e.g. `"CME"`, `"GLOBEX"`, `"IDEALPRO"`).
    pub exchange: String,
}

/// Contract details for non-spot instruments.
///
/// Equity is the default (no extra fields). Options/futures/forex carry
/// enough metadata to uniquely identify contracts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContractSpec {
    /// Spot equities / ETFs.
    Equity,
    /// Listed equity/index options.
    Option {
        underlying: String,
        expiry_yyyymmdd: String,
        strike_micros: i64,
        right: OptionRight,
        multiplier: i32,
    },
    /// Futures. `expiry_yyyymm` is always an explicit, dated contract month
    /// (canonical `YYYYMM`, see [`is_canonical_yyyymm`]) — there is
    /// deliberately no "continuous"/perpetual variant and no automatic-roll
    /// mechanism anywhere in this enum or its constructors: a continuous
    /// research series is a distinct concept the research/backtest domain
    /// represents on its own terms and never converts into this executable
    /// shape. Rolling from one dated contract to the next (if ever
    /// implemented) must be an explicit, separately-authorized operator/
    /// strategy decision that constructs a brand-new `Future` value with the
    /// next `expiry_yyyymm` — never a mutation of this one.
    Future {
        root: String,
        expiry_yyyymm: String,
        multiplier: i32,
        tick_size_micros: i64,
    },
    /// Spot crypto (pair symbol is usually enough; venue matters).
    Crypto,
    /// Spot/forward FX pair — Wave C1. Canonical base/quote identity
    /// (`base_currency`/`quote_currency`, each an uppercase ISO-4217-style
    /// code; see [`is_canonical_currency_code`]), distinct from
    /// [`ContractSpec::Crypto`] even though both are quoted pairs: an FX
    /// pair's economics (pip/lot sizing, leverage, 24x5 session) are
    /// currency-market-specific, not modeled by the crypto shape.
    Forex {
        base_currency: String,
        quote_currency: String,
    },
}

/// True iff `s` is a canonical compact `YYYYMM` (exactly six ASCII digits,
/// month 01-12). Surrounding whitespace, separators and short forms are not
/// canonical: a contract's expiry is part of its identity, so only one
/// spelling of each month is accepted.
pub fn is_canonical_yyyymm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 6 && b.iter().all(u8::is_ascii_digit) && matches!(s[4..6].parse::<u32>(), Ok(1..=12))
}

/// True iff `s` is a canonical compact `YYYYMMDD`: exactly eight ASCII digits
/// naming a real calendar day (leap years honoured).
pub fn is_canonical_yyyymmdd(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 8 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[0..4].parse::<i32>(),
        s[4..6].parse::<u32>(),
        s[6..8].parse::<u32>(),
    ) else {
        return false;
    };
    chrono::NaiveDate::from_ymd_opt(y, m, d).is_some()
}

#[cfg(test)]
mod canonical_contract_date_tests {
    use super::*;

    #[test]
    fn yyyymm_accepts_only_real_months() {
        for ok in ["202601", "202612", "209912"] {
            assert!(is_canonical_yyyymm(ok), "{ok}");
        }
        for bad in [
            "",
            "2026",
            "202600",
            "202613",
            "2026-1",
            "20261",
            "2026012",
            " 202601",
            "202601 ",
            "20a601",
            "２０２６０１",
        ] {
            assert!(!is_canonical_yyyymm(bad), "{bad:?}");
        }
    }

    #[test]
    fn yyyymmdd_accepts_only_real_days() {
        for ok in ["20260101", "20261231", "20280229", "20000229"] {
            assert!(is_canonical_yyyymmdd(ok), "{ok}");
        }
        for bad in [
            "",
            "202601",
            "2026010",
            "202601011",
            "20260100",
            "20260132",
            "20260431",
            "20270229",
            "21000229",
            "20261301",
            "20260001",
            "2026-01-01",
            " 20260101",
            "20260101 ",
            "2026010a",
        ] {
            assert!(!is_canonical_yyyymmdd(bad), "{bad:?}");
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionRight {
    Call,
    Put,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

/// Broker-agnostic order specification.
///
/// `qty` is **always positive**; `side` determines direction.
/// Prices are integer micros (1 unit = 1_000_000), matching the execution
/// boundary invariant used elsewhere in the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSpec {
    pub client_order_id: String,
    pub instrument: Instrument,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub qty: QtyMicros,
    pub limit_price_micros: Option<i64>,
    pub stop_price_micros: Option<i64>,
    pub time_in_force: String,
}

/// Broker-agnostic position snapshot for an instrument.
///
/// `qty` is signed: +long, -short, 0 = flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub instrument: Instrument,
    pub qty: i64,
    /// Average entry price in integer micros.
    pub avg_price_micros: Option<i64>,
}

// ---------------------------------------------------------------------------
// Wave C1 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
// ContractSpec::Forex + BrokerContractProvenance
// ---------------------------------------------------------------------------

#[cfg(test)]
mod contract_provenance_tests {
    use super::*;

    fn base_instrument(contract: ContractSpec, asset_class: AssetClass) -> Instrument {
        Instrument {
            symbol: "EUR/USD".to_string(),
            asset_class,
            venue: Some("IDEALPRO".to_string()),
            currency: "USD".to_string(),
            contract,
            provenance: None,
        }
    }

    /// A freshly-constructed instrument (the shape every research/backtest
    /// path produces) has no broker provenance -- it cannot be mistaken for
    /// a broker-confirmed, executable contract merely by existing.
    #[test]
    fn fresh_instrument_has_no_broker_provenance_by_default() {
        let instrument = base_instrument(ContractSpec::Equity, AssetClass::Equity);
        assert_eq!(instrument.provenance, None);
    }

    /// Once a broker resolves the contract, provenance is attached without
    /// altering the canonical `ContractSpec` identity it describes.
    #[test]
    fn broker_confirmed_provenance_is_additive_not_a_second_identity() {
        let mut instrument = base_instrument(
            ContractSpec::Future {
                root: "MES".to_string(),
                expiry_yyyymm: "202606".to_string(),
                multiplier: 5,
                tick_size_micros: 250_000,
            },
            AssetClass::Future,
        );
        let before_contract = instrument.contract.clone();
        instrument.provenance = Some(BrokerContractProvenance {
            provider: "ibkr".to_string(),
            native_contract_id: "620731015".to_string(),
            local_symbol: "MESM6".to_string(),
            exchange: "CME".to_string(),
        });
        assert_eq!(
            instrument.contract, before_contract,
            "attaching provenance must never change the canonical contract shape"
        );
        assert_eq!(
            instrument.provenance.unwrap().native_contract_id,
            "620731015"
        );
    }

    /// `Instrument` deserializes from JSON that predates this field (no
    /// `"provenance"` key at all) as `None` -- `#[serde(default)]` keeps
    /// every pre-existing fixture/registry file readable unchanged.
    #[test]
    fn provenance_defaults_to_none_when_absent_from_json() {
        let json = r#"{
            "symbol": "AAPL",
            "asset_class": "Equity",
            "venue": null,
            "currency": "USD",
            "contract": "Equity"
        }"#;
        let instrument: Instrument = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(instrument.provenance, None);
    }

    /// `ContractSpec::Forex` round-trips through JSON exactly, and a Forex
    /// pair is structurally distinct from a Crypto pair even though both are
    /// quoted-pair shapes (Wave C1's whole reason for a dedicated variant
    /// instead of reusing `ContractSpec::Crypto` with an `AssetClass::Forex`
    /// label, which is what this repo did before this patch).
    #[test]
    fn forex_contract_spec_round_trips_and_is_distinct_from_crypto() {
        let forex = ContractSpec::Forex {
            base_currency: "EUR".to_string(),
            quote_currency: "USD".to_string(),
        };
        let json = serde_json::to_string(&forex).expect("serialize");
        let back: ContractSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(forex, back);
        assert_ne!(forex, ContractSpec::Crypto);
    }

    /// A different `native_contract_id` (the broker's own confirmed
    /// identity) is a different provenance, even when every other field
    /// (provider/local_symbol/exchange) is identical -- provenance equality
    /// must be exact broker-confirmed identity, not a partial match.
    #[test]
    fn provenance_equality_requires_the_exact_native_contract_id() {
        let a = BrokerContractProvenance {
            provider: "ibkr".to_string(),
            native_contract_id: "620731015".to_string(),
            local_symbol: "MESM6".to_string(),
            exchange: "CME".to_string(),
        };
        let mut b = a.clone();
        b.native_contract_id = "999999999".to_string();
        assert_ne!(a, b);
    }
}
