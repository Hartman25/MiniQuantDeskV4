use mqk_portfolio::{
    apply_entry, enforce_max_gross_exposure, Fill, LedgerEntry, PortfolioState, Side, marks,
};

const M: i64 = 1_000_000;

#[test]
fn scenario_multi_symbol_exposure_enforcement() {
    let mut pf = PortfolioState::new(100_000 * M);

    // Build positions via fills (ledger truth)
    apply_entry(&mut pf, LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 200 * M, 0)));
    apply_entry(&mut pf, LedgerEntry::Fill(Fill::new("MSFT", Side::Buy, 10, 300 * M, 0)));

    // Marks: $200 and $300
    let marks = marks([("AAPL", 200 * M), ("MSFT", 300 * M)]);

    // Gross exposure = 10*200 + 10*300 = 5000
    let max_ok = 6_000 * M;
    let max_bad = 4_000 * M;

    assert!(enforce_max_gross_exposure(&pf.positions, &marks, max_ok).is_ok());

    let err = enforce_max_gross_exposure(&pf.positions, &marks, max_bad).unwrap_err();
    assert_eq!(err.gross_exposure_micros, 5_000 * M);
    assert_eq!(err.max_gross_exposure_micros, 4_000 * M);
}
