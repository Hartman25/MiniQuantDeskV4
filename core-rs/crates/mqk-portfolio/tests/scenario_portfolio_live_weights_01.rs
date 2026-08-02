//! Scenario: PORTFOLIO-LIVE-WEIGHTS-01 — live position valuation & weight seam.
//!
//! Proves `compute_portfolio_weights` never fabricates a mark, NAV, or
//! weight, and is overflow-safe under `i128` arithmetic.
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                          |
//! |-------|--------------------------------------------------------------------------|
//! | PW-01 | Empty portfolio + cash → NAV == cash, truth_state = "active"           |
//! | PW-02 | Long position + mark → positive market value + positive weight_bps     |
//! | PW-03 | Short position + mark → negative market value + negative weight_bps    |
//! | PW-04 | Multiple positions → correct gross exposure + per-symbol weights       |
//! | PW-05 | Missing mark → truth_state = "missing_marks"; NAV/weights are None     |
//! | PW-06 | NAV <= 0 (negative and exact zero) → truth_state = "nav_unavailable"   |
//! | PW-07 | i64::MAX-magnitude qty * price does not panic / silently overflow      |
//! | PW-08 | Zero-quantity position ignores mark presence; market value is always 0|
//! | PW-09 | Determinism: identical inputs → byte-identical (`PartialEq`) output    |

use std::collections::BTreeMap;

use mqk_portfolio::{compute_portfolio_weights, PositionMark, PositionWeightInput};

const M: i64 = 1_000_000; // micro-dollar scale factor

fn pos(symbol: &str, signed_qty: i64) -> PositionWeightInput {
    PositionWeightInput {
        symbol: symbol.to_string(),
        signed_qty,
    }
}

fn mark(symbol: &str, price_micros: i64) -> PositionMark {
    PositionMark {
        symbol: symbol.to_string(),
        mark_price_micros: price_micros,
        mark_ts_utc: Some(1_700_000_000),
        source: "test:manual".to_string(),
    }
}

fn marks_map(items: &[(&str, i64)]) -> BTreeMap<String, PositionMark> {
    items
        .iter()
        .map(|(sym, px)| (sym.to_string(), mark(sym, *px)))
        .collect()
}

// ---------------------------------------------------------------------------
// PW-01: empty portfolio + cash → NAV == cash, active
// ---------------------------------------------------------------------------

#[test]
fn pw01_empty_portfolio_nav_equals_cash() {
    let cash = 1000 * M;
    let snap = compute_portfolio_weights(cash, &[], &BTreeMap::new());

    assert_eq!(snap.truth_state, "active");
    assert_eq!(snap.nav_micros, Some(cash as i128));
    assert_eq!(snap.gross_exposure_micros, Some(0));
    assert_eq!(snap.cash_micros, cash);
    assert!(snap.positions.is_empty());
    assert!(snap.missing_mark_symbols.is_empty());
}

// ---------------------------------------------------------------------------
// PW-02: long position + mark → positive market value + positive weight
// ---------------------------------------------------------------------------

#[test]
fn pw02_long_position_positive_market_value_and_weight() {
    let cash = 50 * M;
    let positions = vec![pos("AAPL", 10)];
    let marks = marks_map(&[("AAPL", 10 * M)]); // $10/share

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "active");
    assert_eq!(snap.nav_micros, Some(150 * M as i128)); // 50 cash + 100 mv
    assert_eq!(snap.gross_exposure_micros, Some(100 * M as i128));
    assert_eq!(snap.positions.len(), 1);
    let row = &snap.positions[0];
    assert_eq!(row.market_value_micros, Some(100 * M as i128));
    assert_eq!(row.absolute_notional_micros, Some(100 * M as i128));
    assert_eq!(row.weight_bps, Some(6666)); // 100/150 truncated to 4dp bps
    assert!(!row.missing_mark);
}

// ---------------------------------------------------------------------------
// PW-03: short position + mark → negative market value + negative weight
// ---------------------------------------------------------------------------

#[test]
fn pw03_short_position_negative_market_value_and_weight() {
    let cash = 200 * M;
    let positions = vec![pos("TLT", -5)];
    let marks = marks_map(&[("TLT", 20 * M)]); // $20/share

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "active");
    assert_eq!(snap.nav_micros, Some(100 * M as i128)); // 200 cash - 100 mv
    assert_eq!(snap.gross_exposure_micros, Some(100 * M as i128));
    let row = &snap.positions[0];
    assert_eq!(row.market_value_micros, Some(-100 * M as i128));
    assert_eq!(row.absolute_notional_micros, Some(100 * M as i128));
    assert_eq!(row.weight_bps, Some(-10000)); // exactly -100% of NAV
    assert!(!row.missing_mark);
}

// ---------------------------------------------------------------------------
// PW-04: multiple positions → gross exposure + per-symbol weights
// ---------------------------------------------------------------------------

#[test]
fn pw04_multiple_positions_gross_exposure_and_weights() {
    let cash = 100 * M;
    let positions = vec![pos("AAPL", 2), pos("MSFT", -1)];
    let marks = marks_map(&[("AAPL", 50 * M), ("MSFT", 100 * M)]);

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "active");
    // cash 100 + AAPL mv 100 (2*50) + MSFT mv -100 (-1*100) = 100
    assert_eq!(snap.nav_micros, Some(100 * M as i128));
    // gross = |100| + |-100| = 200
    assert_eq!(snap.gross_exposure_micros, Some(200 * M as i128));

    let aapl = snap.positions.iter().find(|r| r.symbol == "AAPL").unwrap();
    assert_eq!(aapl.market_value_micros, Some(100 * M as i128));
    assert_eq!(aapl.weight_bps, Some(10000)); // exactly 100% of NAV

    let msft = snap.positions.iter().find(|r| r.symbol == "MSFT").unwrap();
    assert_eq!(msft.market_value_micros, Some(-100 * M as i128));
    assert_eq!(msft.weight_bps, Some(-10000)); // exactly -100% of NAV
}

// ---------------------------------------------------------------------------
// PW-05: missing mark → truth_state = "missing_marks"; NAV/weights are None
// ---------------------------------------------------------------------------

#[test]
fn pw05_missing_mark_blocks_nav_and_weights_lists_symbol() {
    let cash = 100 * M;
    let positions = vec![pos("AAPL", 1), pos("MSFT", 1)];
    let marks = marks_map(&[("AAPL", 10 * M)]); // MSFT has no mark

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "missing_marks");
    assert_eq!(snap.nav_micros, None);
    assert_eq!(snap.gross_exposure_micros, None);
    assert_eq!(snap.missing_mark_symbols, vec!["MSFT".to_string()]);

    let aapl = snap.positions.iter().find(|r| r.symbol == "AAPL").unwrap();
    assert_eq!(
        aapl.market_value_micros,
        Some(10 * M as i128),
        "known marks are still surfaced even though the snapshot-wide NAV is blocked"
    );
    assert_eq!(
        aapl.weight_bps, None,
        "no weight is ever computed without a confirmed NAV"
    );
    assert!(!aapl.missing_mark);

    let msft = snap.positions.iter().find(|r| r.symbol == "MSFT").unwrap();
    assert_eq!(msft.market_value_micros, None);
    assert_eq!(msft.weight_bps, None);
    assert!(msft.missing_mark);
}

// ---------------------------------------------------------------------------
// PW-06: NAV <= 0 → truth_state = "nav_unavailable"
// ---------------------------------------------------------------------------

#[test]
fn pw06_negative_nav_blocks_weights_but_reports_nav_and_gross() {
    let cash = -1000 * M;
    let positions = vec![pos("AAPL", 1)];
    let marks = marks_map(&[("AAPL", 10 * M)]);

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "nav_unavailable");
    assert_eq!(snap.nav_micros, Some(-990 * M as i128));
    assert_eq!(
        snap.gross_exposure_micros,
        Some(10 * M as i128),
        "gross exposure is still well-defined; only weights require NAV > 0"
    );
    let row = &snap.positions[0];
    assert_eq!(row.market_value_micros, Some(10 * M as i128));
    assert_eq!(row.weight_bps, None);
}

#[test]
fn pw06b_exact_zero_nav_blocks_weights() {
    let cash = -10 * M;
    let positions = vec![pos("AAPL", 1)];
    let marks = marks_map(&[("AAPL", 10 * M)]);

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(
        snap.truth_state, "nav_unavailable",
        "NAV == 0 must be treated the same as NAV < 0 (gate is <=)"
    );
    assert_eq!(snap.nav_micros, Some(0));
    assert_eq!(snap.positions[0].weight_bps, None);
}

// ---------------------------------------------------------------------------
// PW-07: i64::MAX-magnitude qty * price does not panic / silently overflow
// ---------------------------------------------------------------------------

#[test]
fn pw07_extreme_magnitude_is_overflow_safe_and_exact() {
    let qty = i64::MAX;
    let price = i64::MAX;
    let expected_mv = (qty as i128) * (price as i128);

    let positions = vec![pos("ZMAX", qty)];
    let marks = marks_map(&[("ZMAX", price)]);

    // cash = 0 so NAV == this single position's market value exactly.
    let snap = compute_portfolio_weights(0, &positions, &marks);

    assert_eq!(snap.truth_state, "active");
    assert_eq!(snap.nav_micros, Some(expected_mv));
    assert_eq!(snap.gross_exposure_micros, Some(expected_mv));
    let row = &snap.positions[0];
    assert_eq!(row.market_value_micros, Some(expected_mv));
    assert_eq!(
        row.weight_bps,
        Some(10000),
        "a single position that equals the entire NAV must be exactly 100%, \
         even when the intermediate mv*10_000 product would overflow i128"
    );
}

// ---------------------------------------------------------------------------
// PW-08: zero-quantity position ignores mark presence; market value is 0
// ---------------------------------------------------------------------------

#[test]
fn pw08_zero_quantity_position_is_predictable_without_a_mark() {
    let cash = 100 * M;
    let positions = vec![pos("FLAT", 0)];

    let snap = compute_portfolio_weights(cash, &positions, &BTreeMap::new());

    assert_eq!(
        snap.truth_state, "active",
        "a flat position with no mark must not force missing_marks"
    );
    assert!(snap.missing_mark_symbols.is_empty());
    let row = &snap.positions[0];
    assert_eq!(row.market_value_micros, Some(0));
    assert_eq!(row.absolute_notional_micros, Some(0));
    assert_eq!(row.weight_bps, Some(0));
    assert!(!row.missing_mark);
    assert_eq!(row.mark_price_micros, None);
}

#[test]
fn pw08b_zero_quantity_position_ignores_an_available_mark() {
    let cash = 100 * M;
    let positions = vec![pos("FLAT2", 0)];
    let marks = marks_map(&[("FLAT2", 999 * M)]);

    let snap = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap.truth_state, "active");
    let row = &snap.positions[0];
    assert_eq!(
        row.market_value_micros,
        Some(0),
        "zero quantity means zero market value regardless of price"
    );
    assert_eq!(row.weight_bps, Some(0));
    // Provenance is still echoed for operator visibility even though it had no effect.
    assert_eq!(row.mark_price_micros, Some(999 * M));
}

// ---------------------------------------------------------------------------
// PW-09: determinism — identical inputs → byte-identical output
// ---------------------------------------------------------------------------

#[test]
fn pw09_computation_is_deterministic() {
    let cash = 321 * M;
    let positions = vec![pos("AAPL", 7), pos("TLT", -3)];
    let marks = marks_map(&[("AAPL", 11 * M), ("TLT", 13 * M)]);

    let snap1 = compute_portfolio_weights(cash, &positions, &marks);
    let snap2 = compute_portfolio_weights(cash, &positions, &marks);

    assert_eq!(snap1, snap2);
}
