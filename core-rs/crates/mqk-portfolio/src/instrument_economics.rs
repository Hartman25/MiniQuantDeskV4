//! ASSET-CORE-04A: pure, default-unused instrument economics model.
//!
//! This is a foundation/model seam for the future multi-asset portfolio
//! ledger (`ASSET-CORE-04`), not a production cutover. It makes the current
//! equity assumptions this crate's live accounting already relies on
//! explicit -- whole shares, single currency, multiplier implicitly 1 -- and
//! shows those assumptions are a special case of a more general,
//! multiplier/currency/quantity-scale-aware single-position valuation.
//!
//! **Nothing in this module is called by `accounting.rs`, `metrics.rs`, or
//! `valuation.rs`.** Those remain the unmodified, authoritative live
//! equity-accounting and live-weights paths. This module is additive and
//! has zero production callers anywhere in the workspace.
//!
//! # Relationship to existing economics/valuation seams
//!
//! - [`crate::valuation::compute_portfolio_weights`] (`PORTFOLIO-LIVE-WEIGHTS-01`) is a *portfolio*-level seam: it turns a whole book of signed quantities + marks into NAV/weights, with no currency or multiplier concept (every position is implicitly multiplier=1, implicitly the account's own currency). This module is a *position*-level seam: one instrument, one quantity, one mark in -> one valuation out, but multiplier- and currency-aware. A future `ASSET-CORE-04B` could compose this module's per-position output into a multi-asset NAV the way `valuation.rs` already does for equities.
//! - `mqk_backtest::economics::BacktestInstrumentEconomics` (`BACKTEST-MULTIPLIER-MARGIN-01`) is a *backtest-only* multiplier seam living in a different crate, with a raw integer `contract_multiplier` (e.g. `50`, not micros-scaled) and no currency field at all. This module's `contract_multiplier_micros` is expressed in the same [`crate::MICROS_SCALE`] (1e-6) fixed-point convention as every other monetary/quantity field in this crate, so `contract_multiplier_micros == MICROS_SCALE` means "1.0x", matching this crate's existing `price_micros`/`cash_micros` convention rather than the backtest crate's plain-integer one. The two are intentionally not unified by this patch -- `mqk-backtest` depends on `mqk-portfolio` already, and reversing or adding a cross-crate economics dependency is out of this patch's scope (see module docs in `mqk-backtest::economics` for why it chose to stay independent).
//! - `mqk_md::instrument_registry_v2::ContractDefinitionV2::{Future,Option}` carry a raw integer `multiplier: i64` field (e.g. `50`). A future bridge from that type to [`InstrumentEconomics`] would need to multiply by [`crate::MICROS_SCALE`] to convert "50" into `contract_multiplier_micros = 50_000_000`. No such bridge exists yet: `mqk-portfolio` has zero Cargo dependencies today, and depending on `mqk-md` (which pulls in `serde`/`anyhow`) would widen this crate's dependency graph more than this foundation patch's scope justifies. Deferred as `ASSET-CORE-04B`.
//!
//! # Design
//!
//! [`InstrumentEconomics`] is per-instrument metadata; [`PositionEconomicsInput`]
//! pairs it with a signed quantity, an optional mark, and the account's own
//! currency; [`value_position_economics`] is the one pure function that
//! turns that into a [`PositionEconomicsValue`] with an explicit
//! [`InstrumentEconomicsTruthState`] -- never a fabricated notional.
//!
//! `quantity_scale`, `min_trade_qty_micros`, and `tick_size_micros` on
//! [`InstrumentEconomics`] are forward-looking descriptive metadata only:
//! [`value_position_economics`] never reads them. They exist so a future
//! patch can describe (and, later, enforce) fractional/lot-size granularity
//! without this patch fabricating enforcement it cannot yet prove -- mirroring
//! `BacktestInstrumentEconomics`'s margin fields, which are metadata-only in
//! exactly the same way.
//!
//! All arithmetic is integer, deterministic, and overflow-checked via `i128`
//! (never `f64`). No IO, no time source, no broker/provider/DB access, no
//! RNG -- consistent with the rest of this crate.

/// Truthful outcome of [`value_position_economics`].
///
/// `NavUnavailable` and `QuantityScaleUnsupported` are reserved for a future
/// portfolio-level aggregation (`NavUnavailable`, mirroring
/// [`crate::valuation::PortfolioWeightsSnapshot`]'s `"nav_unavailable"`
/// state, which is meaningless for a single position) and a future
/// quantity-granularity enforcement patch (`QuantityScaleUnsupported`).
/// Neither is produced by [`value_position_economics`] in this patch; they
/// exist now so a later patch's match arms are additive rather than a
/// breaking enum change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentEconomicsTruthState {
    /// A notional was computed (or the position is flat, which always
    /// values to zero deterministically).
    Active,
    /// `signed_qty_micros != 0` but no mark was supplied.
    MissingMark,
    /// `contract_multiplier_micros <= 0`.
    MissingMultiplier,
    /// `quote_currency` or `account_currency` is empty.
    MissingCurrency,
    /// `quote_currency != account_currency`. This model never fakes FX
    /// conversion -- the caller must resolve currency before calling, or
    /// accept this refusal.
    CurrencyConversionUnsupported,
    /// Reserved for a future quantity-granularity enforcement patch. Not
    /// produced by [`value_position_economics`] in this patch.
    QuantityScaleUnsupported,
    /// Reserved for a future portfolio-level (multi-position) aggregation.
    /// Not produced by [`value_position_economics`] in this patch.
    NavUnavailable,
    /// The qty/price/multiplier product could not be represented even in
    /// `i128`.
    Overflow,
    /// `asset_class` is empty. This function is deliberately generic over
    /// any *non-empty* asset class string (equity/future/option/crypto/forex/
    /// anything else) -- it never enables or implies trading enablement for
    /// any of them -- but an empty asset class can never be valid descriptive
    /// metadata.
    UnsupportedInstrument,
}

/// Per-instrument economics metadata. Pure data; no behavior.
///
/// `contract_multiplier_micros` uses this crate's [`crate::MICROS_SCALE`]
/// (1e-6) convention: `MICROS_SCALE` itself (`1_000_000`) means "1.0x". An
/// equity/share instrument is `MICROS_SCALE` (see [`InstrumentEconomics::equity`]);
/// a futures contract with a 50x multiplier is `50 * MICROS_SCALE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentEconomics {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub quote_currency: String,
    pub contract_multiplier_micros: i64,
    /// Descriptive only -- see module docs. Not read by [`value_position_economics`].
    pub quantity_scale: i64,
    /// Descriptive only -- see module docs. Not read by [`value_position_economics`].
    pub min_trade_qty_micros: Option<i64>,
    /// Descriptive only -- see module docs. Not read by [`value_position_economics`].
    pub tick_size_micros: Option<i64>,
}

impl InstrumentEconomics {
    /// Default equity/share economics: multiplier = 1.0x, whole-share
    /// quantity granularity, no min-trade-size or tick-size metadata.
    ///
    /// This is the implicit behavior of every equity position in this
    /// crate's existing live paths today (`accounting::{buy_fifo,
    /// sell_fifo}`, `metrics::compute_equity_micros`,
    /// `valuation::compute_portfolio_weights`) -- none of which read a
    /// multiplier at all, so multiplier=1 here reproduces their
    /// un-multiplied `qty * price` math exactly (proved by this module's
    /// `eq01`/`eq02` tests).
    pub fn equity(
        instrument_id: impl Into<String>,
        symbol: impl Into<String>,
        currency: impl Into<String>,
    ) -> Self {
        Self {
            instrument_id: instrument_id.into(),
            symbol: symbol.into(),
            asset_class: "equity".to_string(),
            quote_currency: currency.into(),
            contract_multiplier_micros: crate::MICROS_SCALE,
            quantity_scale: crate::MICROS_SCALE,
            min_trade_qty_micros: None,
            tick_size_micros: None,
        }
    }
}

/// One position's economics valuation request: an instrument, a signed
/// quantity (positive = long, negative = short, 0 = flat), an optional mark,
/// and the account's own currency.
///
/// `signed_qty_micros` and `mark_price_micros` follow this crate's
/// `*_micros` (1e-6) fixed-point convention throughout -- e.g. 10 shares is
/// `10 * MICROS_SCALE`, and 0.5 (fractional/crypto-style) units is `500_000`.
/// This module never enforces that a quantity is a whole multiple of any
/// particular granularity; fractional values are representable here purely
/// as a model, independent of whether any asset class is actually enabled
/// for trading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionEconomicsInput {
    pub instrument: InstrumentEconomics,
    pub signed_qty_micros: i64,
    pub mark_price_micros: Option<i64>,
    pub account_currency: String,
}

/// Truthful, point-in-time valuation of one [`PositionEconomicsInput`]. See
/// [`InstrumentEconomicsTruthState`] for the full truth-state contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionEconomicsValue {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub quote_currency: String,
    pub account_currency: String,
    pub signed_qty_micros: i64,
    pub mark_price_micros: Option<i64>,
    pub contract_multiplier_micros: i64,
    pub notional_micros: Option<i128>,
    pub absolute_notional_micros: Option<i128>,
    pub truth_state: InstrumentEconomicsTruthState,
    pub reason_code: String,
}

/// [`crate::MICROS_SCALE`] squared, as `i128`. A `qty_micros * price_micros
/// * multiplier_micros` triple product carries one extra factor of
/// `MICROS_SCALE` from `qty_micros` and one from `multiplier_micros`
/// (`price_micros` already carries the single factor every other monetary
/// value in this crate carries) -- dividing by this constant descales back
/// to a plain `price_micros`-scaled notional.
const MICROS_SCALE_SQUARED_I128: i128 =
    (crate::MICROS_SCALE as i128) * (crate::MICROS_SCALE as i128);

/// Checked `qty_micros * price_micros * multiplier_micros`, descaled by
/// [`MICROS_SCALE_SQUARED_I128`]. Returns `None` if either multiplication
/// step overflows `i128`, so the caller can fail closed with an explicit
/// [`InstrumentEconomicsTruthState::Overflow`] rather than panicking or
/// silently wrapping/saturating.
///
/// Exact for all realistic finance magnitudes -- a single position's qty,
/// price, and multiplier, each comfortably under `i64::MAX`, fit `i128` even
/// tripled (`i64::MAX` squared is already only ~half of `i128::MAX`).
/// Integer division truncates toward zero, which only loses precision at
/// sub-micro-dollar magnitudes no real position can reach (the same
/// trade-off `weight_bps_of_nav` documents in `valuation.rs`).
fn checked_notional_micros(
    qty_micros: i64,
    price_micros: i64,
    multiplier_micros: i64,
) -> Option<i128> {
    let step1 = (qty_micros as i128).checked_mul(price_micros as i128)?;
    let step2 = step1.checked_mul(multiplier_micros as i128)?;
    Some(step2 / MICROS_SCALE_SQUARED_I128)
}

/// Non-panicking `i128` absolute value. `i128::MIN` has no positive
/// counterpart; falls back to `i128::MAX` rather than panicking (mirrors
/// `Micros::abs`'s `saturating_abs` precedent in `fixedpoint.rs`).
fn abs_i128_saturating(x: i128) -> i128 {
    x.checked_abs().unwrap_or(i128::MAX)
}

/// Value one position's economics. Never fabricates a notional: every
/// refusal returns `notional_micros: None` / `absolute_notional_micros: None`
/// with an explicit [`InstrumentEconomicsTruthState`] and human-readable
/// `reason_code`.
///
/// Check order (first match wins -- documented so multi-failure inputs are
/// deterministic, not merely "whichever check happened to run first"):
///
/// 1. Empty `quote_currency` or `account_currency` -> [`MissingCurrency`](InstrumentEconomicsTruthState::MissingCurrency).
/// 2. Empty `asset_class` -> [`UnsupportedInstrument`](InstrumentEconomicsTruthState::UnsupportedInstrument).
/// 3. `contract_multiplier_micros <= 0` -> [`MissingMultiplier`](InstrumentEconomicsTruthState::MissingMultiplier).
/// 4. `quote_currency != account_currency` -> [`CurrencyConversionUnsupported`](InstrumentEconomicsTruthState::CurrencyConversionUnsupported).
/// 5. `signed_qty_micros == 0` -> [`Active`](InstrumentEconomicsTruthState::Active), zero notional, no mark required.
/// 6. `mark_price_micros.is_none()` -> [`MissingMark`](InstrumentEconomicsTruthState::MissingMark).
/// 7. Overflow computing the notional -> [`Overflow`](InstrumentEconomicsTruthState::Overflow).
/// 8. Otherwise -> [`Active`](InstrumentEconomicsTruthState::Active) with a computed notional.
///
/// Structural checks (1-4) run even for a flat position: a flat position
/// never *needs* a mark, but it still should not be reported `Active` over
/// instrument metadata that is itself malformed -- consistent with this
/// crate's fail-closed-over-fail-open posture.
pub fn value_position_economics(input: PositionEconomicsInput) -> PositionEconomicsValue {
    let PositionEconomicsInput {
        instrument,
        signed_qty_micros,
        mark_price_micros,
        account_currency,
    } = input;

    let InstrumentEconomics {
        instrument_id,
        symbol,
        asset_class,
        quote_currency,
        contract_multiplier_micros,
        quantity_scale: _,
        min_trade_qty_micros: _,
        tick_size_micros: _,
    } = instrument;

    if quote_currency.trim().is_empty() || account_currency.trim().is_empty() {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::MissingCurrency,
            reason_code: "missing_quote_or_account_currency".to_string(),
        };
    }

    if asset_class.trim().is_empty() {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::UnsupportedInstrument,
            reason_code: "empty_asset_class".to_string(),
        };
    }

    if contract_multiplier_micros <= 0 {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::MissingMultiplier,
            reason_code: "contract_multiplier_micros_must_be_positive".to_string(),
        };
    }

    if quote_currency != account_currency {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::CurrencyConversionUnsupported,
            reason_code: "quote_currency_does_not_match_account_currency".to_string(),
        };
    }

    if signed_qty_micros == 0 {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: Some(0),
            absolute_notional_micros: Some(0),
            truth_state: InstrumentEconomicsTruthState::Active,
            reason_code: "flat_position".to_string(),
        };
    }

    let Some(mark) = mark_price_micros else {
        return PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::MissingMark,
            reason_code: "missing_mark_for_non_flat_position".to_string(),
        };
    };

    match checked_notional_micros(signed_qty_micros, mark, contract_multiplier_micros) {
        Some(notional) => PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: Some(notional),
            absolute_notional_micros: Some(abs_i128_saturating(notional)),
            truth_state: InstrumentEconomicsTruthState::Active,
            reason_code: "active".to_string(),
        },
        None => PositionEconomicsValue {
            instrument_id,
            symbol,
            asset_class,
            quote_currency,
            account_currency,
            signed_qty_micros,
            mark_price_micros,
            contract_multiplier_micros,
            notional_micros: None,
            absolute_notional_micros: None,
            truth_state: InstrumentEconomicsTruthState::Overflow,
            reason_code: "notional_computation_overflowed_i128".to_string(),
        },
    }
}
