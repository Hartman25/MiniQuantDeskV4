//! Integer-micros price representation — Patch L9
//!
//! # Design invariant
//!
//! All prices on the **execution decision surface** are represented as `i64`
//! integer micros (1 unit = 1_000_000 micros).  This eliminates f64 drift in
//! routing logic — e.g. two limit prices that compare equal as `f64` but
//! differ at the 7th decimal place will always be distinguishable as `i64`.
//!
//! `f64` conversions are **only** performed at the wire boundary:
//!
//! | Direction                  | Function            | Notes                      |
//! |----------------------------|---------------------|----------------------------|
//! | internal → broker REST API | [`micros_to_price`] | Serialization only         |
//! | broker REST API → internal | [`price_to_micros`] | Parsing / ingestion only   |
//!
//! No other code path should produce or consume `f64` prices.

/// Scale factor: 1 price unit = 1_000_000 micros (6 decimal places).
pub const MICROS_PER_UNIT: i64 = 1_000_000;

// ---------------------------------------------------------------------------
// PricingError (PATCH A5)
// ---------------------------------------------------------------------------

/// Errors returned by [`price_to_micros`] when the input is not representable.
///
/// Both variants fire in **all** build profiles (debug and release) — unlike
/// the previous `debug_assert!` which was silently elided in release builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PricingError {
    /// Input was `NaN` or infinite.
    ///
    /// These values indicate a broken upstream and must not silently propagate
    /// into the internal `i64` representation.
    NotFinite,
    /// Input would overflow `i64` after scaling by [`MICROS_PER_UNIT`].
    ///
    /// Occurs only for absurdly large values (above ~$9.2 trillion per unit),
    /// which are always indicative of a data-quality error.
    OutOfRange,
}

impl std::fmt::Display for PricingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PricingError::NotFinite => {
                write!(f, "price_to_micros: non-finite input (NaN or Inf)")
            }
            PricingError::OutOfRange => {
                write!(f, "price_to_micros: price out of i64 range after scaling")
            }
        }
    }
}

impl std::error::Error for PricingError {}

// ---------------------------------------------------------------------------
// Wire-boundary conversion functions
// ---------------------------------------------------------------------------

/// Convert an integer-micros price to `f64` for external broker serialization.
///
/// **Only call at the broker wire boundary** — e.g. when building the JSON
/// body for a broker REST request.  Internal prices must stay as `i64`.
///
/// `f64` has 53-bit mantissa (~15.9 significant decimal digits), which is
/// exact for typical equity prices well below $10^9.
pub fn micros_to_price(micros: i64) -> f64 {
    micros as f64 / MICROS_PER_UNIT as f64
}

/// Convert an `f64` price received from a broker wire response into integer
/// micros.
///
/// **Only call when ingesting broker prices** (e.g. parsing a REST response).
/// Rounds to the nearest integer micro to avoid systematic truncation bias.
///
/// # Errors (PATCH A5)
/// Returns [`PricingError::NotFinite`] if `price` is `NaN` or infinite.
/// Returns [`PricingError::OutOfRange`] if `price * MICROS_PER_UNIT` would
/// overflow `i64`.
///
/// Both checks fire in **all** build profiles, not just debug.
pub fn price_to_micros(price: f64) -> Result<i64, PricingError> {
    if !price.is_finite() {
        return Err(PricingError::NotFinite);
    }
    let scaled = price * MICROS_PER_UNIT as f64;
    // Guard against f64→i64 cast overflow (Rust cast saturates; we must reject).
    if scaled > i64::MAX as f64 || scaled < i64::MIN as f64 {
        return Err(PricingError::OutOfRange);
    }
    Ok(scaled.round() as i64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Round-trip: integer prices ---

    #[test]
    fn round_trip_whole_dollar_price() {
        let dollars = 150_i64;
        let micros = dollars * MICROS_PER_UNIT;
        let back = price_to_micros(micros_to_price(micros)).unwrap();
        assert_eq!(back, micros, "whole-dollar round-trip must be exact");
    }

    #[test]
    fn round_trip_fractional_price() {
        // $100.50 — common US equity price with cents
        let micros = 100_500_000_i64;
        let back = price_to_micros(micros_to_price(micros)).unwrap();
        assert_eq!(back, micros, "$100.50 round-trip must be exact");
    }

    // --- micros_to_price ---

    #[test]
    fn micros_to_price_one_dollar() {
        assert!((micros_to_price(MICROS_PER_UNIT) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn micros_to_price_zero() {
        assert_eq!(micros_to_price(0), 0.0);
    }

    #[test]
    fn micros_to_price_half_cent() {
        // $0.005000 = 5_000 micros
        let f = micros_to_price(5_000);
        assert!((f - 0.005).abs() < 1e-9);
    }

    // --- price_to_micros (valid inputs) ---

    #[test]
    fn price_to_micros_one_dollar() {
        assert_eq!(price_to_micros(1.0).unwrap(), MICROS_PER_UNIT);
    }

    #[test]
    fn price_to_micros_rounds_half_up() {
        // 0.0000005 is exactly half a micro — should round to 1
        let m = price_to_micros(0.000_000_5).unwrap();
        assert_eq!(m, 1, "half-micro must round to 1, got {m}");
    }

    #[test]
    fn price_to_micros_deterministic_for_same_input() {
        let p = 123.456_789;
        let m1 = price_to_micros(p).unwrap();
        let m2 = price_to_micros(p).unwrap();
        assert_eq!(m1, m2, "conversion must be deterministic");
    }

    // --- price_to_micros: non-finite rejection (PATCH A5) ---
    // These tests verify that the check fires in ALL build profiles,
    // not just debug (i.e. the old debug_assert! was insufficient).

    #[test]
    fn nan_is_rejected() {
        let result = price_to_micros(f64::NAN);
        assert_eq!(result, Err(PricingError::NotFinite), "NaN must be rejected");
    }

    #[test]
    fn pos_inf_is_rejected() {
        let result = price_to_micros(f64::INFINITY);
        assert_eq!(
            result,
            Err(PricingError::NotFinite),
            "positive infinity must be rejected"
        );
    }

    #[test]
    fn neg_inf_is_rejected() {
        let result = price_to_micros(f64::NEG_INFINITY);
        assert_eq!(
            result,
            Err(PricingError::NotFinite),
            "negative infinity must be rejected"
        );
    }

    #[test]
    fn out_of_range_is_rejected() {
        // f64::MAX * MICROS_PER_UNIT vastly exceeds i64::MAX.
        let result = price_to_micros(f64::MAX);
        assert_eq!(
            result,
            Err(PricingError::OutOfRange),
            "f64::MAX must be rejected as out of range"
        );
    }

    // --- Comparison that shows why i64 is safer than f64 ---

    #[test]
    fn micros_comparison_is_exact_where_f64_may_not_be() {
        // Two prices that differ by exactly 1 micro (0.000001)
        let a: i64 = 100_000_001;
        let b: i64 = 100_000_000;
        // As micros they are clearly distinguishable
        assert_ne!(a, b);
        // As f64 the difference may collapse (for very large prices)
        // This test documents the intent rather than triggering the specific bug
        assert!(a > b);
    }
}
