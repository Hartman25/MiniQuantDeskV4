//! CORP-ACT-01: Corporate-action economic consistency and fail-closed proof.
//!
//! ## What this closes
//!
//! The existing B4 (scenario_corporate_action_policy.rs) proves policy enforcement:
//! ForbidPeriods halts on forbidden bars; Allow never halts.  The existing B7
//! (scenario_corp_actions_b7.rs) proves operator-surface honesty:
//! `corp_actions_screening = "not_wired"` on both daemon surfaces.
//!
//! CORP-ACT-01 fills four remaining gaps:
//!
//! - **CA-01** (`split_adjustment_does_not_create_economic_drift`):
//!   With split-adjusted data the portfolio accounting model is economically
//!   neutral.  Two representations of the same economic exposure —
//!   100 shares × $100 and 200 shares × $50 — produce identical equity.
//!   Drift comes only from unadjusted marks, not from the accounting logic.
//!
//! - **CA-02** (`unsupported_corp_action_not_silently_accepted`):
//!   `Allow` policy with unadjusted post-split data creates a MEASURABLE and
//!   EXPLICIT incorrect portfolio state.  The error is exactly quantifiable;
//!   the system does not normalize or hide it.
//!
//! - **CA-03** (`portfolio_and_market_data_corp_action_truth_remain_aligned`):
//!   With correctly adjusted marks, portfolio equity equals initial cash for
//!   both split representations (marks match cost basis → zero drift).  The
//!   same portfolio marked at an unadjusted post-split price explicitly drifts,
//!   proving the alignment invariant holds if and only if data is adjusted.
//!
//! - **CA-04** (`execution_fill_semantics_do_not_backdoor_corp_action_error`):
//!   `recompute_from_ledger` and the incremental `apply_entry` path are
//!   internally consistent.  When a fill at a pre-split price is later marked
//!   at an unadjusted post-split price the mark-to-market discrepancy is
//!   transparent and exact (no silent normalization).  Additionally, a
//!   `ForbidPeriods` engine halts before the split-boundary bar is processed,
//!   keeping the equity curve and fill ledger free of contaminated entries.
//!
//! All tests are pure in-process — no DB, no IO, no `MQK_DATABASE_URL` required.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, CorporateActionPolicy, ForbidEntry,
};
use mqk_execution::StrategyOutput;
use mqk_portfolio::{
    apply_entry, compute_equity_micros, compute_unrealized_pnl_micros, marks,
    recompute_from_ledger, Fill, LedgerEntry, PortfolioState, Side,
};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/// $1,000,000 initial cash in micros.
const INITIAL_CASH: i64 = 1_000_000_000_000;

/// $100 per share in micros.
const PRICE_100: i64 = 100_000_000;

/// $50 per share in micros — unadjusted post-split price for a 2:1 split.
const PRICE_50: i64 = 50_000_000;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn buy_fill(symbol: &str, qty: i64, price_micros: i64) -> LedgerEntry {
    LedgerEntry::Fill(Fill::new(symbol, Side::Buy, qty, price_micros, 0))
}

fn bar(symbol: &str, end_ts: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        PRICE_100,
        PRICE_100 + 10_000_000,
        PRICE_100 - 10_000_000,
        PRICE_100,
        1_000,
    )
}

// ---------------------------------------------------------------------------
// Noop strategy — used by CA-04 Part C.
// ---------------------------------------------------------------------------

struct Noop;

impl Strategy for Noop {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Noop", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

// ---------------------------------------------------------------------------
// CA-01: split_adjustment_does_not_create_economic_drift
//
// Proves: the portfolio accounting model is economically neutral with respect
// to the split ratio.
//
// Economic identity: qty × price = constant across representations.
//   100 × $100 == 200 × $50 == $10,000 notional
//
// With adjusted marks (mark == cost basis) both representations yield:
//   equity  == initial_cash   (zero net change in portfolio value)
//   upnl    == 0              (mark at cost — no paper gain or loss)
//
// This proves that economic drift across a split event arises only from
// unadjusted marks, NOT from a flaw in the accounting model itself.
// ---------------------------------------------------------------------------

#[test]
fn split_adjustment_does_not_create_economic_drift() {
    // Verify the economic identity in micros.
    let cost_a = 100i64 * PRICE_100; // 100 × $100 = $10,000
    let cost_b = 200i64 * PRICE_50; //  200 × $50  = $10,000
    assert_eq!(
        cost_a, cost_b,
        "CA-01: economic identity — 100×$100 must equal 200×$50 notional; got {cost_a} vs {cost_b}"
    );

    // Scenario A: 100 shares bought at $100, marked at $100 (adjusted, no drift).
    let mut pf_a = PortfolioState::new(INITIAL_CASH);
    apply_entry(&mut pf_a, buy_fill("SPY", 100, PRICE_100));
    let m_a = marks([("SPY", PRICE_100)]);
    let equity_a = compute_equity_micros(pf_a.cash_micros, &pf_a.positions, &m_a);
    let upnl_a = compute_unrealized_pnl_micros(&pf_a.positions, &m_a);

    // Scenario B: 200 shares bought at $50 (split-adjusted), marked at $50.
    let mut pf_b = PortfolioState::new(INITIAL_CASH);
    apply_entry(&mut pf_b, buy_fill("SPY", 200, PRICE_50));
    let m_b = marks([("SPY", PRICE_50)]);
    let equity_b = compute_equity_micros(pf_b.cash_micros, &pf_b.positions, &m_b);
    let upnl_b = compute_unrealized_pnl_micros(&pf_b.positions, &m_b);

    // Both adjusted representations give equity == initial_cash.
    assert_eq!(
        equity_a, INITIAL_CASH,
        "CA-01: 100@$100 marked@$100 → equity must equal initial_cash; got {equity_a}"
    );
    assert_eq!(
        equity_b, INITIAL_CASH,
        "CA-01: 200@$50 marked@$50 → equity must equal initial_cash; got {equity_b}"
    );
    assert_eq!(
        equity_a, equity_b,
        "CA-01: both split representations must yield identical equity; A={equity_a} B={equity_b}"
    );

    // Zero unrealized PnL when mark == cost basis (no drift).
    assert_eq!(
        upnl_a, 0,
        "CA-01: 100@$100 marked@$100 → zero unrealized PnL"
    );
    assert_eq!(upnl_b, 0, "CA-01: 200@$50 marked@$50 → zero unrealized PnL");
}

// ---------------------------------------------------------------------------
// CA-02: unsupported_corp_action_not_silently_accepted
//
// Proves: the `Allow` policy with unadjusted post-split data creates an
// EXPLICIT and QUANTIFIABLE incorrect portfolio state.
//
// Scenario: 100 SPY bought pre-split at $100.  After a 2:1 split the
// unadjusted market data shows $50.  The portfolio lot basis remains $100
// (no automatic adjustment exists).  The mark-to-market therefore reports:
//
//   unrealized_pnl = ($50 − $100) × 100 = −$5,000   ← false loss
//   equity         = initial_cash − $5,000            ← incorrect
//
// The system does NOT normalize this error to zero.  The magnitude is
// exact and observable — proving unsupported corp-action usage (Allow
// without pre-adjusted data) is fail-open with a visible, measurable signal,
// not a silent false positive.
// ---------------------------------------------------------------------------

#[test]
fn unsupported_corp_action_not_silently_accepted() {
    let mut pf = PortfolioState::new(INITIAL_CASH);
    // Buy 100 SPY at $100 (pre-split, unadjusted basis).
    apply_entry(&mut pf, buy_fill("SPY", 100, PRICE_100));

    // Post-split unadjusted mark: $50.  No adjustment mechanism exists.
    let unadjusted_marks = marks([("SPY", PRICE_50)]);

    let upnl = compute_unrealized_pnl_micros(&pf.positions, &unadjusted_marks);
    let equity = compute_equity_micros(pf.cash_micros, &pf.positions, &unadjusted_marks);

    // Expected false loss: ($50 − $100) × 100 = −$5,000 = −5_000_000_000 micros.
    let expected_false_loss: i64 = -5_000_000_000;

    assert_eq!(
        upnl, expected_false_loss,
        "CA-02: unadjusted post-split mark must produce exact false unrealized loss of $5,000; \
         got {upnl}"
    );
    assert_eq!(
        equity,
        INITIAL_CASH + expected_false_loss,
        "CA-02: equity with unadjusted mark must be initial_cash − $5,000; got {equity}"
    );

    // The error is NOT zero — the system does not silently normalize it.
    assert_ne!(upnl, 0, "CA-02: false loss must be visible (non-zero)");
    assert_ne!(
        equity, INITIAL_CASH,
        "CA-02: equity must differ from initial_cash — drift is explicit and observable"
    );
}

// ---------------------------------------------------------------------------
// CA-03: portfolio_and_market_data_corp_action_truth_remain_aligned
//
// Proves: with correctly adjusted marks (mark == cost basis), portfolio equity
// is exactly initial_cash for both pre- and post-split-adjusted representations.
// The two adjusted views agree — no truth gap between portfolio state and
// market-data truth.
//
// Also proves the negative: marking the pre-split-adjusted position at the
// unadjusted post-split price produces equity < initial_cash, confirming that
// misaligned marks create visible, measurable drift.  Alignment holds if and
// only if the mark reflects the same price basis used at fill time.
// ---------------------------------------------------------------------------

#[test]
fn portfolio_and_market_data_corp_action_truth_remain_aligned() {
    // Pre-split-adjusted view: buy at adjusted $100, mark at adjusted $100.
    let mut pf_pre = PortfolioState::new(INITIAL_CASH);
    apply_entry(&mut pf_pre, buy_fill("SPY", 100, PRICE_100));
    let m_pre = marks([("SPY", PRICE_100)]);
    let eq_pre = compute_equity_micros(pf_pre.cash_micros, &pf_pre.positions, &m_pre);

    // Post-split-adjusted view: same economic exposure as 200 shares at $50,
    // marked at $50 (current adjusted price).
    let mut pf_post = PortfolioState::new(INITIAL_CASH);
    apply_entry(&mut pf_post, buy_fill("SPY", 200, PRICE_50));
    let m_post = marks([("SPY", PRICE_50)]);
    let eq_post = compute_equity_micros(pf_post.cash_micros, &pf_post.positions, &m_post);

    // Both adjusted views give equity == initial_cash (aligned with economic reality).
    assert_eq!(
        eq_pre, INITIAL_CASH,
        "CA-03: pre-split-adjusted view equity must equal initial_cash; got {eq_pre}"
    );
    assert_eq!(
        eq_post, INITIAL_CASH,
        "CA-03: post-split-adjusted view equity must equal initial_cash; got {eq_post}"
    );
    assert_eq!(
        eq_pre, eq_post,
        "CA-03: both adjusted representations must agree on equity; pre={eq_pre} post={eq_post}"
    );

    // Misaligned mark: pre-split-adjusted position ($100 basis) marked at unadjusted
    // post-split price ($50).  Portfolio truth must diverge from market-data truth.
    let eq_misaligned = compute_equity_micros(pf_pre.cash_micros, &pf_pre.positions, &m_post);
    assert_ne!(
        eq_misaligned, INITIAL_CASH,
        "CA-03: mismatched mark must not produce correct equity — alignment requires adjusted data"
    );
    assert!(
        eq_misaligned < INITIAL_CASH,
        "CA-03: mismatched mark creates apparent loss — portfolio truth drifts from market-data truth"
    );
}

// ---------------------------------------------------------------------------
// CA-04: execution_fill_semantics_do_not_backdoor_corp_action_error
//
// Three sub-proofs:
//
// Part A — Ledger consistency:
//   `recompute_from_ledger` produces the same cash, realized PnL, and
//   positions as the incremental `apply_entry` path.  The fill ledger is
//   internally consistent.
//
// Part B — Cross-split discrepancy is transparent:
//   Marking a pre-split fill at an unadjusted post-split price produces an
//   exact, quantifiable equity discrepancy of −$5,000.  No silent normalization
//   occurs — the error surface (compute_equity_micros) exposes it directly.
//
// Part C — ForbidPeriods boundary proof:
//   The engine halts before the split-boundary bar is processed.  The equity
//   curve contains only the pre-split bar entry; the split-boundary bar and
//   any subsequent bars are excluded.
// ---------------------------------------------------------------------------

#[test]
fn execution_fill_semantics_do_not_backdoor_corp_action_error() {
    // ---- Part A: ledger consistency ----------------------------------------

    let mut pf = PortfolioState::new(INITIAL_CASH);
    apply_entry(&mut pf, buy_fill("SPY", 100, PRICE_100));

    let (cash_r, realized_r, positions_r) = recompute_from_ledger(INITIAL_CASH, &pf.ledger);

    assert_eq!(
        pf.cash_micros, cash_r,
        "CA-04-A: incremental cash must match recompute_from_ledger"
    );
    assert_eq!(
        pf.realized_pnl_micros, realized_r,
        "CA-04-A: incremental realized_pnl must match recompute_from_ledger"
    );
    assert_eq!(
        pf.positions, positions_r,
        "CA-04-A: incremental positions must match recompute_from_ledger"
    );

    // ---- Part B: cross-split discrepancy is transparent --------------------

    let split_marks = marks([("SPY", PRICE_50)]);
    let equity_at_split_mark = compute_equity_micros(pf.cash_micros, &pf.positions, &split_marks);

    // Discrepancy: 100 × ($50 − $100) = −$5,000 = −5_000_000_000 micros.
    let expected_discrepancy: i64 = -5_000_000_000;
    assert_eq!(
        equity_at_split_mark - INITIAL_CASH,
        expected_discrepancy,
        "CA-04-B: cross-split mark-to-market discrepancy must be exactly −$5,000; \
         got {}",
        equity_at_split_mark - INITIAL_CASH
    );

    // No realized PnL — the error is entirely in the mark, not in the ledger.
    assert_eq!(
        pf.realized_pnl_micros, 0,
        "CA-04-B: realized PnL must be zero — no sell has occurred"
    );

    // ---- Part C: ForbidPeriods halts at the split boundary -----------------

    let split_ts = 200i64;
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "SPY",
            split_ts,
            split_ts + 3_600,
        )]),
        ..BacktestConfig::test_defaults()
    };

    let bars = vec![
        bar("SPY", 100),      // pre-split — must be processed
        bar("SPY", split_ts), // at split boundary — must trigger halt
        bar("SPY", 300),      // post-split — must NOT be processed
    ];

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Noop)).unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(
        report.halted,
        "CA-04-C: engine must halt on split-boundary bar"
    );
    assert_eq!(
        report.equity_curve.len(),
        1,
        "CA-04-C: only the pre-split bar must appear in equity curve; got {}",
        report.equity_curve.len()
    );

    let halt_reason = report
        .halt_reason
        .expect("CA-04-C: halt_reason must be set on corp-action halt");
    assert!(
        halt_reason.contains("SPY"),
        "CA-04-C: halt reason must name the affected symbol; got: {halt_reason}"
    );
}
