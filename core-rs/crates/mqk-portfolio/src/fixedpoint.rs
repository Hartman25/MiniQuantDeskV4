//! Fixed-point money type — M4-1
//!
//! # Motivation
//!
//! All money amounts in this system use a 1e-6 (micros) fixed-point
//! representation stored as `i64`.  Using raw `i64` for money is error-prone:
//! it allows accidental arithmetic with unrelated integers (quantities, IDs,
//! prices at different scales) without any compile-time signal.
//!
//! `Micros` wraps the raw `i64` so the type system prevents:
//! - Implicit construction from raw `i64` (no `From<i64>` impl).
//! - Mixing `Micros` with unrelated `i64` values in arithmetic.
//!
//! # Scale
//!
//! 1 USD = 1_000_000 Micros.  All monetary values (cash, PnL, price × qty)
//! use this scale.  Non-monetary quantities (share counts, order IDs, day
//! counters) remain plain `i64`/`u64` and are never implicitly convertible.
//!
//! # Arithmetic
//!
//! - `Add`, `Sub`, `Neg`, `AddAssign`, `SubAssign` are implemented for
//!   `Micros op Micros`; these panic on overflow in debug builds and wrap in
//!   release (matching Rust's standard integer semantics).
//! - `saturating_add` / `saturating_sub` — safe alternatives that clamp at
//!   `i64::MAX` / `i64::MIN`.
//! - `checked_mul_qty(qty: i64) -> Option<Micros>` — multiply a per-unit
//!   Micros price by an integer share quantity with overflow detection.
//!   Returns `None` on overflow; callers must handle this explicitly.

use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

// ---------------------------------------------------------------------------
// Micros newtype
// ---------------------------------------------------------------------------

/// A fixed-point monetary amount at 1e-6 scale (micros).
///
/// 1 USD = `Micros(1_000_000)`.
///
/// # Construction
///
/// Use [`Micros::new`] for explicit construction.  There is intentionally
/// no `From<i64>` implementation — callers must be deliberate about when a
/// raw integer represents a monetary amount.
///
/// # Retrieval
///
/// Use [`Micros::raw`] to extract the underlying `i64` when crossing
/// crate or layer boundaries that require raw integers.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Micros(i64);

impl Micros {
    /// Zero monetary amount.
    pub const ZERO: Micros = Micros(0);

    /// Maximum representable value.
    pub const MAX: Micros = Micros(i64::MAX);

    /// Minimum representable value.
    pub const MIN: Micros = Micros(i64::MIN);

    /// Construct a `Micros` from a raw `i64`.
    ///
    /// Prefer over transmutation; use only when the raw integer is known to
    /// represent a fixed-point monetary amount at 1e-6 scale.
    #[inline]
    pub const fn new(raw: i64) -> Self {
        Micros(raw)
    }

    /// Extract the underlying raw `i64`.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Saturating addition — clamps at [`Micros::MAX`] on overflow.
    #[inline]
    pub fn saturating_add(self, rhs: Micros) -> Micros {
        Micros(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction — clamps at [`Micros::MIN`] on underflow.
    #[inline]
    pub fn saturating_sub(self, rhs: Micros) -> Micros {
        Micros(self.0.saturating_sub(rhs.0))
    }

    /// Absolute value.  `Micros::MIN.abs()` saturates to `Micros::MAX`.
    #[inline]
    pub fn abs(self) -> Micros {
        Micros(self.0.saturating_abs())
    }

    /// Sign: returns `1`, `0`, or `-1` as a plain integer (not a Micros value).
    #[inline]
    pub fn signum(self) -> i64 {
        self.0.signum()
    }

    /// `true` if this amount is non-negative.
    #[inline]
    pub fn is_non_negative(self) -> bool {
        self.0 >= 0
    }

    /// `true` if this amount is strictly negative.
    #[inline]
    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// Multiply a per-unit price by an integer share quantity.
    ///
    /// Returns `None` if the multiplication overflows `i64`.  Callers MUST
    /// handle `None` explicitly; there is no implicit clamp here because
    /// overflow in a trade value calculation is a critical error, not a
    /// routine saturation.
    ///
    /// `qty` is a plain share count (not a Micros value).
    #[inline]
    pub fn checked_mul_qty(self, qty: i64) -> Option<Micros> {
        self.0.checked_mul(qty).map(Micros)
    }
}

// ---------------------------------------------------------------------------
// Arithmetic operators (closed over Micros)
// ---------------------------------------------------------------------------

impl Add for Micros {
    type Output = Micros;
    #[inline]
    fn add(self, rhs: Micros) -> Micros {
        Micros(self.0 + rhs.0)
    }
}

impl Sub for Micros {
    type Output = Micros;
    #[inline]
    fn sub(self, rhs: Micros) -> Micros {
        Micros(self.0 - rhs.0)
    }
}

impl Neg for Micros {
    type Output = Micros;
    #[inline]
    fn neg(self) -> Micros {
        Micros(-self.0)
    }
}

impl AddAssign for Micros {
    #[inline]
    fn add_assign(&mut self, rhs: Micros) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Micros {
    #[inline]
    fn sub_assign(&mut self, rhs: Micros) {
        self.0 -= rhs.0;
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl std::fmt::Display for Micros {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dollars = self.0 / 1_000_000;
        let frac = (self.0 % 1_000_000).abs();
        // When |value| < $1 and value is negative, dollars truncates to 0,
        // losing the sign.  Emit "-0" explicitly in that case.
        if self.0 < 0 && dollars == 0 {
            write!(f, "-{dollars}.{frac:06}")
        } else {
            write!(f, "{dollars}.{frac:06}")
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_additive_identity() {
        let a = Micros::new(42_000_000);
        assert_eq!(a + Micros::ZERO, a);
        assert_eq!(Micros::ZERO + a, a);
    }

    #[test]
    fn add_and_sub_roundtrip() {
        let a = Micros::new(100_000_000);
        let b = Micros::new(25_000_000);
        assert_eq!((a + b) - b, a);
    }

    #[test]
    fn neg_produces_opposite_sign() {
        let pos = Micros::new(5_000_000);
        let neg = -pos;
        assert_eq!(neg.raw(), -5_000_000);
        assert_eq!(-neg, pos);
    }

    #[test]
    fn ord_less_than() {
        let a = Micros::new(1_000_000);
        let b = Micros::new(2_000_000);
        assert!(a < b);
        assert!(b > a);
        assert!(a <= a);
    }

    #[test]
    fn saturating_add_clamps_at_max() {
        let near_max = Micros::MAX;
        let result = near_max.saturating_add(Micros::new(1));
        assert_eq!(result, Micros::MAX);
    }

    #[test]
    fn saturating_sub_clamps_at_min() {
        let near_min = Micros::MIN;
        let result = near_min.saturating_sub(Micros::new(1));
        assert_eq!(result, Micros::MIN);
    }

    #[test]
    fn abs_of_negative() {
        let neg = Micros::new(-10_000_000);
        assert_eq!(neg.abs(), Micros::new(10_000_000));
    }

    #[test]
    fn abs_of_min_saturates_to_max() {
        // i64::MIN has no positive counterpart; saturating_abs returns i64::MAX.
        assert_eq!(Micros::MIN.abs(), Micros::MAX);
    }

    #[test]
    fn signum_values() {
        assert_eq!(Micros::new(5).signum(), 1);
        assert_eq!(Micros::new(-5).signum(), -1);
        assert_eq!(Micros::ZERO.signum(), 0);
    }

    #[test]
    fn raw_roundtrip() {
        let raw = 123_456_789_i64;
        assert_eq!(Micros::new(raw).raw(), raw);
    }

    #[test]
    fn checked_mul_qty_normal() {
        let price = Micros::new(100_000_000); // $100.00
        let qty = 10_i64;
        let result = price.checked_mul_qty(qty).expect("should not overflow");
        assert_eq!(result, Micros::new(1_000_000_000)); // $1000.00
    }

    #[test]
    fn checked_mul_qty_overflow_returns_none() {
        let price = Micros::MAX;
        let qty = 2_i64;
        assert_eq!(price.checked_mul_qty(qty), None);
    }

    #[test]
    fn add_assign_works() {
        let mut acc = Micros::new(10_000_000);
        acc += Micros::new(5_000_000);
        assert_eq!(acc, Micros::new(15_000_000));
    }

    #[test]
    fn display_formats_with_six_decimal_places() {
        let m = Micros::new(1_500_000); // $1.50
        assert_eq!(format!("{m}"), "1.500000");
    }

    #[test]
    fn display_negative() {
        let m = Micros::new(-2_750_000); // -$2.75
        assert_eq!(format!("{m}"), "-2.750000");
    }

    #[test]
    fn is_non_negative_and_is_negative() {
        assert!(Micros::new(0).is_non_negative());
        assert!(Micros::new(1).is_non_negative());
        assert!(!Micros::new(-1).is_non_negative());
        assert!(Micros::new(-1).is_negative());
        assert!(!Micros::new(0).is_negative());
    }
}
