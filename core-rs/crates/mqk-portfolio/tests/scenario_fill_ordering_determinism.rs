//! Scenario: Fill application ordering is deterministic — R3-2
//!
//! # Invariants under test
//!
//! 1. `sort_fills_canonical` produces the same sorted order regardless of
//!    the initial arrival order of fills (permutation invariance).
//!
//! 2. Applying fills via `apply_fills_canonical` always produces the same
//!    ledger state — cash, positions, and realized PnL are identical across
//!    all input permutations.
//!
//! 3. Applying fills in a *non-canonical* order to a raw ledger produces a
//!    **different** final state — proving FIFO lot accounting is genuinely
//!    order-sensitive and the canonical path is necessary.
//!
//! 4. The sort key `(seq_no, symbol, side_ord, qty)` places Buy before Sell
//!    when seq_no and symbol are tied.
//!
//! 5. `sort_fills_canonical` is idempotent: sorting a second time does not
//!    change the result.
//!
//! All tests are pure; no IO, no DB, no network.

use mqk_portfolio::{
    apply_fills_canonical, sort_fills_canonical, Fill, Ledger, LedgerSnapshot, Side, TaggedFill,
    MICROS_SCALE,
};

const M: i64 = MICROS_SCALE;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tf(seq_no: u64, symbol: &str, side: Side, qty: i64, price_dollars: i64) -> TaggedFill {
    TaggedFill {
        seq_no,
        fill: Fill::new(symbol, side, qty, price_dollars * M, 0),
    }
}

fn canonical_snapshot(fills: Vec<TaggedFill>) -> LedgerSnapshot {
    let mut ledger = Ledger::new(100_000 * M);
    apply_fills_canonical(&mut ledger, fills).unwrap();
    ledger.snapshot()
}

// ---------------------------------------------------------------------------
// 1 + 2: Permutation invariance
// ---------------------------------------------------------------------------

#[test]
fn canonical_apply_is_permutation_invariant() {
    // Canonical order: seq 1 (buy 10@100), seq 2 (buy 10@110), seq 3 (sell 5@120).
    // FIFO: sell closes 5 shares from the seq-1 lot @ $100.
    // Realized PnL = (120 - 100) * 5 = $100.
    let fill_a = tf(1, "AAPL", Side::Buy, 10, 100);
    let fill_b = tf(2, "AAPL", Side::Buy, 10, 110);
    let fill_c = tf(3, "AAPL", Side::Sell, 5, 120);

    let snap_canonical = canonical_snapshot(vec![
        fill_a.clone(),
        fill_b.clone(),
        fill_c.clone(),
    ]);

    // Reversed arrival order.
    let snap_reversed = canonical_snapshot(vec![
        fill_c.clone(),
        fill_b.clone(),
        fill_a.clone(),
    ]);

    // Another permutation.
    let snap_middle = canonical_snapshot(vec![
        fill_b.clone(),
        fill_c.clone(),
        fill_a.clone(),
    ]);

    assert_eq!(
        snap_canonical.realized_pnl_micros,
        100 * M,
        "canonical order: realized PnL must be $100"
    );
    assert_eq!(
        snap_reversed.realized_pnl_micros,
        snap_canonical.realized_pnl_micros,
        "reversed arrival must match canonical after sort"
    );
    assert_eq!(
        snap_middle.realized_pnl_micros,
        snap_canonical.realized_pnl_micros,
        "middle permutation must match canonical after sort"
    );
    assert_eq!(snap_reversed.cash_micros, snap_canonical.cash_micros);
    assert_eq!(snap_reversed.positions, snap_canonical.positions);
}

// ---------------------------------------------------------------------------
// 3: Non-canonical application produces different state
// ---------------------------------------------------------------------------

#[test]
fn non_canonical_order_produces_different_pnl() {
    // Canonical sequence:
    //   seq 1: Buy  10 @ $100  → lots: [10@100]
    //   seq 2: Sell 10 @ $90   → FIFO closes all 10@100; realized = (90-100)*10 = −$100; flat
    //   seq 3: Buy   5 @ $80   → lots: [5@80]
    // Canonical realized PnL = −$100.
    //
    // Non-canonical (seq-3 applied first):
    //   Buy  5  @ $80  → lots: [5@80]
    //   Buy  10 @ $100 → lots: [5@80, 10@100]
    //   Sell 10 @ $90  → FIFO: close 5@80 (realized = +$50), close 5@100 (realized = −$50)
    //                    → realized = $0; remaining: [5@100]
    // Non-canonical realized PnL = $0.
    //
    // The two paths produce genuinely different PnL (−$100 vs $0),
    // demonstrating that FIFO lot accounting IS order-sensitive.
    let mut ledger_wrong = Ledger::new(100_000 * M);
    ledger_wrong
        .append_fill(Fill::new("AAPL", Side::Buy, 5, 80 * M, 0))
        .unwrap();
    ledger_wrong
        .append_fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0))
        .unwrap();
    ledger_wrong
        .append_fill(Fill::new("AAPL", Side::Sell, 10, 90 * M, 0))
        .unwrap();

    let snap_wrong = ledger_wrong.snapshot();
    let snap_canonical = canonical_snapshot(vec![
        tf(1, "AAPL", Side::Buy, 10, 100),
        tf(2, "AAPL", Side::Sell, 10, 90),
        tf(3, "AAPL", Side::Buy, 5, 80),
    ]);

    assert_eq!(
        snap_canonical.realized_pnl_micros,
        -100 * M,
        "canonical: buy@100 then sell@90 → realized = −$100"
    );
    assert_eq!(
        snap_wrong.realized_pnl_micros,
        0,
        "non-canonical: buy@80 first → sell@90 closes mixed lots → realized = $0"
    );
    assert_ne!(
        snap_wrong.realized_pnl_micros,
        snap_canonical.realized_pnl_micros,
        "non-canonical application must produce different PnL — proves FIFO is order-sensitive"
    );
}

// ---------------------------------------------------------------------------
// 4: Sort key: Buy before Sell when seq_no and symbol are tied
// ---------------------------------------------------------------------------

#[test]
fn sort_key_buy_before_sell_on_tied_seq_no() {
    let mut fills = vec![
        tf(1, "AAPL", Side::Sell, 5, 120),  // should sort to index 1
        tf(1, "AAPL", Side::Buy, 10, 100),  // should sort to index 0
    ];
    sort_fills_canonical(&mut fills);
    assert_eq!(
        fills[0].fill.side,
        Side::Buy,
        "Buy must precede Sell when seq_no and symbol are tied"
    );
    assert_eq!(fills[1].fill.side, Side::Sell);
}

// ---------------------------------------------------------------------------
// 5: sort_fills_canonical is idempotent
// ---------------------------------------------------------------------------

#[test]
fn sort_fills_canonical_is_idempotent() {
    let mut fills = vec![
        tf(3, "AAPL", Side::Sell, 5, 120),
        tf(1, "AAPL", Side::Buy, 10, 100),
        tf(2, "MSFT", Side::Buy, 8, 300),
    ];
    sort_fills_canonical(&mut fills);
    let after_first = fills.clone();
    sort_fills_canonical(&mut fills);
    assert_eq!(
        fills, after_first,
        "sorting twice must produce the same order"
    );
}

// ---------------------------------------------------------------------------
// 6: Multi-symbol canonical ordering
// ---------------------------------------------------------------------------

#[test]
fn multi_symbol_canonical_ordering_is_deterministic() {
    // Two symbols with interleaved seq_nos.
    let fill_spy1 = tf(1, "SPY", Side::Buy, 100, 400);
    let fill_qqq1 = tf(2, "QQQ", Side::Buy, 50, 300);
    let fill_spy2 = tf(3, "SPY", Side::Sell, 50, 410);
    let fill_qqq2 = tf(4, "QQQ", Side::Sell, 25, 310);

    let snap_a = canonical_snapshot(vec![
        fill_spy1.clone(),
        fill_qqq1.clone(),
        fill_spy2.clone(),
        fill_qqq2.clone(),
    ]);
    let snap_b = canonical_snapshot(vec![
        fill_qqq2.clone(),
        fill_spy1.clone(),
        fill_spy2.clone(),
        fill_qqq1.clone(),
    ]);

    assert_eq!(snap_a.realized_pnl_micros, snap_b.realized_pnl_micros);
    assert_eq!(snap_a.cash_micros, snap_b.cash_micros);
    assert_eq!(snap_a.positions, snap_b.positions);
}
