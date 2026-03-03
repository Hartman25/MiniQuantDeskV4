//! Scenario: Fixed-point rounding boundaries for execution gate — M4-1
//!
//! Proves that `enforce_max_gross_exposure` uses exact integer (i64) comparison
//! with no f64 rounding ambiguity.  A one-micro difference in gross exposure is
//! always correctly detected by the gate.
//!
//! # Tests
//! 1. `exposure_at_exact_limit_passes`           — exposure == max → Ok (strict >)
//! 2. `exposure_one_micro_over_limit_is_breach`  — max = exposure - 1 → Err
//! 3. `exposure_one_micro_under_limit_passes`    — max = exposure + 1 → Ok
//! 4. `exposure_gate_is_deterministic`           — identical inputs → identical result
//! 5. `integer_gate_at_f64_precision_boundary`  — 1-micro difference correctly
//!    detected at 2^53 + 1, a value f64 cannot represent exactly

use mqk_portfolio::{
    apply_entry, enforce_max_gross_exposure, marks, Fill, LedgerEntry, PortfolioState, Side,
};

const M: i64 = 1_000_000; // micro-dollar scale factor

/// Build a portfolio with one share of `symbol` bought at $100.
/// Gross exposure under mark `mark_price` will be `1 * mark_price`.
fn build_portfolio(symbol: &str) -> PortfolioState {
    let mut pf = PortfolioState::new(100_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(symbol, Side::Buy, 1, 100 * M, 0)),
    );
    pf
}

// Gross exposure for a 1-share portfolio marked at BOUNDARY is BOUNDARY.
const BOUNDARY: i64 = 200 * M; // $200 = 200_000_000 micros

// ---------------------------------------------------------------------------
// 1. Exact boundary: not a breach (gate uses strict >)
// ---------------------------------------------------------------------------

#[test]
fn exposure_at_exact_limit_passes() {
    let pf = build_portfolio("AAPL");
    let m = marks([("AAPL", BOUNDARY)]);
    assert!(
        enforce_max_gross_exposure(&pf.positions, &m, BOUNDARY).is_ok(),
        "exposure == limit must not be a breach (gate is strict >)"
    );
}

// ---------------------------------------------------------------------------
// 2. One micro above: breach
// ---------------------------------------------------------------------------

#[test]
fn exposure_one_micro_over_limit_is_breach() {
    let pf = build_portfolio("AAPL");
    let m = marks([("AAPL", BOUNDARY)]);
    // max = BOUNDARY - 1: exposure exceeds limit by exactly 1 micro
    let err = enforce_max_gross_exposure(&pf.positions, &m, BOUNDARY - 1)
        .expect_err("exposure 1 micro above limit must be a breach");
    assert_eq!(err.gross_exposure_micros, BOUNDARY);
    assert_eq!(err.max_gross_exposure_micros, BOUNDARY - 1);
}

// ---------------------------------------------------------------------------
// 3. One micro below: not a breach
// ---------------------------------------------------------------------------

#[test]
fn exposure_one_micro_under_limit_passes() {
    let pf = build_portfolio("AAPL");
    let m = marks([("AAPL", BOUNDARY)]);
    // max = BOUNDARY + 1: exposure is 1 micro below limit
    assert!(
        enforce_max_gross_exposure(&pf.positions, &m, BOUNDARY + 1).is_ok(),
        "exposure 1 micro below limit must not be a breach"
    );
}

// ---------------------------------------------------------------------------
// 4. Determinism: same inputs → same output
// ---------------------------------------------------------------------------

#[test]
fn exposure_gate_is_deterministic() {
    let pf = build_portfolio("AAPL");
    let m = marks([("AAPL", BOUNDARY)]);
    let r1 = enforce_max_gross_exposure(&pf.positions, &m, BOUNDARY - 1);
    let r2 = enforce_max_gross_exposure(&pf.positions, &m, BOUNDARY - 1);
    assert_eq!(
        r1, r2,
        "gate must be deterministic: same inputs must produce same output"
    );
}

// ---------------------------------------------------------------------------
// 5. Integer gate correctly distinguishes values at the f64 precision boundary.
//
// 2^53 + 1 (= 9_007_199_254_740_993) is the first positive integer that f64
// cannot represent exactly: it rounds down to 2^53 in IEEE 754 round-to-even.
// A gate using f64 arithmetic would treat OVER_F64_PRECISION and
// OVER_F64_PRECISION - 1 as identical, failing to detect the 1-micro breach.
// The i64 gate must correctly detect it.
// ---------------------------------------------------------------------------

const OVER_F64_PRECISION: i64 = 9_007_199_254_740_993; // 2^53 + 1

#[test]
fn integer_gate_at_f64_precision_boundary() {
    // Verify the f64 precision loss that motivates this test.
    let as_f64: f64 = OVER_F64_PRECISION as f64;
    let roundtripped: i64 = as_f64 as i64;
    assert_eq!(
        roundtripped,
        OVER_F64_PRECISION - 1,
        "f64 cannot represent 2^53+1 exactly — proves why integer gate is required"
    );

    let pf = build_portfolio("BIGCO");
    let m = marks([("BIGCO", OVER_F64_PRECISION)]);

    // max = OVER_F64_PRECISION - 1: i64 detects breach; f64 would not
    assert!(
        enforce_max_gross_exposure(&pf.positions, &m, OVER_F64_PRECISION - 1).is_err(),
        "i64 gate must detect 1-micro breach at f64 precision boundary"
    );

    // max = OVER_F64_PRECISION: exactly at limit, not a breach
    assert!(
        enforce_max_gross_exposure(&pf.positions, &m, OVER_F64_PRECISION).is_ok(),
        "i64 gate must not falsely breach at exact limit"
    );
}
