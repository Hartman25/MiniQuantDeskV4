use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side};

const M: i64 = 1_000_000;

#[test]
fn scenario_position_flatten_behavior() {
    let mut pf = PortfolioState::new(10_000 * M);

    // Buy 10 @ 100
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0)),
    );

    // Sell 10 @ 90 (flatten)
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Sell, 10, 90 * M, 0)),
    );

    // Position should be removed (flat)
    assert!(!pf.positions.contains_key("AAPL"));

    // Realized PnL = (90-100)*10 = -100
    assert_eq!(pf.realized_pnl_micros, -100 * M);
}
