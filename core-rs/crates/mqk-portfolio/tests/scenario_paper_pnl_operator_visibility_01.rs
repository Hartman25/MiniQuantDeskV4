//! Scenario: PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01B — unrealized P&L
//! from a single blended average cost basis + mark price.
//!
//! Closes the seam `PAPER-TRADE-LIFECYCLE-PROOF-02` exposed: a real filled
//! paper position (`AAPL qty=3 avg_price=314.81`) had `mark_price` and
//! `unrealized_pnl` both `null` on `/api/v1/portfolio/positions` because no
//! pure helper existed to combine a broker-snapshot `avg_price` with a
//! `md_bars`-sourced mark. `unrealized_pnl_micros` is that helper.
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                    |
//! |-------|--------------------------------------------------------------------|
//! | PNL-01| Long position, mark above avg -> positive P&L                    |
//! | PNL-02| Long position, mark below avg -> negative P&L                    |
//! | PNL-03| Short position, mark above avg -> negative P&L                   |
//! | PNL-04| Short position, mark below avg -> positive P&L                   |
//! | PNL-05| Flat position -> always zero regardless of avg/mark              |
//! | PNL-06| Mark equal to avg -> zero P&L                                    |
//! | PNL-07| i64::MAX-magnitude qty * price does not panic / silently overflow|
//! | PNL-08| Real proof-02 fill (AAPL qty=3 avg_price=314.81) computes cleanly |

use mqk_portfolio::unrealized_pnl_micros;

const M: i64 = 1_000_000; // micro-dollar scale factor

// ---------------------------------------------------------------------------
// PNL-01 / PNL-02: long position
// ---------------------------------------------------------------------------

#[test]
fn pnl01_long_position_mark_above_avg_is_positive() {
    let pnl = unrealized_pnl_micros(10, 100 * M, 110 * M);
    assert_eq!(pnl, 100 * M as i128);
}

#[test]
fn pnl02_long_position_mark_below_avg_is_negative() {
    let pnl = unrealized_pnl_micros(10, 100 * M, 90 * M);
    assert_eq!(pnl, -100 * M as i128);
}

// ---------------------------------------------------------------------------
// PNL-03 / PNL-04: short position
// ---------------------------------------------------------------------------

#[test]
fn pnl03_short_position_mark_above_avg_is_negative() {
    // Short 10 shares (signed_qty = -10): a rising mark is a loss.
    let pnl = unrealized_pnl_micros(-10, 100 * M, 110 * M);
    assert_eq!(pnl, -100 * M as i128);
}

#[test]
fn pnl04_short_position_mark_below_avg_is_positive() {
    // Short 10 shares: a falling mark is a gain.
    let pnl = unrealized_pnl_micros(-10, 100 * M, 90 * M);
    assert_eq!(pnl, 100 * M as i128);
}

// ---------------------------------------------------------------------------
// PNL-05 / PNL-06: edge cases
// ---------------------------------------------------------------------------

#[test]
fn pnl05_flat_position_is_always_zero() {
    assert_eq!(unrealized_pnl_micros(0, 100 * M, 999 * M), 0);
    assert_eq!(unrealized_pnl_micros(0, 0, 0), 0);
}

#[test]
fn pnl06_mark_equal_avg_is_zero() {
    assert_eq!(unrealized_pnl_micros(5, 314_810_000, 314_810_000), 0);
}

// ---------------------------------------------------------------------------
// PNL-07: overflow safety
// ---------------------------------------------------------------------------

#[test]
fn pnl07_extreme_magnitudes_do_not_overflow_i128() {
    let pnl = unrealized_pnl_micros(i64::MAX, 0, i64::MAX);
    let expected = (i64::MAX as i128) * (i64::MAX as i128);
    assert_eq!(pnl, expected);
}

// ---------------------------------------------------------------------------
// PNL-08: real proof-02 fill scenario
// ---------------------------------------------------------------------------

#[test]
fn pnl08_proof_02_real_fill_computes_positive_pnl_when_mark_above_avg() {
    // PAPER-TRADE-LIFECYCLE-PROOF-02 §12: real filled position
    // `{"symbol":"AAPL","qty":3,"avg_price":314.81,...}`.
    let avg_price_micros = 314_810_000i64;
    let qty = 3i64;
    let mark_price_micros = 320_000_000i64; // hypothetical mark for the test
    let pnl = unrealized_pnl_micros(qty, avg_price_micros, mark_price_micros);
    assert_eq!(pnl, (mark_price_micros as i128 - avg_price_micros as i128) * qty as i128);
    assert!(pnl > 0, "mark above avg_price must yield positive unrealized pnl");
}

#[test]
fn pnl08b_proof_02_real_fill_computes_negative_pnl_when_mark_below_avg() {
    let avg_price_micros = 314_810_000i64;
    let qty = 3i64;
    let mark_price_micros = 310_000_000i64;
    let pnl = unrealized_pnl_micros(qty, avg_price_micros, mark_price_micros);
    assert!(pnl < 0, "mark below avg_price must yield negative unrealized pnl");
}
