//! Scenario: Capital Conservation Invariant Suite — I9-1
//!
//! # Invariant under test
//!
//! ```text
//! equity(marks) == initial_cash − total_fees_paid + realized_pnl + unrealized_pnl(marks)
//! ```
//!
//! This invariant must hold after EVERY portfolio mutation — partial fills,
//! late fills, duplicate events — with no silent drift between the incremental
//! update path and the authoritative ledger-replay path.
//!
//! All scenarios are pure portfolio accounting: no DB, no IO, no wall-clock time,
//! no randomness.
//!
//! ## S1 — Partial fills
//!
//! Fill a 10-share buy order in two batches of 5.  Assert conservation after
//! each partial fill and after the final closing sell.
//!
//! ## S2 — Late fill after cancel
//!
//! A 10-share buy is partially filled (5 shares).  A cancel is issued at the
//! broker, but from the portfolio's perspective a cancel is a broker-level event
//! that does NOT mutate portfolio state.  A "late fill" for the remaining 5
//! shares arrives anyway (the cancel message arrived too late at the broker).
//! Assert conservation after each fill.
//!
//! ## S3 — Replace-reject then fill
//!
//! A replace request is rejected — a broker-level no-op that does not mutate
//! portfolio state.  The original order then fills in full.  Assert conservation.
//!
//! ## S4 — Duplicate fill not double-counted
//!
//! The Ledger's seq-no ordering guard rejects a second application of the same
//! fill (same seq_no), returning `LedgerError::OutOfOrderSeqNo`.  State must be
//! completely unchanged after the rejected duplicate, and `verify_integrity`
//! must still pass.

use mqk_portfolio::{
    apply_entry, compute_equity_micros, marks, Fill, Ledger, LedgerEntry, LedgerError,
    PortfolioState, Side, MICROS_SCALE,
};
use mqk_testkit::assert_capital_conservation;

// ── Constants ────────────────────────────────────────────────────────────────

const INITIAL_CASH: i64 = 100_000 * MICROS_SCALE; // $100,000
const SPY_PRICE: i64 = 100 * MICROS_SCALE; // $100.00
const SPY_PRICE_HI: i64 = 110 * MICROS_SCALE; // $110.00
const FEE: i64 = 500_000; // $0.50 per fill

// ── S1: Partial fills ─────────────────────────────────────────────────────────

/// Fill a 10-share buy order in two batches of 5.
/// Conservation must hold after each partial fill and after the closing sell.
#[test]
fn partial_fill_conservation() {
    let mut state = PortfolioState::new(INITIAL_CASH);
    let m_at_cost = marks([("SPY", SPY_PRICE)]);

    // Partial fill 1: buy 5 SPY @ $100, no fee.
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Buy, 5, SPY_PRICE, 0)),
    );
    assert_capital_conservation(
        &state,
        &m_at_cost,
        "S1: after partial fill 1 — buy 5 @ $100",
    );

    // Partial fill 2: buy remaining 5 SPY @ $100, no fee.
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Buy, 5, SPY_PRICE, 0)),
    );
    assert_capital_conservation(
        &state,
        &m_at_cost,
        "S1: after partial fill 2 — buy 10 total @ $100",
    );

    // Mark moves to $110 — unrealized PnL increases; no fill yet.
    let m_hi = marks([("SPY", SPY_PRICE_HI)]);
    assert_capital_conservation(&state, &m_hi, "S1: marks at $110 before close");

    // Closing sell: sell all 10 SPY @ $110, no fee.
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Sell, 10, SPY_PRICE_HI, 0)),
    );
    assert_capital_conservation(&state, &m_hi, "S1: after closing sell — sell 10 @ $110");

    // Sanity: realized = qty * (sell - buy); no fees paid.
    let expected_realized = 10 * (SPY_PRICE_HI - SPY_PRICE);
    assert_eq!(
        state.realized_pnl_micros, expected_realized,
        "S1: realized PnL must equal 10 * ($110 - $100)"
    );

    // Equity after close = initial + realized (flat portfolio, no fees).
    let equity = compute_equity_micros(state.cash_micros, &state.positions, &m_hi);
    assert_eq!(
        equity,
        INITIAL_CASH + expected_realized,
        "S1: equity after round-trip (no fees) = initial + realized_pnl"
    );
}

// ── S2: Late fill after cancel ────────────────────────────────────────────────

/// A 10-share buy is partially filled (5 shares).  A cancel is issued at the
/// broker — no portfolio state change.  A late fill for the remaining 5 shares
/// arrives at a different price.  Conservation must hold throughout.
#[test]
fn late_fill_after_cancel_conservation() {
    let mut state = PortfolioState::new(INITIAL_CASH);
    let m = marks([("SPY", SPY_PRICE)]);

    // Partial fill: buy 5 SPY @ $100 with fee.
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Buy, 5, SPY_PRICE, FEE)),
    );
    assert_capital_conservation(
        &state,
        &m,
        "S2: after partial fill (5 shares @ $100 + fee) — cancel pending",
    );

    // Cancel issued — broker-level event only.  Portfolio state unchanged.
    // No apply_entry call here.

    // Late fill: remaining 5 SPY fill at $102 (cancel arrived too late).
    let late_price = 102 * MICROS_SCALE;
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Buy, 5, late_price, FEE)),
    );
    let m2 = marks([("SPY", late_price)]);
    assert_capital_conservation(&state, &m2, "S2: after late fill (5 more @ $102 + fee)");

    // Close: sell all 10 SPY @ $105 with fee.
    let close_price = 105 * MICROS_SCALE;
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Sell, 10, close_price, FEE)),
    );
    let m3 = marks([("SPY", close_price)]);
    assert_capital_conservation(
        &state,
        &m3,
        "S2: after close sell (10 @ $105 + fee) — 3 fees total",
    );

    // Sanity: 3 fees paid, portfolio flat.
    assert!(
        state.positions.is_empty(),
        "S2: portfolio must be flat after close"
    );
    let equity = compute_equity_micros(state.cash_micros, &state.positions, &m3);
    let total_fees = 3 * FEE;
    let realized = state.realized_pnl_micros;
    assert_eq!(
        equity,
        INITIAL_CASH - total_fees + realized,
        "S2: equity = initial - fees + realized (flat portfolio)"
    );
}

// ── S3: Replace-reject then fill ─────────────────────────────────────────────

/// A replace request is rejected by the broker — a no-op from the portfolio's
/// perspective.  The original order then fills in full.  Conservation must hold.
#[test]
fn replace_reject_then_fill_conservation() {
    let mut state = PortfolioState::new(INITIAL_CASH);
    let m = marks([("SPY", SPY_PRICE)]);

    // Initial state: replace-reject is a broker-level no-op.  Nothing to apply.
    assert_capital_conservation(
        &state,
        &m,
        "S3: initial state — replace-reject is a portfolio no-op",
    );

    // Original order fills in full: buy 10 SPY @ $100 with fee.
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Buy, 10, SPY_PRICE, FEE)),
    );
    assert_capital_conservation(&state, &m, "S3: after original fill (10 @ $100 + fee)");

    // Close: sell all 10 SPY @ $108 with fee.
    let close_price = 108 * MICROS_SCALE;
    apply_entry(
        &mut state,
        LedgerEntry::Fill(Fill::new("SPY", Side::Sell, 10, close_price, FEE)),
    );
    let m2 = marks([("SPY", close_price)]);
    assert_capital_conservation(&state, &m2, "S3: after close sell (10 @ $108 + fee)");

    // Sanity: 2 fees, realized = 10 * ($108 - $100), flat portfolio.
    assert!(
        state.positions.is_empty(),
        "S3: portfolio must be flat after close"
    );
    let expected_realized = 10 * (close_price - SPY_PRICE);
    assert_eq!(
        state.realized_pnl_micros, expected_realized,
        "S3: realized PnL must equal 10 * ($108 - $100)"
    );
    let equity = compute_equity_micros(state.cash_micros, &state.positions, &m2);
    let total_fees = 2 * FEE;
    assert_eq!(
        equity,
        INITIAL_CASH - total_fees + expected_realized,
        "S3: equity = initial - fees + realized"
    );
}

// ── S4: Duplicate fill not double-counted ────────────────────────────────────

/// The Ledger seq-no guard rejects a duplicate fill (same seq_no).
/// State must be completely unchanged after the rejected duplicate, and
/// `verify_integrity` must still pass.
#[test]
fn duplicate_fill_not_double_counted() {
    let mut ledger = Ledger::new(INITIAL_CASH);
    let m = marks([("SPY", SPY_PRICE)]);

    let fill = Fill::new("SPY", Side::Buy, 5, SPY_PRICE, 0);

    // First application — seq_no 1: must succeed.
    ledger
        .append_fill_seq(fill.clone(), 1)
        .expect("S4: seq_no=1 must succeed");

    // At fill price with no fee, equity is unchanged from initial_cash.
    assert_eq!(
        ledger.equity_micros(&m),
        INITIAL_CASH,
        "S4: equity at fill price (no fee) must equal initial_cash"
    );
    assert!(
        ledger.verify_integrity(),
        "S4: verify_integrity must pass after first fill"
    );

    let snap_after_first = ledger.snapshot();

    // Duplicate application — same seq_no 1: must be rejected.
    let dup_result = ledger.append_fill_seq(fill.clone(), 1);
    assert!(
        matches!(
            dup_result,
            Err(LedgerError::OutOfOrderSeqNo { supplied: 1, .. })
        ),
        "S4: duplicate seq_no=1 must return OutOfOrderSeqNo, got: {:?}",
        dup_result
    );

    // State must be completely unchanged after the rejected duplicate.
    let snap_after_dup = ledger.snapshot();
    assert_eq!(
        snap_after_dup.cash_micros, snap_after_first.cash_micros,
        "S4: cash must not change after rejected duplicate"
    );
    assert_eq!(
        snap_after_dup.realized_pnl_micros, snap_after_first.realized_pnl_micros,
        "S4: realized_pnl must not change after rejected duplicate"
    );
    assert_eq!(
        snap_after_dup.positions, snap_after_first.positions,
        "S4: positions must not change after rejected duplicate"
    );
    assert_eq!(
        snap_after_dup.entry_count, 1,
        "S4: exactly 1 ledger entry — the original fill, not the duplicate"
    );
    assert_eq!(
        snap_after_dup.last_seq_no, 1,
        "S4: last_seq_no must remain 1 after rejected duplicate"
    );

    // Equity still equals initial_cash (buy at mark price, no fee, no dup applied).
    assert_eq!(
        ledger.equity_micros(&m),
        INITIAL_CASH,
        "S4: equity must be unchanged after rejected duplicate"
    );
    assert!(
        ledger.verify_integrity(),
        "S4: verify_integrity must pass after rejected duplicate"
    );

    // A second distinct fill with seq_no=2 must succeed and change state.
    ledger
        .append_fill_seq(fill.clone(), 2)
        .expect("S4: seq_no=2 must succeed");

    assert_eq!(
        ledger.snapshot().entry_count,
        2,
        "S4: 2 entries in ledger after seq_no=2 fill"
    );
    assert!(
        ledger.verify_integrity(),
        "S4: verify_integrity must pass after seq_no=2 fill"
    );
}
