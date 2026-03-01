//! M4-2: Conservation Invariant scenario tests
//!
//! Verifies the fundamental bookkeeping identity at every observable
//! checkpoint:
//!
//!   `equity(mark) = initial_cash + realized_pnl + unrealized_pnl(mark) − total_fees`
//!
//! All tests are external-perspective.  They only exercise the public
//! `Ledger` API and verify the observable accounting identity; no internal
//! ledger fields are touched.

use mqk_portfolio::{marks, Fill, Ledger, MarkMap, Side, MICROS_SCALE};

const M: i64 = MICROS_SCALE;

// ---------------------------------------------------------------------------
// Assertion helper
// ---------------------------------------------------------------------------

/// Assert the conservation identity:
///   equity(mark) == initial + realized_pnl + unrealized_pnl(mark) - total_fees
///
/// Panics on violation with a diagnostic showing all constituent values.
fn assert_conservation(ledger: &Ledger, mark_map: &MarkMap, initial: i64, total_fees: i64) {
    let equity = ledger.equity_micros(mark_map);
    let realized = ledger.realized_pnl_micros();
    let unrealized = ledger.unrealized_pnl_micros(mark_map);
    let expected = initial + realized + unrealized - total_fees;
    assert_eq!(
        equity, expected,
        "conservation violated: equity={equity} != \
         initial({initial}) + realized({realized}) + unrealized({unrealized}) \
         - fees({total_fees}) = {expected}"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Marking at cost price immediately after a buy must recover initial capital.
#[test]
fn mark_at_cost_recovers_initial_capital() {
    let initial = 100_000 * M;
    let mut ledger = Ledger::new(initial);

    ledger
        .append_fill(Fill::new("SPY", Side::Buy, 10, 100 * M, 0))
        .unwrap();

    let mk = marks([("SPY", 100 * M)]);
    assert_conservation(&ledger, &mk, initial, 0);
    assert!(ledger.verify_integrity());
}

/// After a full round-trip (buy + sell all shares, no fees), the ledger is
/// flat and cash equals initial_cash + realized_pnl.
#[test]
fn round_trip_no_fee_cash_equals_initial_plus_realized() {
    let initial = 100_000 * M;
    let mut ledger = Ledger::new(initial);

    ledger
        .append_fill(Fill::new("SPY", Side::Buy, 10, 100 * M, 0))
        .unwrap();
    ledger
        .append_fill(Fill::new("SPY", Side::Sell, 10, 120 * M, 0))
        .unwrap();

    assert!(ledger.is_flat());
    let realized = ledger.realized_pnl_micros();
    assert_eq!(realized, 200 * M); // (120 - 100) * 10
    assert_eq!(ledger.cash_micros(), initial + realized);
    assert!(ledger.verify_integrity());
}

/// Loss trade: realized_pnl is negative and cash is reduced by the loss.
#[test]
fn loss_trade_cash_reduced_by_loss() {
    let initial = 100_000 * M;
    let mut ledger = Ledger::new(initial);

    ledger
        .append_fill(Fill::new("SPY", Side::Buy, 10, 100 * M, 0))
        .unwrap();
    ledger
        .append_fill(Fill::new("SPY", Side::Sell, 10, 90 * M, 0))
        .unwrap();

    assert!(ledger.is_flat());
    let realized = ledger.realized_pnl_micros();
    assert_eq!(realized, -100 * M); // (90 - 100) * 10
    assert_eq!(ledger.cash_micros(), initial + realized); // cash fell by the loss
    assert!(ledger.verify_integrity());
}

/// Fees are a permanent one-way deduction — total equity falls by exactly
/// the sum of all fees paid, regardless of trade direction.
#[test]
fn fees_reduce_equity_by_exact_total() {
    let initial = 100_000 * M;
    let fee = M; // $1.00 per fill
    let mut ledger = Ledger::new(initial);
    let mut total_fees: i64 = 0;

    ledger
        .append_fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, fee))
        .unwrap();
    total_fees += fee;

    ledger
        .append_fill(Fill::new("AAPL", Side::Sell, 10, 110 * M, fee))
        .unwrap();
    total_fees += fee;

    assert!(ledger.is_flat());
    let realized = ledger.realized_pnl_micros();
    assert_eq!(realized, 100 * M); // gross gain before fees
    assert_eq!(ledger.cash_micros(), initial + realized - total_fees);

    let mk = marks([("AAPL", 110 * M)]); // flat, unrealized = 0
    assert_conservation(&ledger, &mk, initial, total_fees);
    assert!(ledger.verify_integrity());
}

/// The conservation identity holds at every checkpoint in a multi-step
/// sequence: open, mark-move, add to position, partial close, full close.
#[test]
fn conservation_holds_at_every_checkpoint() {
    let initial = 50_000 * M;
    let mut ledger = Ledger::new(initial);
    let mut total_fees: i64 = 0;
    let fee = 500_000_i64; // $0.50 per fill
    let sym = "SPY";

    // --- step 1: buy 5 @ $100, fee = $0.50 ---
    ledger
        .append_fill(Fill::new(sym, Side::Buy, 5, 100 * M, fee))
        .unwrap();
    total_fees += fee;
    assert_conservation(&ledger, &marks([(sym, 100 * M)]), initial, total_fees);

    // --- step 2: same position, mark moves to $105 ---
    assert_conservation(&ledger, &marks([(sym, 105 * M)]), initial, total_fees);

    // --- step 3: buy 5 more @ $105, fee = $0.50 ---
    ledger
        .append_fill(Fill::new(sym, Side::Buy, 5, 105 * M, fee))
        .unwrap();
    total_fees += fee;
    assert_conservation(&ledger, &marks([(sym, 105 * M)]), initial, total_fees);

    // --- step 4: sell 5 @ $110, fee = $0.50; FIFO closes first lot (cost $100) ---
    ledger
        .append_fill(Fill::new(sym, Side::Sell, 5, 110 * M, fee))
        .unwrap();
    total_fees += fee;
    assert_eq!(ledger.realized_pnl_micros(), 50 * M); // 5*(110-100)
    assert_conservation(&ledger, &marks([(sym, 110 * M)]), initial, total_fees);

    // --- step 5: sell remaining 5 @ $110, fee = $0.50; closes second lot (cost $105) ---
    ledger
        .append_fill(Fill::new(sym, Side::Sell, 5, 110 * M, fee))
        .unwrap();
    total_fees += fee;
    assert_eq!(ledger.realized_pnl_micros(), 75 * M); // 50 + 5*(110-105)
    assert!(ledger.is_flat());
    assert_conservation(&ledger, &marks([(sym, 110 * M)]), initial, total_fees);

    assert!(ledger.verify_integrity());
}

/// Two independent symbols — combined equity identity holds with gains on one
/// and losses on the other symbol simultaneously.
#[test]
fn two_symbol_combined_equity_identity() {
    let initial = 200_000 * M;
    let mut ledger = Ledger::new(initial);

    ledger
        .append_fill(Fill::new("AAPL", Side::Buy, 10, 150 * M, 0))
        .unwrap();
    ledger
        .append_fill(Fill::new("MSFT", Side::Buy, 20, 100 * M, 0))
        .unwrap();

    // AAPL: mark $160 (+$10/share), MSFT: mark $95 (-$5/share)
    // net unrealized = 10*(160-150) + 20*(95-100) = 100 - 100 = 0
    let mk = marks([("AAPL", 160 * M), ("MSFT", 95 * M)]);
    assert_eq!(ledger.unrealized_pnl_micros(&mk), 0);
    assert_conservation(&ledger, &mk, initial, 0);
    assert!(ledger.verify_integrity());
}

/// Partial close — some lots remain open; conservation identity holds both
/// before and after the partial sell.
#[test]
fn partial_close_conservation_holds() {
    let initial = 100_000 * M;
    let mut ledger = Ledger::new(initial);

    // Buy 20 @ $200
    ledger
        .append_fill(Fill::new("GOOG", Side::Buy, 20, 200 * M, 0))
        .unwrap();
    let mk_before = marks([("GOOG", 200 * M)]);
    assert_conservation(&ledger, &mk_before, initial, 0);

    // Sell 8 @ $220 — FIFO closes first 8 lots; 12 remain at cost $200
    ledger
        .append_fill(Fill::new("GOOG", Side::Sell, 8, 220 * M, 0))
        .unwrap();
    assert_eq!(ledger.realized_pnl_micros(), 160 * M); // 8*(220-200)
    assert_eq!(ledger.qty_signed("GOOG"), 12);

    let mk_after = marks([("GOOG", 220 * M)]);
    let unrealized = ledger.unrealized_pnl_micros(&mk_after);
    assert_eq!(unrealized, 240 * M); // 12*(220-200)
    assert_conservation(&ledger, &mk_after, initial, 0);
    assert!(ledger.verify_integrity());
}
