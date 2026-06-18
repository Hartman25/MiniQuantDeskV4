//! SHORT-SIDE-PAPER-BROKER-POSITION-PROOF-01 — Pure portfolio accounting
//! lifecycle for short positions.
//!
//! Proves the FIFO lot accounting model (via `Ledger`) correctly handles the
//! entire short-position lifecycle without enabling real shorting in production.
//!
//! # Safety invariants
//! - No strategy short signals are enabled.
//! - B5 guard is not modified.
//! - No broker submit calls, no Alpaca routing changes.
//! - No DB, no IO, no network.
//!
//! # What "paper fill proxy" tests mean
//! The fill engine (`DeterministicFillEngine`) produces `BrokerEvent::Fill`
//! with `side` and `delta_qty` fields (always positive).
//! `broker_event_to_portfolio_fill` in `mqk-daemon` maps these to
//! `mqk_portfolio::Fill { side, qty: delta_qty }`.
//! The proxy tests use `Fill` directly with the same `(side, qty)` pair the
//! fill engine would emit, proving accounting correctness without driving the
//! full broker pipeline.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                                     |
//! |------|------------------------------------------------------------------------------|
//! | SP01 | Sell from flat → negative signed position (-100)                             |
//! | SP02 | Sell again while short → larger negative position (-150)                     |
//! | SP03 | Partial cover (buy 30 of 100 short) → still negative (-70)                   |
//! | SP04 | Full cover (buy exact 100) → flat (qty=0)                                    |
//! | SP05 | Buy beyond cover (buy 150 vs -100 short) → long (+50): documents crossover   |
//! | SP06 | Realized PnL: sell high $100, cover low $80 → positive (+$2000)              |
//! | SP07 | Realized PnL: sell low $80, cover high $100 → negative (-$2000)              |
//! | SP08 | qty_signed() returns negative for an open short position                     |
//! | SP09 | Short position is not absent: symbol exists in positions map with qty < 0    |
//! | SP10 | verify_integrity() passes through full short lifecycle (incremental == replay) |
//! | SP11 | Paper fill proxy: Fill{side=Sell, qty>0} → negative position                 |
//! | SP12 | Paper cover proxy: Fill{side=Buy, qty>0} → covers short back to flat         |
//! | SP13 | Paper fill qty is absolute: qty > 0 for both sell and buy-to-cover fills     |
//! | SP14 | Two-step cover lifecycle: integrity check at every stage                     |
//! | SP15 | Negative position is not confused with zero/missing                          |

use mqk_portfolio::{Fill, Ledger, Side, MICROS_SCALE};

const M: i64 = MICROS_SCALE; // 1_000_000 micros per dollar
const INITIAL_CASH: i64 = 100_000 * M; // $100,000 starting cash

fn make_ledger() -> Ledger {
    Ledger::new(INITIAL_CASH)
}

fn sell_fill(symbol: &str, qty: i64, price_dollars: i64) -> Fill {
    Fill::new(symbol, Side::Sell, qty, price_dollars * M, 0)
}

fn buy_fill(symbol: &str, qty: i64, price_dollars: i64) -> Fill {
    Fill::new(symbol, Side::Buy, qty, price_dollars * M, 0)
}

// ---------------------------------------------------------------------------
// SP01 — Sell from flat creates negative signed position
// ---------------------------------------------------------------------------

#[test]
fn sp01_sell_from_flat_creates_negative_position() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 50)).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, -100,
        "SP01: sell 100 from flat must create qty_signed=-100; got {qty}"
    );
    assert!(
        !ledger.is_flat(),
        "SP01: after short sell, ledger must not be flat"
    );
    assert!(
        ledger.verify_integrity(),
        "SP01: integrity must hold after opening short"
    );
}

// ---------------------------------------------------------------------------
// SP02 — Sell again while short increases negative signed position
// ---------------------------------------------------------------------------

#[test]
fn sp02_sell_again_while_short_increases_negative_position() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 50)).unwrap();
    ledger.append_fill(sell_fill("GME", 50, 48)).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, -150,
        "SP02: selling 50 more while at -100 must yield qty_signed=-150; got {qty}"
    );
    assert!(
        ledger.verify_integrity(),
        "SP02: integrity must hold after adding to short"
    );
}

// ---------------------------------------------------------------------------
// SP03 — Partial cover leaves negative signed position
// ---------------------------------------------------------------------------

#[test]
fn sp03_partial_cover_leaves_negative_position() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 50)).unwrap();
    ledger.append_fill(buy_fill("GME", 30, 45)).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, -70,
        "SP03: covering 30 of -100 short must leave qty_signed=-70; got {qty}"
    );
    assert!(!ledger.is_flat(), "SP03: partial cover must not reach flat");
    assert!(
        ledger.verify_integrity(),
        "SP03: integrity must hold after partial cover"
    );
}

// ---------------------------------------------------------------------------
// SP04 — Full cover (buy exact qty) returns to flat
// ---------------------------------------------------------------------------

#[test]
fn sp04_full_cover_returns_to_flat() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 50)).unwrap();
    ledger.append_fill(buy_fill("GME", 100, 45)).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, 0,
        "SP04: covering exact 100 of a -100 short must return to flat; got {qty}"
    );
    assert!(
        ledger.is_flat(),
        "SP04: after exact full cover, ledger must report flat"
    );
    assert!(
        ledger.verify_integrity(),
        "SP04: integrity must hold after full cover"
    );
}

// ---------------------------------------------------------------------------
// SP05 — Buy beyond cover crosses from short to long
//
// Documents the current accounting behavior: buying more than the short qty
// covers the short FIFO and opens a residual long lot with the excess quantity.
// This tests the existing FIFO semantics; it does NOT enable strategy shorting.
// ---------------------------------------------------------------------------

#[test]
fn sp05_buy_beyond_cover_crosses_to_long() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 50)).unwrap();
    ledger.append_fill(buy_fill("GME", 150, 45)).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, 50,
        "SP05: buying 150 to cover -100 short must produce residual long qty_signed=+50; got {qty}"
    );
    assert!(
        !ledger.is_flat(),
        "SP05: crossed-to-long position must not be flat"
    );
    assert!(
        ledger.verify_integrity(),
        "SP05: integrity must hold after short-to-long crossover"
    );
}

// ---------------------------------------------------------------------------
// SP06 — Realized PnL: sell high, cover low → positive
// ---------------------------------------------------------------------------

#[test]
fn sp06_realized_pnl_short_sell_high_cover_low_is_positive() {
    // Realized PnL formula: (entry_px - cover_px) * qty = (100 - 80) * 100 = +$2000.
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 100)).unwrap();
    ledger.append_fill(buy_fill("GME", 100, 80)).unwrap();

    let realized = ledger.realized_pnl_micros();
    assert_eq!(
        realized,
        2_000 * M,
        "SP06: short sell $100 cover $80 qty=100 must yield realized PnL=+$2000; got ${}",
        realized / M
    );
    assert!(
        realized > 0,
        "SP06: profitable short cover must have positive realized PnL"
    );
    assert!(
        ledger.is_flat(),
        "SP06: fully covered position must be flat"
    );
    // cash = initial + sell_proceeds - cover_cost = $100k + $10k - $8k = $102k
    assert_eq!(
        ledger.cash_micros(),
        INITIAL_CASH + 2_000 * M,
        "SP06: cash must equal initial + realized PnL after round-trip short"
    );
}

// ---------------------------------------------------------------------------
// SP07 — Realized PnL: sell low, cover high → negative
// ---------------------------------------------------------------------------

#[test]
fn sp07_realized_pnl_short_sell_low_cover_high_is_negative() {
    // Realized PnL formula: (entry_px - cover_px) * qty = (80 - 100) * 100 = -$2000.
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("GME", 100, 80)).unwrap();
    ledger.append_fill(buy_fill("GME", 100, 100)).unwrap();

    let realized = ledger.realized_pnl_micros();
    assert_eq!(
        realized,
        -2_000 * M,
        "SP07: short sell $80 cover $100 qty=100 must yield realized PnL=-$2000; got ${}",
        realized / M
    );
    assert!(
        realized < 0,
        "SP07: losing short cover must have negative realized PnL"
    );
    assert!(
        ledger.is_flat(),
        "SP07: fully covered position must be flat"
    );
    // cash = initial + sell_proceeds - cover_cost = $100k + $8k - $10k = $98k
    assert_eq!(
        ledger.cash_micros(),
        INITIAL_CASH - 2_000 * M,
        "SP07: cash must equal initial + realized PnL (negative) after losing short"
    );
}

// ---------------------------------------------------------------------------
// SP08 — qty_signed() returns negative for open short position
// ---------------------------------------------------------------------------

#[test]
fn sp08_qty_signed_is_negative_for_short() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("TSLA", 75, 200)).unwrap();

    let qty = ledger.qty_signed("TSLA");
    assert!(
        qty < 0,
        "SP08: qty_signed for a short position must be negative; got {qty}"
    );
    assert_eq!(
        qty, -75,
        "SP08: qty_signed must equal -(sell qty) = -75; got {qty}"
    );
}

// ---------------------------------------------------------------------------
// SP09 — Short position is not absent: symbol exists with qty < 0
// ---------------------------------------------------------------------------

#[test]
fn sp09_short_position_is_not_absent() {
    let mut ledger = make_ledger();
    ledger.append_fill(sell_fill("NVDA", 20, 400)).unwrap();

    let snap = ledger.snapshot();
    assert!(
        snap.positions.contains_key("NVDA"),
        "SP09: short position must appear in the positions map (not absent)"
    );
    let qty = snap.qty_signed("NVDA");
    assert!(
        qty < 0,
        "SP09: positions map must store negative qty for short; got {qty}"
    );
    assert_ne!(
        qty, 0,
        "SP09: short position is not flat/absent (qty must not be 0)"
    );
}

// ---------------------------------------------------------------------------
// SP10 — verify_integrity() passes through full short lifecycle
//
// verify_integrity() replays the ledger and compares against running state.
// This is the same invariant that check_capital_invariants in Phase 3b enforces
// after every inbox-apply pass (via recompute_from_ledger).
// ---------------------------------------------------------------------------

#[test]
fn sp10_integrity_holds_through_full_short_lifecycle() {
    let mut ledger = make_ledger();

    // Stage 1: open short
    ledger.append_fill(sell_fill("SPY", 200, 500)).unwrap();
    assert!(
        ledger.verify_integrity(),
        "SP10: integrity after opening short"
    );
    assert_eq!(ledger.qty_signed("SPY"), -200);

    // Stage 2: add to short
    ledger.append_fill(sell_fill("SPY", 100, 495)).unwrap();
    assert!(
        ledger.verify_integrity(),
        "SP10: integrity after adding to short"
    );
    assert_eq!(ledger.qty_signed("SPY"), -300);

    // Stage 3: partial cover (buy 100 from -300)
    // FIFO covers from the first lot (200 @ $500): covers 100 → lot becomes -100 @ $500
    // Realized PnL = (500 - 490) * 100 = $1000
    ledger.append_fill(buy_fill("SPY", 100, 490)).unwrap();
    assert!(
        ledger.verify_integrity(),
        "SP10: integrity after partial cover"
    );
    assert_eq!(ledger.qty_signed("SPY"), -200);

    // Stage 4: full cover (buy remaining 200)
    // FIFO: first covers remaining -100 @ $500: pnl = (500-485)*100 = $1500
    //       then covers -100 @ $495: pnl = (495-485)*100 = $1000
    ledger.append_fill(buy_fill("SPY", 200, 485)).unwrap();
    assert!(
        ledger.verify_integrity(),
        "SP10: integrity after full cover"
    );
    assert!(
        ledger.is_flat(),
        "SP10: fully covered position must be flat"
    );

    // Total realized PnL: $1000 + $1500 + $1000 = $3500
    let realized = ledger.realized_pnl_micros();
    assert_eq!(
        realized,
        3_500 * M,
        "SP10: total realized PnL for the lifecycle must be $3500; got ${}",
        realized / M
    );
}

// ---------------------------------------------------------------------------
// SP11 — Paper fill proxy: Fill{side=Sell, qty>0} → negative position
//
// The fill engine produces BrokerEvent::Fill{side=Sell, delta_qty=abs_qty>0}.
// broker_event_to_portfolio_fill maps this to Fill{side=Sell, qty=delta_qty}.
// This proves that such a fill (positive qty, Sell side) opens a short lot.
// ---------------------------------------------------------------------------

#[test]
fn sp11_paper_fill_proxy_sell_side_opens_short() {
    let mut ledger = make_ledger();

    // Mirrors what the fill engine produces: side=Sell, qty=abs_qty (positive)
    let paper_fill = Fill::new("GME", Side::Sell, 200, 50 * M, 0);
    assert!(
        paper_fill.qty > 0,
        "SP11: paper fill qty must be positive (fill engine P1-01 invariant)"
    );

    ledger.append_fill(paper_fill).unwrap();

    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, -200,
        "SP11: Side::Sell fill qty=200 must open short lot qty_signed=-200; got {qty}"
    );
    assert!(
        qty < 0,
        "SP11: accounting result of Side::Sell on flat position must be negative"
    );
    assert!(
        ledger.verify_integrity(),
        "SP11: integrity must hold after paper short fill"
    );
}

// ---------------------------------------------------------------------------
// SP12 — Paper cover proxy: Fill{side=Buy, qty>0} → covers short to flat
//
// The fill engine produces BrokerEvent::Fill{side=Buy, delta_qty>0} for
// buy-to-cover orders. broker_event_to_portfolio_fill maps this to
// Fill{side=Buy, qty=delta_qty}. This proves the cover closes the short lot.
// ---------------------------------------------------------------------------

#[test]
fn sp12_paper_cover_proxy_buy_side_covers_short() {
    let mut ledger = make_ledger();

    // Open the short (simulating fill engine sell output)
    ledger
        .append_fill(Fill::new("GME", Side::Sell, 200, 50 * M, 0))
        .unwrap();
    assert_eq!(ledger.qty_signed("GME"), -200);

    // Mirrors what the fill engine produces for buy-to-cover: side=Buy, qty > 0
    let cover_fill = Fill::new("GME", Side::Buy, 200, 45 * M, 0);
    assert!(
        cover_fill.qty > 0,
        "SP12: cover fill qty must be positive (fill engine P1-01 invariant)"
    );

    ledger.append_fill(cover_fill).unwrap();

    assert!(
        ledger.is_flat(),
        "SP12: Side::Buy fill matching short qty must return position to flat"
    );
    let qty = ledger.qty_signed("GME");
    assert_eq!(
        qty, 0,
        "SP12: qty_signed after full cover must be 0; got {qty}"
    );
    assert!(
        ledger.verify_integrity(),
        "SP12: integrity must hold after paper cover fill"
    );
}

// ---------------------------------------------------------------------------
// SP13 — Paper fill qty is always absolute positive
//
// Fill.qty is ALWAYS positive regardless of trade direction (Sell or Buy).
// The fill engine invariant (P1-01): delta_qty > 0 for any side.
// Direction is conveyed exclusively by the side field.
// ---------------------------------------------------------------------------

#[test]
fn sp13_paper_fill_qty_is_always_absolute_positive() {
    let sell = Fill::new("GME", Side::Sell, 100, 50 * M, 0);
    let buy = Fill::new("GME", Side::Buy, 100, 45 * M, 0);

    assert!(
        sell.qty > 0,
        "SP13: sell fill qty must be positive (absolute); got {}",
        sell.qty
    );
    assert!(
        buy.qty > 0,
        "SP13: buy fill qty must be positive (absolute); got {}",
        buy.qty
    );
    // Direction is in the side field, not in the sign of qty
    assert_ne!(
        sell.side, buy.side,
        "SP13: direction must differ via side field, not via qty sign"
    );
    assert_eq!(
        sell.qty, buy.qty,
        "SP13: both fills have equal absolute qty; direction is exclusively in side field"
    );
}

// ---------------------------------------------------------------------------
// SP14 — Two-step cover lifecycle: integrity at every stage
// ---------------------------------------------------------------------------

#[test]
fn sp14_two_step_cover_lifecycle_integrity_at_every_stage() {
    let mut ledger = make_ledger();

    // Stage 1: open short 100 @ $180
    ledger.append_fill(sell_fill("AAPL", 100, 180)).unwrap();
    assert_eq!(
        ledger.qty_signed("AAPL"),
        -100,
        "SP14 stage 1: qty must be -100"
    );
    assert!(ledger.verify_integrity(), "SP14: integrity at stage 1");

    // Stage 2: partial cover (buy 40 @ $170)
    // FIFO: covers 40 from -100 @ $180: pnl = (180-170)*40 = $400
    ledger.append_fill(buy_fill("AAPL", 40, 170)).unwrap();
    assert_eq!(
        ledger.qty_signed("AAPL"),
        -60,
        "SP14 stage 2: qty must be -60 after covering 40"
    );
    assert!(!ledger.is_flat(), "SP14: partial cover must not be flat");
    assert!(ledger.verify_integrity(), "SP14: integrity at stage 2");

    // Stage 3: remaining cover (buy 60 @ $175)
    // FIFO: covers 60 from -60 @ $180: pnl = (180-175)*60 = $300
    ledger.append_fill(buy_fill("AAPL", 60, 175)).unwrap();
    assert_eq!(
        ledger.qty_signed("AAPL"),
        0,
        "SP14 stage 3: qty must be 0 after full cover"
    );
    assert!(ledger.is_flat(), "SP14: full cover must be flat");
    assert!(ledger.verify_integrity(), "SP14: integrity at stage 3");

    // Total realized PnL: $400 + $300 = $700
    let realized = ledger.realized_pnl_micros();
    assert_eq!(
        realized,
        700 * M,
        "SP14: total realized PnL must be $700 (both covers from $180 entry); got ${}",
        realized / M
    );
}

// ---------------------------------------------------------------------------
// SP15 — Negative position is not confused with zero/missing
// ---------------------------------------------------------------------------

#[test]
fn sp15_negative_position_not_confused_with_zero_or_missing() {
    let mut ledger = make_ledger();

    // Unknown symbol: qty_signed returns 0 (missing, not short)
    let missing_qty = ledger.qty_signed("UNKNOWN");
    assert_eq!(
        missing_qty, 0,
        "SP15: missing symbol must return qty_signed=0"
    );

    // Known symbol with short position: qty_signed returns negative
    ledger.append_fill(sell_fill("GME", 50, 100)).unwrap();
    let short_qty = ledger.qty_signed("GME");
    assert_eq!(
        short_qty, -50,
        "SP15: short position must return qty_signed=-50"
    );

    // Short and missing must not be confused
    assert_ne!(
        missing_qty, short_qty,
        "SP15: missing (0) must not equal short (-50)"
    );
    assert!(
        short_qty < missing_qty,
        "SP15: short qty (-50) must be < missing qty (0)"
    );

    // After full cover: position is removed from the positions map;
    // qty_signed returns 0 (flat), same as "missing" — but both are 0, not a mismatch.
    ledger.append_fill(buy_fill("GME", 50, 95)).unwrap();
    let flat_qty = ledger.qty_signed("GME");
    assert_eq!(
        flat_qty, 0,
        "SP15: after full cover, qty_signed must return 0 (flat)"
    );
    assert!(
        ledger.is_flat(),
        "SP15: ledger must report flat after full cover"
    );
    assert!(
        ledger.verify_integrity(),
        "SP15: integrity holds throughout all stages"
    );
}
