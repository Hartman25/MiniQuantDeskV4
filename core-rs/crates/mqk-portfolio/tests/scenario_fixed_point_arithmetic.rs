//! Scenario: Fixed-point money type arithmetic and type boundary — M4-1
//!
//! # Invariants under test
//!
//! 1. Micros arithmetic is closed — Add/Sub/Neg only accept Micros, never
//!    raw i64. The compiler enforces this at compile time (no runtime test
//!    needed; the test file compiles only if the type boundary is respected).
//!
//! 2. Conservation: sum of debits equals sum of credits over a ledger
//!    sequence when expressed as Micros.
//!
//! 3. Saturation: saturating_add/saturating_sub clamp at i64 extremes;
//!    no silent wrap-around.
//!
//! 4. checked_mul_qty returns None on overflow and Some on normal values.
//!
//! 5. Ordering: Micros respects the natural i64 total order.
//!
//! 6. Display: formats as dollars.microseconds with six decimal places.
//!
//! All tests are pure; no IO, no DB, no network.

use mqk_portfolio::Micros;

const ONE_DOLLAR: Micros = Micros::new(1_000_000);
const TEN_DOLLARS: Micros = Micros::new(10_000_000);

// ---------------------------------------------------------------------------
// 1. Type boundary: this file compiles only because all arithmetic uses Micros
//    operands. An attempt to write `ONE_DOLLAR + 1_i64` would be a compile
//    error (no Add<i64> impl). That property is enforced by the compiler; the
//    tests below exercise the *allowed* surface.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2. Conservation: buy-then-sell cash flow nets to zero minus fees
// ---------------------------------------------------------------------------

#[test]
fn buy_sell_cash_conservation() {
    let initial_cash = Micros::new(100_000 * 1_000_000); // $100,000
    let price = Micros::new(150 * 1_000_000);            // $150/share
    let qty = 10_i64;
    let fee = Micros::new(500_000);                       // $0.50 each way

    let cost = price.checked_mul_qty(qty).unwrap() + fee;
    let proceeds = price.checked_mul_qty(qty).unwrap() - fee;

    let after_buy = initial_cash - cost;
    let after_sell = after_buy + proceeds;

    // Net: initial - 2*fee (one on each side)
    let expected = initial_cash - fee - fee;
    assert_eq!(after_sell, expected, "cash conservation: only fees are lost");
}

// ---------------------------------------------------------------------------
// 3. Saturation at MAX / MIN
// ---------------------------------------------------------------------------

#[test]
fn saturating_add_does_not_overflow() {
    let result = Micros::MAX.saturating_add(Micros::new(1_000_000));
    assert_eq!(result, Micros::MAX, "saturating_add must clamp at MAX");
    // Importantly: the raw i64 did NOT wrap around.
    assert!(result.raw() > 0, "must remain positive after saturation");
}

#[test]
fn saturating_sub_does_not_underflow() {
    let result = Micros::MIN.saturating_sub(Micros::new(1_000_000));
    assert_eq!(result, Micros::MIN, "saturating_sub must clamp at MIN");
    assert!(result.raw() < 0, "must remain negative after saturation");
}

#[test]
fn normal_add_does_not_saturate() {
    let a = Micros::new(1_000_000);
    let b = Micros::new(2_000_000);
    assert_eq!(a.saturating_add(b), Micros::new(3_000_000));
}

// ---------------------------------------------------------------------------
// 4. checked_mul_qty
// ---------------------------------------------------------------------------

#[test]
fn checked_mul_qty_returns_correct_value() {
    let price = Micros::new(100 * 1_000_000); // $100
    let result = price.checked_mul_qty(7).expect("should not overflow");
    assert_eq!(result, Micros::new(700 * 1_000_000)); // $700
}

#[test]
fn checked_mul_qty_returns_none_on_overflow() {
    // i64::MAX * 2 overflows
    assert_eq!(Micros::MAX.checked_mul_qty(2), None);
}

#[test]
fn checked_mul_qty_with_zero_qty() {
    let price = Micros::new(999 * 1_000_000);
    let result = price.checked_mul_qty(0).expect("zero is valid, result = 0");
    assert_eq!(result, Micros::ZERO);
}

#[test]
fn checked_mul_qty_with_negative_qty() {
    // qty = -1 for short-sell cost computation.
    let price = Micros::new(50 * 1_000_000); // $50
    let result = price.checked_mul_qty(-1).expect("negative qty is valid");
    assert_eq!(result.raw(), -50 * 1_000_000);
}

// ---------------------------------------------------------------------------
// 5. Ordering
// ---------------------------------------------------------------------------

#[test]
fn ordering_is_total() {
    let zero = Micros::ZERO;
    let pos = ONE_DOLLAR;
    let neg = -ONE_DOLLAR;

    assert!(neg < zero);
    assert!(zero < pos);
    assert!(neg < pos);
    assert_eq!(pos, pos);
}

#[test]
fn min_max_in_iter() {
    let amounts = [
        TEN_DOLLARS,
        Micros::new(3_000_000),
        ONE_DOLLAR,
        Micros::new(7_000_000),
    ];
    let min = amounts.iter().copied().min().unwrap();
    let max = amounts.iter().copied().max().unwrap();
    assert_eq!(min, ONE_DOLLAR);
    assert_eq!(max, TEN_DOLLARS);
}

// ---------------------------------------------------------------------------
// 6. Display
// ---------------------------------------------------------------------------

#[test]
fn display_positive() {
    let m = Micros::new(1_250_000); // $1.25
    assert_eq!(format!("{m}"), "1.250000");
}

#[test]
fn display_zero() {
    assert_eq!(format!("{}", Micros::ZERO), "0.000000");
}

#[test]
fn display_negative() {
    let m = Micros::new(-500_000); // -$0.50
    assert_eq!(format!("{m}"), "-0.500000");
}

// ---------------------------------------------------------------------------
// 7. Neg and AddAssign / SubAssign
// ---------------------------------------------------------------------------

#[test]
fn neg_roundtrips() {
    let a = Micros::new(42_000_000);
    assert_eq!(-(-a), a);
    assert_eq!(a + (-a), Micros::ZERO);
}

#[test]
fn add_assign_accumulates() {
    let mut total = Micros::ZERO;
    for _ in 0..5 {
        total += ONE_DOLLAR;
    }
    assert_eq!(total, Micros::new(5_000_000));
}

#[test]
fn sub_assign_drains() {
    let mut balance = TEN_DOLLARS;
    balance -= ONE_DOLLAR;
    balance -= ONE_DOLLAR;
    assert_eq!(balance, Micros::new(8_000_000));
}
