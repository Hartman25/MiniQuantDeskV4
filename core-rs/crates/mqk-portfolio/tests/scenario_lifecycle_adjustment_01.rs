//! D2: options-lifecycle economics in the canonical mutable ledger
//! (`LedgerEntry::LifecycleAdjustment`).
//!
//! The four required economic directions, expiration, provider-signed cash,
//! saturation at flat, FIFO interaction with the underlying, and the
//! incremental-vs-recompute determinism invariant. Pure (no DB).
//!
//! Sign convention under test: the option delta is the change to the option
//! position (negative removes long contracts, positive removes short); the
//! underlying delta is the signed share change; the cash delta is the
//! provider's signed `net_amount`.

use mqk_portfolio::{
    apply_entry, recompute_from_ledger, Fill, LedgerEntry, LifecycleAdjustment, PortfolioState,
    QtyMicros, Side, UnderlyingAdjustment,
};

const OPTION: &str = "AAPL230721C00150000";
const PUT: &str = "AAPL230721P00150000";
const START_CASH: i64 = 1_000_000_000_000; // $1,000,000.000000
const STRIKE: i64 = 150_000_000; // $150.000000
const PREMIUM: i64 = 5_000_000; // $5.000000

fn units(n: i64) -> QtyMicros {
    QtyMicros::from_whole_units(n).unwrap()
}

fn adjustment(
    option_symbol: &str,
    option_delta: i64,
    underlying_delta: Option<i64>,
    cash: i64,
) -> LifecycleAdjustment {
    LifecycleAdjustment {
        economic_apply_id: format!("apply-{option_symbol}-{option_delta}"),
        option_symbol: option_symbol.to_string(),
        option_qty_delta: units(option_delta),
        underlying: underlying_delta.map(|d| UnderlyingAdjustment {
            symbol: "AAPL".to_string(),
            qty_delta: units(d),
            basis_price_micros: STRIKE,
        }),
        cash_delta_micros: cash,
    }
}

/// A portfolio holding `contracts` of `symbol` (long if positive, short if
/// negative) opened by an ordinary fill at the premium.
fn holding(symbol: &str, contracts: i64) -> PortfolioState {
    let mut pf = PortfolioState::new(START_CASH);
    let side = if contracts > 0 { Side::Buy } else { Side::Sell };
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(symbol, side, units(contracts.abs()), PREMIUM, 0)),
    );
    pf
}

fn qty(pf: &PortfolioState, symbol: &str) -> QtyMicros {
    pf.positions
        .get(symbol)
        .map(|p| p.qty_signed())
        .unwrap_or(QtyMicros::ZERO)
}

fn apply(pf: &mut PortfolioState, a: LifecycleAdjustment) {
    apply_entry(pf, LedgerEntry::LifecycleAdjustment(a));
}

#[test]
fn long_call_exercise_removes_the_option_receives_shares_pays_cash() {
    let mut pf = holding(OPTION, 2);
    let cash_before = pf.cash_micros;
    apply(&mut pf, adjustment(OPTION, -2, Some(200), -30_000_000_000));
    assert_eq!(qty(&pf, OPTION), QtyMicros::ZERO, "long option removed");
    assert_eq!(qty(&pf, "AAPL"), units(200), "shares received");
    assert_eq!(
        pf.cash_micros,
        cash_before - 30_000_000_000,
        "cash decreases"
    );
}

#[test]
fn long_put_exercise_removes_the_option_delivers_shares_receives_cash() {
    let mut pf = holding(PUT, 2);
    let cash_before = pf.cash_micros;
    apply(&mut pf, adjustment(PUT, -2, Some(-200), 30_000_000_000));
    assert_eq!(qty(&pf, PUT), QtyMicros::ZERO);
    assert_eq!(
        qty(&pf, "AAPL"),
        units(-200),
        "shares delivered (short lot)"
    );
    assert_eq!(
        pf.cash_micros,
        cash_before + 30_000_000_000,
        "cash increases"
    );
}

#[test]
fn short_call_assignment_removes_the_short_delivers_shares_receives_cash() {
    let mut pf = holding(OPTION, -2);
    let cash_before = pf.cash_micros;
    apply(&mut pf, adjustment(OPTION, 2, Some(-200), 30_000_000_000));
    assert_eq!(qty(&pf, OPTION), QtyMicros::ZERO, "short option removed");
    assert_eq!(qty(&pf, "AAPL"), units(-200));
    assert_eq!(pf.cash_micros, cash_before + 30_000_000_000);
}

#[test]
fn short_put_assignment_removes_the_short_receives_shares_pays_cash() {
    let mut pf = holding(PUT, -2);
    let cash_before = pf.cash_micros;
    apply(&mut pf, adjustment(PUT, 2, Some(200), -30_000_000_000));
    assert_eq!(qty(&pf, PUT), QtyMicros::ZERO);
    assert_eq!(qty(&pf, "AAPL"), units(200));
    assert_eq!(pf.cash_micros, cash_before - 30_000_000_000);
}

#[test]
fn expiration_removes_the_option_and_invents_no_delivery_and_no_cash() {
    let mut pf = holding(OPTION, 3);
    let cash_before = pf.cash_micros;
    let realized_before = pf.realized_pnl_micros;
    apply(&mut pf, adjustment(OPTION, -3, None, 0));
    assert_eq!(qty(&pf, OPTION), QtyMicros::ZERO);
    assert!(!pf.positions.contains_key("AAPL"), "no underlying invented");
    assert_eq!(pf.cash_micros, cash_before, "no cash invented");
    assert_eq!(pf.realized_pnl_micros, realized_before);
}

#[test]
fn cash_is_exactly_the_provider_value_never_recomputed_from_qty_and_strike() {
    let mut pf = holding(OPTION, 1);
    let cash_before = pf.cash_micros;
    // qty x strike = 100 x 150 = 15,000.00; the provider reports 15,000.37
    // (e.g. a fee folded into net_amount). The ledger must carry the
    // provider's number, not force consistency.
    apply(&mut pf, adjustment(OPTION, -1, Some(100), -15_000_370_000));
    assert_eq!(pf.cash_micros, cash_before - 15_000_370_000);
}

#[test]
fn option_removal_saturates_at_flat_and_never_crosses_to_the_other_side() {
    let mut pf = holding(OPTION, 1);
    // Evidence removes 5 long contracts; only 1 is held locally.
    apply(&mut pf, adjustment(OPTION, -5, None, 0));
    assert_eq!(qty(&pf, OPTION), QtyMicros::ZERO, "flat, not short");
    // Removing a short side never touches a long holding.
    let mut pf = holding(OPTION, 2);
    apply(&mut pf, adjustment(OPTION, 2, None, 0));
    assert_eq!(qty(&pf, OPTION), units(2), "no short lots to remove");
}

#[test]
fn partial_removal_keeps_the_remainder_fifo() {
    let mut pf = holding(OPTION, 5);
    apply(&mut pf, adjustment(OPTION, -2, None, 0));
    assert_eq!(qty(&pf, OPTION), units(3));
}

#[test]
fn delivered_shares_close_an_existing_long_fifo_with_realized_pnl_at_the_strike() {
    let mut pf = PortfolioState::new(START_CASH);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, units(200), 140_000_000, 0)),
    );
    let realized_before = pf.realized_pnl_micros;
    // Long put exercised: 200 shares delivered at the 150 strike.
    apply(&mut pf, adjustment(PUT, -2, Some(-200), 30_000_000_000));
    assert_eq!(qty(&pf, "AAPL"), QtyMicros::ZERO);
    // (150 - 140) x 200 = $2,000 realized.
    assert_eq!(pf.realized_pnl_micros - realized_before, 2_000_000_000);
}

#[test]
fn incremental_apply_matches_recompute_from_ledger() {
    let mut pf = holding(OPTION, 2);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("MSFT", Side::Buy, units(10), 300_000_000, 1_000)),
    );
    apply(&mut pf, adjustment(OPTION, -2, Some(200), -30_000_000_000));
    apply(&mut pf, adjustment(PUT, -1, None, 0));
    let (cash, realized, positions) = recompute_from_ledger(START_CASH, &pf.ledger);
    assert_eq!(cash, pf.cash_micros);
    assert_eq!(realized, pf.realized_pnl_micros);
    assert_eq!(positions, pf.positions);
}

#[test]
fn the_ledger_records_the_adjustment_as_its_own_entry_never_a_fill() {
    let mut pf = holding(OPTION, 1);
    apply(&mut pf, adjustment(OPTION, -1, Some(100), -15_000_000_000));
    let last = pf.ledger.last().unwrap();
    assert!(matches!(last, LedgerEntry::LifecycleAdjustment(_)));
    assert_eq!(
        pf.ledger
            .iter()
            .filter(|e| matches!(e, LedgerEntry::Fill(_)))
            .count(),
        1,
        "only the original premium fill is a fill"
    );
}
