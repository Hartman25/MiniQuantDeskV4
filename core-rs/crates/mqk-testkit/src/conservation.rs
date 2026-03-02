//! Capital conservation invariant helper — I9-1.
//!
//! Reusable assertion that verifies the conservation identity after every
//! portfolio mutation:
//!
//! ```text
//! equity(marks) == initial_cash − total_fees_paid + realized_pnl + unrealized_pnl(marks)
//! ```
//!
//! Also verifies that the incremental portfolio state (`cash_micros`,
//! `realized_pnl_micros`, `positions`) matches a full replay of the
//! ledger via `recompute_from_ledger` — catching any drift between the
//! two update paths.
//!
//! Zero IO, zero wall-clock time, fully deterministic.

use mqk_portfolio::{
    compute_equity_micros, compute_unrealized_pnl_micros, recompute_from_ledger, LedgerEntry,
    MarkMap, PortfolioState,
};

/// Sum all fill fees recorded in `state.ledger`.
fn total_fees(state: &PortfolioState) -> i64 {
    state
        .ledger
        .iter()
        .filter_map(|e| match e {
            LedgerEntry::Fill(f) => Some(f.fee_micros),
            LedgerEntry::Cash(_) => None,
        })
        .fold(0i64, |acc, fee| acc.saturating_add(fee))
}

/// Assert the capital conservation invariant for `state` marked at `marks`.
///
/// ## Invariant
///
/// ```text
/// equity(marks) == initial_cash − total_fees_paid + realized_pnl + unrealized_pnl(marks)
/// ```
///
/// ## Also checks
///
/// Incremental portfolio state (`state.cash_micros`, `state.realized_pnl_micros`,
/// `state.positions`) must match a full replay of `state.ledger` via
/// `recompute_from_ledger`.  Any discrepancy means the incremental update path
/// has drifted from the authoritative ledger replay path.
///
/// ## Parameters
///
/// - `state` — the `PortfolioState` to inspect (must have been built via
///   `apply_entry` so that `state.ledger` is populated).
/// - `marks` — mark prices used for unrealized-PnL and equity computation.
/// - `label` — description printed in any assertion failure message.
///
/// # Panics
///
/// Panics with a descriptive message if any invariant is violated.
pub fn assert_capital_conservation(state: &PortfolioState, marks: &MarkMap, label: &str) {
    // ── 1. Incremental vs. ledger-recompute consistency ──────────────────────
    let (r_cash, r_realized, r_positions) =
        recompute_from_ledger(state.initial_cash_micros, &state.ledger);

    assert_eq!(
        state.cash_micros, r_cash,
        "{}: cash drift — incremental={} recomputed={}",
        label, state.cash_micros, r_cash
    );
    assert_eq!(
        state.realized_pnl_micros, r_realized,
        "{}: realized_pnl drift — incremental={} recomputed={}",
        label, state.realized_pnl_micros, r_realized
    );
    assert_eq!(
        state.positions, r_positions,
        "{}: position drift — incremental != recomputed",
        label
    );

    // ── 2. Conservation identity ──────────────────────────────────────────────
    let fees = total_fees(state);
    let unrealized = compute_unrealized_pnl_micros(&state.positions, marks);
    let equity = compute_equity_micros(state.cash_micros, &state.positions, marks);

    let expected = state
        .initial_cash_micros
        .saturating_sub(fees)
        .saturating_add(state.realized_pnl_micros)
        .saturating_add(unrealized);

    assert_eq!(
        equity,
        expected,
        "{}: conservation violated — \
         equity={} but initial({})-fees({})+realized({})+unrealized({})={}",
        label,
        equity,
        state.initial_cash_micros,
        fees,
        state.realized_pnl_micros,
        unrealized,
        expected
    );
}
