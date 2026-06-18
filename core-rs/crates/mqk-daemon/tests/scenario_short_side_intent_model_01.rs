//! SHORT-SIDE-INTENT-MODEL-01 — Order intent classification and short-entry policy wiring.
//!
//! Proves that:
//! - `classify_order_intent` correctly classifies all intent variants.
//! - Short-open and sell-beyond-long require short-entry policy authorization.
//! - Sell-to-close and sell-to-flat are not subject to the short-entry gate.
//! - Buy-to-cover, buy-to-flat, buy-beyond-short-to-long are not short entries.
//! - Default policy blocks short-open with clear reason codes.
//! - Live mode blocks unconditionally regardless of config.
//! - Unknown shortable status blocks when require_shortable_check=true.
//! - bar_result_to_decisions continues to block short-open fail-closed (B5 backstop).
//! - Long-only strategy output is classified as LongOpen and produces buy decisions.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                                  |
//! |------|---------------------------------------------------------------------------|
//! | I01  | Sell from flat → ShortOpen                                                |
//! | I02  | Sell beyond long holdings → SellBeyondLongToShort                         |
//! | I03  | Sell less than long holdings → SellToClose; policy gate not applicable    |
//! | I04  | Sell exactly long holdings → SellToFlat; policy gate not applicable       |
//! | I05  | Buy from flat → LongOpen; not a short entry                               |
//! | I06  | Buy while short (partial cover) → BuyToCover; not a short entry           |
//! | I07  | Buy beyond short into long → BuyBeyondShortToLong; not a short entry     |
//! | I08  | Default policy blocks ShortOpen with reason "short_entries_disabled"      |
//! | I09  | Default policy blocks SellBeyondLongToShort with reason code              |
//! | I10  | Live mode blocks short-open even if all config flags are true             |
//! | I11  | Unknown shortable status blocks when require_shortable_check=true         |
//! | I12  | bar_result_to_decisions blocks short-open (B5 backstop still intact)      |
//! | I13  | Long-only integration: positive target → LongOpen → buy decision          |
//! | I14  | BuyToFlat (exact short cover) is not a short entry                        |
//! | I15  | order_intent_to_short_entry_intent mapping is correct for all variants    |

use std::collections::BTreeMap;

use uuid::Uuid;

use mqk_daemon::capital_policy::{
    classify_order_intent, evaluate_short_entry_policy, order_intent_to_short_entry_intent,
    OrderIntent, ShortEntryConfig, ShortEntryIntent, ShortEntryPolicyDecision,
};
use mqk_daemon::decision::bar_result_to_decisions;
use mqk_strategy::{
    IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec, TargetPosition,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_result(targets: Vec<TargetPosition>) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("test_strategy", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput { targets },
        },
    }
}

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_NOW_MICROS: i64 = 1_700_000_000_000_000;

fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

const PAPER: bool = true;
const LIVE: bool = false;

// ---------------------------------------------------------------------------
// I01 — Sell from flat → ShortOpen
// ---------------------------------------------------------------------------

#[test]
fn i01_sell_from_flat_is_short_open() {
    let intent = classify_order_intent(0, -10);
    assert_eq!(
        intent,
        OrderIntent::ShortOpen,
        "I01: current=0, delta=-10 → ShortOpen"
    );
    assert!(
        intent.requires_short_entry_policy(),
        "I01: ShortOpen requires short-entry policy authorization"
    );
}

// ---------------------------------------------------------------------------
// I02 — Sell beyond long holdings → SellBeyondLongToShort
// ---------------------------------------------------------------------------

#[test]
fn i02_sell_beyond_long_is_sell_beyond_long_to_short() {
    // current=5, delta=-8: qty_to_sell=8 > current=5 → excess drives net-short
    let intent = classify_order_intent(5, -8);
    assert_eq!(
        intent,
        OrderIntent::SellBeyondLongToShort,
        "I02: sell(8) > holdings(5) → SellBeyondLongToShort"
    );
    assert!(
        intent.requires_short_entry_policy(),
        "I02: SellBeyondLongToShort requires short-entry policy authorization"
    );
}

// ---------------------------------------------------------------------------
// I03 — Sell less than long holdings → SellToClose; policy gate not applicable
// ---------------------------------------------------------------------------

#[test]
fn i03_sell_partial_long_is_sell_to_close() {
    // current=10, delta=-4: partial reduce; remains long at 6
    let intent = classify_order_intent(10, -4);
    assert_eq!(
        intent,
        OrderIntent::SellToClose,
        "I03: partial reduce → SellToClose"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I03: SellToClose does not require short-entry policy"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I03: SellToClose → NotShortOpen; gate not applicable"
    );
}

// ---------------------------------------------------------------------------
// I04 — Sell exactly long holdings → SellToFlat; policy gate not applicable
// ---------------------------------------------------------------------------

#[test]
fn i04_sell_exactly_to_flat_is_sell_to_flat() {
    // current=7, delta=-7: sell exactly to flat
    let intent = classify_order_intent(7, -7);
    assert_eq!(
        intent,
        OrderIntent::SellToFlat,
        "I04: sell exactly to flat → SellToFlat"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I04: SellToFlat does not require short-entry policy"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I04: SellToFlat → NotShortOpen; gate not applicable"
    );
}

// ---------------------------------------------------------------------------
// I05 — Buy from flat → LongOpen; not a short entry
// ---------------------------------------------------------------------------

#[test]
fn i05_buy_from_flat_is_long_open() {
    let intent = classify_order_intent(0, 10);
    assert_eq!(
        intent,
        OrderIntent::LongOpen,
        "I05: buy from flat → LongOpen"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I05: LongOpen is not a short entry"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I05: LongOpen → NotShortOpen"
    );
}

// ---------------------------------------------------------------------------
// I06 — Buy while short (partial cover) → BuyToCover; not a short entry
// ---------------------------------------------------------------------------

#[test]
fn i06_buy_while_short_partial_is_buy_to_cover() {
    // current=-5, delta=+3: partial cover; remains short at -2
    let intent = classify_order_intent(-5, 3);
    assert_eq!(
        intent,
        OrderIntent::BuyToCover,
        "I06: partial cover → BuyToCover"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I06: BuyToCover is risk-reducing; not a short entry"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I06: BuyToCover → NotShortOpen (gate not applicable)"
    );
}

// ---------------------------------------------------------------------------
// I07 — Buy more than current short → BuyBeyondShortToLong; not a short entry
// ---------------------------------------------------------------------------

#[test]
fn i07_buy_beyond_short_is_buy_beyond_short_to_long() {
    // current=-5, delta=+8: covers short and opens long; result is +3
    let intent = classify_order_intent(-5, 8);
    assert_eq!(
        intent,
        OrderIntent::BuyBeyondShortToLong,
        "I07: buy(8) > abs(short=5) → BuyBeyondShortToLong"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I07: not a short entry"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I07: BuyBeyondShortToLong → NotShortOpen"
    );
}

// ---------------------------------------------------------------------------
// I08 — Default policy blocks ShortOpen with reason "short_entries_disabled"
// ---------------------------------------------------------------------------

#[test]
fn i08_default_policy_blocks_short_open_with_reason() {
    let intent = classify_order_intent(0, -5);
    assert_eq!(intent, OrderIntent::ShortOpen, "I08: setup");

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "short_entries_disabled"
        ),
        "I08: default policy must block with short_entries_disabled; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// I09 — Default policy blocks SellBeyondLongToShort with reason code
// ---------------------------------------------------------------------------

#[test]
fn i09_default_policy_blocks_sell_beyond_long_to_short() {
    // current=3, delta=-7: sell beyond holdings → SellBeyondLongToShort
    let intent = classify_order_intent(3, -7);
    assert_eq!(intent, OrderIntent::SellBeyondLongToShort, "I09: setup");

    let short_entry_intent = order_intent_to_short_entry_intent(intent);
    assert_eq!(
        short_entry_intent,
        ShortEntryIntent::ShortOpen,
        "I09: SellBeyondLongToShort maps to ShortEntryIntent::ShortOpen"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        short_entry_intent,
        PAPER,
        None,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "short_entries_disabled"
        ),
        "I09: SellBeyondLongToShort must be blocked by default policy; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// I10 — Live mode blocks short-open even if all config flags are true
// ---------------------------------------------------------------------------

#[test]
fn i10_live_mode_blocks_short_open_unconditionally() {
    let permissive = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: true,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_order_intent(0, -10);
    let decision = evaluate_short_entry_policy(
        &permissive,
        order_intent_to_short_entry_intent(intent),
        LIVE, // live mode: always blocked regardless of config
        Some(true),
        Some(10),
        Some(1000.0),
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "live_short_entries_blocked"
        ),
        "I10: live mode must always block with live_short_entries_blocked; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// I11 — Unknown shortable status blocks when require_shortable_check=true
// ---------------------------------------------------------------------------

#[test]
fn i11_unknown_shortable_blocks_when_check_required() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: true,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_order_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config,
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None, // shortable status unknown
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_check_required"
        ),
        "I11: unknown shortable + require_shortable_check=true must block; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// I12 — bar_result_to_decisions blocks short-open (B5 backstop still intact)
// ---------------------------------------------------------------------------

#[test]
fn i12_bar_result_b5_backstop_blocks_short_open_from_flat() {
    let result = live_result(vec![TargetPosition::new("AAPL", -10)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());
    assert!(
        decisions.is_empty(),
        "I12: sell from flat must be blocked by B5 backstop; got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// I13 — Long-only integration: positive target → LongOpen → buy decision
// ---------------------------------------------------------------------------

#[test]
fn i13_long_only_strategy_produces_long_open_and_buy_decision() {
    // buy from flat
    let intent_flat = classify_order_intent(0, 5);
    assert_eq!(
        intent_flat,
        OrderIntent::LongOpen,
        "I13: buy from flat → LongOpen"
    );
    assert!(!intent_flat.requires_short_entry_policy());

    // add to existing long
    let intent_add = classify_order_intent(3, 2);
    assert_eq!(
        intent_add,
        OrderIntent::LongOpen,
        "I13: add to long → LongOpen"
    );
    assert!(!intent_add.requires_short_entry_policy());

    // integration: bar_result_to_decisions produces a buy
    let result = live_result(vec![TargetPosition::new("AAPL", 5)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());
    assert_eq!(
        decisions.len(),
        1,
        "I13: long-only target → one buy decision"
    );
    assert_eq!(decisions[0].side, "buy");
    assert_eq!(decisions[0].qty, 5);
}

// ---------------------------------------------------------------------------
// I14 — BuyToFlat (exact short cover) is not a short entry
// ---------------------------------------------------------------------------

#[test]
fn i14_buy_to_flat_is_not_short_entry() {
    // current=-5, delta=+5: covering exactly to flat
    let intent = classify_order_intent(-5, 5);
    assert_eq!(
        intent,
        OrderIntent::BuyToFlat,
        "I14: cover exactly → BuyToFlat"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "I14: BuyToFlat is not a short entry"
    );

    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        order_intent_to_short_entry_intent(intent),
        PAPER,
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "I14: BuyToFlat → NotShortOpen; gate not applicable"
    );
}

// ---------------------------------------------------------------------------
// I15 — order_intent_to_short_entry_intent mapping is correct for all variants
// ---------------------------------------------------------------------------

#[test]
fn i15_order_intent_to_short_entry_intent_mapping() {
    // Short-open intents → ShortEntryIntent::ShortOpen
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::ShortOpen),
        ShortEntryIntent::ShortOpen,
        "I15: ShortOpen"
    );
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::SellBeyondLongToShort),
        ShortEntryIntent::ShortOpen,
        "I15: SellBeyondLongToShort"
    );

    // Cover intents → ShortEntryIntent::BuyToCover
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::BuyToCover),
        ShortEntryIntent::BuyToCover,
        "I15: BuyToCover"
    );
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::BuyToFlat),
        ShortEntryIntent::BuyToCover,
        "I15: BuyToFlat"
    );
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::BuyBeyondShortToLong),
        ShortEntryIntent::BuyToCover,
        "I15: BuyBeyondShortToLong"
    );

    // Sell-within-long → ShortEntryIntent::LongClose
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::SellToClose),
        ShortEntryIntent::LongClose,
        "I15: SellToClose"
    );
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::SellToFlat),
        ShortEntryIntent::LongClose,
        "I15: SellToFlat"
    );

    // Buy and NoOp → ShortEntryIntent::LongOpen
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::LongOpen),
        ShortEntryIntent::LongOpen,
        "I15: LongOpen"
    );
    assert_eq!(
        order_intent_to_short_entry_intent(OrderIntent::NoOp),
        ShortEntryIntent::LongOpen,
        "I15: NoOp"
    );
}
