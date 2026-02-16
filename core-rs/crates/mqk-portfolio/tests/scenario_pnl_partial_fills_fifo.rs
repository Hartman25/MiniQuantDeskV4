use mqk_portfolio::{
    apply_entry,
    recompute_from_ledger,
    compute_equity_micros,
    compute_exposure_micros,
    compute_unrealized_pnl_micros,
    Fill,
    LedgerEntry,
    PortfolioState,
    Side,
    marks,
};

const M: i64 = 1_000_000;

#[test]
fn scenario_pnl_correctness_under_partial_fills_fifo() {
    // GIVEN: $100,000 initial cash
    let mut pf = PortfolioState::new(100_000 * M);

    // Buy 10 @ $100
    apply_entry(&mut pf, LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0)));

    // Buy 10 @ $110
    apply_entry(&mut pf, LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 110 * M, 0)));

    // Sell 5 @ $120 (FIFO sells from first lot at $100)
    apply_entry(&mut pf, LedgerEntry::Fill(Fill::new("AAPL", Side::Sell, 5, 120 * M, 0)));

    // THEN: realized PnL = (120 - 100) * 5 = $100
    assert_eq!(pf.realized_pnl_micros, 100 * M);

    // Remaining position: +15 shares
    let pos = pf.positions.get("AAPL").expect("AAPL position exists");
    assert_eq!(pos.qty_signed(), 15);

    // Marks at $115
    let mk = marks([("AAPL", 115 * M)]);

    // Unrealized:
    // Remaining lots after FIFO sell:
    // - 5 @ 100, 10 @ 110
    // unreal = (115-100)*5 + (115-110)*10 = 75 + 50 = $125
    let unreal = compute_unrealized_pnl_micros(&pf.positions, &mk);
    assert_eq!(unreal, 125 * M);

    // Cash:
    // start 100,000
    // - (10*100) - (10*110) + (5*120) = -1000 -1100 +600 = -1500
    // cash = 98,500
    assert_eq!(pf.cash_micros, 98_500 * M);

    // Equity = cash + qty*mark = 98,500 + 15*115 = 100,225
    let equity = compute_equity_micros(pf.cash_micros, &pf.positions, &mk);
    assert_eq!(equity, 100_225 * M);

    // Exposure:
    // gross = |15|*115 = 1,725
    let exposure = compute_exposure_micros(&pf.positions, &mk);
    assert_eq!(exposure.gross_exposure_micros, 1_725 * M);

    // Determinism invariant: recompute from ledger matches incremental state
    let (cash2, realized2, positions2) = recompute_from_ledger(pf.initial_cash_micros, &pf.ledger);
    assert_eq!(cash2, pf.cash_micros);
    assert_eq!(realized2, pf.realized_pnl_micros);
    assert_eq!(positions2, pf.positions);
}
