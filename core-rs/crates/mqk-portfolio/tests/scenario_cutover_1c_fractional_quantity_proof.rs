//! CUTOVER-1C — focused proof suite for the QtyMicros-native Fill/Lot/
//! Position/accounting migration (mission-required proof items A, B, C, E,
//! F, I; D/G/H live in mqk-testkit's end-to-end suite since they need the
//! OMS/reconcile/durable-replay seams this crate does not depend on).
//!
//! All scenarios are pure in-process: no DB, no network, no wall-clock reads.

use mqk_portfolio::{
    apply_entry, compute_unrealized_pnl_micros, marks, recompute_from_ledger,
    unrealized_pnl_micros, Fill, LedgerEntry, PortfolioState, QtyMicros, Side,
};

fn qty(n: i64) -> QtyMicros {
    QtyMicros::from_whole_units(n).unwrap()
}

// ---------------------------------------------------------------------------
// A — whole equity share round-trip: buy N, sell N, exact flat + cash + pnl.
// ---------------------------------------------------------------------------

#[test]
fn proof_a_whole_equity_share_round_trip_is_exact() {
    let mut pf = PortfolioState::new(100_000 * 1_000_000);

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, qty(10), 150 * 1_000_000, 0)),
    );
    let pos = pf.positions.get("AAPL").expect("AAPL position exists");
    assert_eq!(
        pos.qty_signed(),
        qty(10),
        "A: whole-share buy must round-trip to exactly 10 shares, no drift"
    );

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Sell, qty(10), 155 * 1_000_000, 0)),
    );
    assert!(
        pf.positions.get("AAPL").is_none() || pf.positions["AAPL"].qty_signed().is_zero(),
        "A: full round-trip sell must leave the position exactly flat"
    );
    assert_eq!(
        pf.realized_pnl_micros,
        10 * 5 * 1_000_000,
        "A: realized pnl must be exactly (155-150)*10 = $50"
    );
}

// ---------------------------------------------------------------------------
// B — 0.0001 BTC round-trip: the smallest representable Alpaca Crypto unit
// must survive a buy/sell round-trip with zero truncation.
// ---------------------------------------------------------------------------

#[test]
fn proof_b_fractional_btc_round_trip_is_exact() {
    let mut pf = PortfolioState::new(100_000 * 1_000_000);
    let btc_qty: QtyMicros = "0.0001".parse().unwrap();
    assert_eq!(btc_qty.raw(), 100, "B: 0.0001 BTC must be exactly 100 raw micros");

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("BTC/USD", Side::Buy, btc_qty, 50_000 * 1_000_000, 0)),
    );
    let pos = pf.positions.get("BTC/USD").expect("BTC/USD position exists");
    assert_eq!(
        pos.qty_signed(),
        btc_qty,
        "B: 0.0001 BTC buy must round-trip exactly, not truncate to zero"
    );
    assert!(
        pos.qty_signed().to_whole_units_checked().is_none(),
        "B: 0.0001 BTC is genuinely fractional -- it must NOT be whole-unit-representable"
    );

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("BTC/USD", Side::Sell, btc_qty, 51_000 * 1_000_000, 0)),
    );
    assert!(
        pf.positions.get("BTC/USD").is_none() || pf.positions["BTC/USD"].qty_signed().is_zero(),
        "B: full round-trip sell must leave the position exactly flat"
    );
}

// ---------------------------------------------------------------------------
// C — fractional partial-fill summation: 0.0001 + 0.00005 BTC = 0.00015 BTC
// exactly, proving checked-arithmetic summation carries no rounding drift.
// ---------------------------------------------------------------------------

#[test]
fn proof_c_fractional_partial_fill_summation_is_exact() {
    let mut pf = PortfolioState::new(100_000 * 1_000_000);

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(
            "BTC/USD",
            Side::Buy,
            "0.0001".parse().unwrap(),
            50_000 * 1_000_000,
            0,
        )),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(
            "BTC/USD",
            Side::Buy,
            "0.00005".parse().unwrap(),
            50_100 * 1_000_000,
            0,
        )),
    );

    let pos = pf.positions.get("BTC/USD").expect("BTC/USD position exists");
    let expected: QtyMicros = "0.00015".parse().unwrap();
    assert_eq!(
        pos.qty_signed(),
        expected,
        "C: 0.0001 + 0.00005 BTC must sum to exactly 0.00015 BTC"
    );
}

// ---------------------------------------------------------------------------
// E — fractional realized P&L: closing a fractional BTC position must
// realize gross P&L computed against the true fractional quantity, not a
// truncated/rounded whole-unit approximation.
// ---------------------------------------------------------------------------

#[test]
fn proof_e_fractional_realized_pnl_is_exact() {
    let mut pf = PortfolioState::new(100_000 * 1_000_000);
    let btc_qty: QtyMicros = "0.0001".parse().unwrap();

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("BTC/USD", Side::Buy, btc_qty, 50_000 * 1_000_000, 0)),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("BTC/USD", Side::Sell, btc_qty, 51_000 * 1_000_000, 0)),
    );

    // Realized = (51_000 - 50_000) * 0.0001 = $0.10 = 100_000 micros.
    assert_eq!(
        pf.realized_pnl_micros, 100_000,
        "E: fractional realized pnl must equal (51000-50000)*0.0001 = $0.10 exactly"
    );
}

// ---------------------------------------------------------------------------
// F — fractional unrealized P&L: mark-to-market on an open fractional
// position must compute against the exact fractional quantity.
// ---------------------------------------------------------------------------

#[test]
fn proof_f_fractional_unrealized_pnl_is_exact() {
    let btc_qty: QtyMicros = "0.0001".parse().unwrap();
    let avg_price_micros = 50_000 * 1_000_000;
    let mark_price_micros = 55_000 * 1_000_000;

    // Direct pure-function proof (avg/mark blended route).
    let pnl = unrealized_pnl_micros(btc_qty, avg_price_micros, mark_price_micros);
    assert_eq!(
        pnl, 500_000,
        "F: unrealized pnl must equal (55000-50000)*0.0001 = $0.50 exactly"
    );

    // Cross-check via the live-position route (compute_unrealized_pnl_micros).
    let mut pf = PortfolioState::new(100_000 * 1_000_000);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("BTC/USD", Side::Buy, btc_qty, avg_price_micros, 0)),
    );
    let mk = marks([("BTC/USD", mark_price_micros)]);
    let unreal = compute_unrealized_pnl_micros(&pf.positions, &mk);
    assert_eq!(
        unreal, 500_000,
        "F: live-position unrealized pnl must also equal $0.50 exactly"
    );
}

// ---------------------------------------------------------------------------
// H — restart/replay exactness: recompute_from_ledger (the same recompute
// path a durable restart/replay uses) must reproduce the exact fractional
// position/cash/realized-pnl state the live incremental apply produced --
// bit-for-bit, no drift from a second derivation of a fractional quantity.
// ---------------------------------------------------------------------------

#[test]
fn proof_h_restart_replay_reproduces_exact_fractional_state() {
    let mut pf = PortfolioState::new(100_000 * 1_000_000);

    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(
            "BTC/USD",
            Side::Buy,
            "0.0001".parse().unwrap(),
            50_000 * 1_000_000,
            1_000,
        )),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(
            "BTC/USD",
            Side::Buy,
            "0.00005".parse().unwrap(),
            50_500 * 1_000_000,
            500,
        )),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(
            "BTC/USD",
            Side::Sell,
            "0.00008".parse().unwrap(),
            51_000 * 1_000_000,
            800,
        )),
    );

    let (recomputed_cash, recomputed_realized, recomputed_positions) =
        recompute_from_ledger(pf.initial_cash_micros, &pf.ledger);

    assert_eq!(
        recomputed_cash, pf.cash_micros,
        "H: replayed cash_micros must exactly match the live incrementally-applied value"
    );
    assert_eq!(
        recomputed_realized, pf.realized_pnl_micros,
        "H: replayed realized_pnl_micros must exactly match the live incrementally-applied value"
    );
    assert_eq!(
        recomputed_positions, pf.positions,
        "H: replayed fractional position map must exactly match the live incrementally-applied \
         state -- restart/replay must never reinterpret or truncate a fractional quantity"
    );

    let pos = pf.positions.get("BTC/USD").expect("BTC/USD position exists");
    let expected_remaining: QtyMicros = "0.00007".parse().unwrap();
    assert_eq!(
        pos.qty_signed(),
        expected_remaining,
        "H: remaining fractional position (0.0001+0.00005-0.00008) must be exactly 0.00007 BTC"
    );
}

// ---------------------------------------------------------------------------
// I — overflow refused: QtyMicros arithmetic never silently wraps a
// quantity that cannot be represented; it fails closed (panics on a
// checked-arithmetic violation rather than producing a wrapped/truncated
// value that could misstate a real position).
// ---------------------------------------------------------------------------

#[test]
fn proof_i_qty_overflow_is_refused_not_wrapped() {
    // from_whole_units fails closed (None) rather than wrapping when the
    // whole-unit count cannot be scaled into QtyMicros's 1e-6 range.
    assert!(
        QtyMicros::from_whole_units(i64::MAX).is_none(),
        "I: from_whole_units must refuse an out-of-range whole-unit count, not wrap"
    );

    // checked_add fails closed (None) at the raw-i64 boundary rather than
    // wrapping to a nonsensical (possibly sign-flipped) quantity.
    let near_max = QtyMicros::new(i64::MAX);
    assert!(
        near_max.checked_add(QtyMicros::new(1)).is_none(),
        "I: checked_add must refuse to overflow QtyMicros's raw i64 range, not wrap"
    );

    // The portfolio's own summation path panics (fails closed) rather than
    // silently wrapping when accumulating lot quantities would overflow --
    // proven via catch_unwind so this test itself does not abort.
    let result = std::panic::catch_unwind(|| {
        let mut pf = PortfolioState::new(0);
        apply_entry(
            &mut pf,
            LedgerEntry::Fill(Fill::new(
                "OVERFLOW",
                Side::Buy,
                QtyMicros::new(i64::MAX),
                1,
                0,
            )),
        );
        apply_entry(
            &mut pf,
            LedgerEntry::Fill(Fill::new("OVERFLOW", Side::Buy, QtyMicros::new(1), 1, 0)),
        );
        // Force the checked-fold summation in qty_signed() to run.
        pf.positions["OVERFLOW"].qty_signed()
    });
    assert!(
        result.is_err(),
        "I: summing lot quantities past i64::MAX must fail closed (panic), never wrap to a \
         smaller/negative quantity that would misstate the true position"
    );
}
