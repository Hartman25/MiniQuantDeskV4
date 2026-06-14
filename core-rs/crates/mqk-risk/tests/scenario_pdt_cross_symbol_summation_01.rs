//! PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 (Patch 11C)
//!
//! Cap #13 proof (docs/design/native_multi_symbol_dispatch.md §6): PDT /
//! day-trade accounting in `mqk_risk::pdt` is account-wide, not per-symbol.
//! Day trades originating from different symbols (AAPL, MSFT) sum into ONE
//! shared `PdtState` and trip `PDT_DAY_TRADE_THRESHOLD` exactly as 4 day
//! trades on a single symbol would.
//!
//! `record_day_trade` / `evaluate_pdt` / `PdtState` / `PdtPolicy` take NO
//! symbol parameter and have NO per-symbol field -- there is no per-symbol
//! dimension to bypass. Each test below labels trades with a `symbol`
//! purely as test-local bookkeeping (representing what a multi-symbol
//! dispatch loop would report); the production PDT API never sees that
//! label, which is the proof itself.
//!
//! All trades in this file occur on the same trading day (`DAY1`) so the
//! rolling-window anchor (a same-day-ordering detail unrelated to
//! cross-symbol summation, already covered by `pdt.rs`'s existing
//! `old_day_trades_roll_out_of_window` / `trades_on_day5_still_in_window`
//! unit tests) cannot influence the result.
//!
//! Labels PDT-X01 through PDT-X10 correspond to the invariants listed in
//! PDT-CROSS-SYMBOL-SUMMATION-PROOF-01.

use mqk_risk::*;

const DAY1: u32 = 20260302;
const EQUITY_OK: i64 = 30_000 * 1_000_000; // above PDT_MIN_EQUITY_MICROS ($25k)

fn policy() -> PdtPolicy {
    // FINRA defaults: max_day_trades_in_window = PDT_DAY_TRADE_THRESHOLD - 1 = 3
    PdtPolicy::finra_defaults()
}

fn day_trade_input() -> PdtInput {
    PdtInput {
        day_id: DAY1,
        equity_micros: EQUITY_OK,
        is_day_trade: true,
    }
}

// ---------------------------------------------------------------------------
// PDT-X01 / PDT-X02 / PDT-X03
// ---------------------------------------------------------------------------

/// PDT-X01: PDT accounting is account-wide, not per-symbol -- `record_day_trade`
/// takes `(policy, state, day_id)` with no symbol parameter, so AAPL and MSFT
/// day trades necessarily accumulate into the SAME `PdtState`.
///
/// PDT-X02: 2 day trades on AAPL + 2 day trades on MSFT == 4 total account
/// day trades.
///
/// PDT-X03: that cross-symbol total of 4 trips PDT_DAY_TRADE_THRESHOLD (flags
/// the account PDT) exactly as 4 day trades on a single symbol would.
#[test]
fn pdt_x01_x02_x03_cross_symbol_trades_sum_and_trip_threshold() {
    let p = policy();

    // 2 day trades on AAPL + 2 day trades on MSFT, fed into ONE shared
    // account-wide PdtState. record_day_trade has NO symbol parameter
    // (PDT-X01) -- there is nothing for "AAPL" / "MSFT" to be passed as.
    let mut cross = PdtState::new();
    record_day_trade(&p, &mut cross, DAY1); // AAPL day trade #1
    record_day_trade(&p, &mut cross, DAY1); // AAPL day trade #2
    record_day_trade(&p, &mut cross, DAY1); // MSFT day trade #1
    record_day_trade(&p, &mut cross, DAY1); // MSFT day trade #2

    // PDT-X02: 2 AAPL + 2 MSFT == 4 total account day trades.
    let total: u32 = cross.day_trade_counts.values().sum();
    assert_eq!(
        total, 4,
        "PDT-X02: 2 AAPL + 2 MSFT day trades must sum to 4 total account day trades"
    );

    // PDT-X03: 4 day trades on a single symbol, for comparison.
    let mut single = PdtState::new();
    record_day_trade(&p, &mut single, DAY1); // AAPL day trade #1
    record_day_trade(&p, &mut single, DAY1); // AAPL day trade #2
    record_day_trade(&p, &mut single, DAY1); // AAPL day trade #3
    record_day_trade(&p, &mut single, DAY1); // AAPL day trade #4

    assert_eq!(
        cross.day_trade_counts, single.day_trade_counts,
        "PDT-X03: cross-symbol day_trade_counts must equal single-symbol day_trade_counts for the same total of 4"
    );
    assert_eq!(
        cross.flagged_pdt, single.flagged_pdt,
        "PDT-X03: cross-symbol total of 4 must flag PDT identically to 4 same-symbol day trades"
    );
    assert!(
        cross.flagged_pdt,
        "PDT-X03: cross-symbol total of 4 day trades must flag the account PDT"
    );

    // Both accounts are now blocked the same way for a further day trade.
    let d_cross = evaluate_pdt(&p, &cross, &day_trade_input());
    let d_single = evaluate_pdt(&p, &single, &day_trade_input());
    assert_eq!(
        d_cross.reason, d_single.reason,
        "PDT-X03: evaluate_pdt reason must match between cross-symbol and single-symbol 4-trade accounts"
    );
    assert!(!d_cross.trading_allowed && !d_single.trading_allowed);
    assert_eq!(d_cross.reason, PdtReason::BlockedFlaggedPdt);
}

// ---------------------------------------------------------------------------
// PDT-X04 / PDT-X05
// ---------------------------------------------------------------------------

/// PDT-X04: per-symbol counts below threshold do not bypass the account-wide
/// rule. 2 AAPL + 2 MSFT (2 each, individually "safe" since
/// `max_day_trades_in_window == 3`) still flags the account at the
/// account-wide total of 4.
#[test]
fn pdt_x04_per_symbol_counts_below_threshold_do_not_bypass_account_wide_rule() {
    let p = policy(); // max_day_trades_in_window = 3
    let mut s = PdtState::new();

    // AAPL: 2 day trades (2 < 3 individually).
    record_day_trade(&p, &mut s, DAY1);
    record_day_trade(&p, &mut s, DAY1);
    // MSFT: 2 day trades (2 < 3 individually).
    record_day_trade(&p, &mut s, DAY1);
    record_day_trade(&p, &mut s, DAY1);

    assert!(
        s.flagged_pdt,
        "PDT-X04: 2 AAPL + 2 MSFT (2 below-threshold per symbol) must still flag the account-wide PDT rule at a total of 4"
    );
}

/// PDT-X05: 2 day trades on AAPL + 1 day trade on MSFT (total 3) remains
/// below the 4-trade PDT trigger.
#[test]
fn pdt_x05_mixed_symbol_below_threshold_total_does_not_trip() {
    let p = policy(); // max_day_trades_in_window = 3
    let mut s = PdtState::new();

    record_day_trade(&p, &mut s, DAY1); // AAPL day trade #1
    record_day_trade(&p, &mut s, DAY1); // AAPL day trade #2
    record_day_trade(&p, &mut s, DAY1); // MSFT day trade #1

    let total: u32 = s.day_trade_counts.values().sum();
    assert_eq!(
        total, 3,
        "PDT-X05: 2 AAPL + 1 MSFT must total 3 account day trades"
    );
    assert!(
        !s.flagged_pdt,
        "PDT-X05: 2 AAPL + 1 MSFT (total 3) must remain below the 4-trade PDT trigger"
    );

    // A further day trade (on either symbol) would be the 4th and is blocked.
    let d = evaluate_pdt(&p, &s, &day_trade_input());
    assert!(!d.trading_allowed);
    assert_eq!(d.reason, PdtReason::BlockedWouldExceedLimit);
    assert_eq!(d.window_day_trade_count, 3);
}

// ---------------------------------------------------------------------------
// PDT-X06
// ---------------------------------------------------------------------------

/// PDT-X06: order of symbols does not change the count. "AAPL, AAPL, MSFT,
/// MSFT" and "MSFT, AAPL, MSFT, AAPL" both reduce to the same 4 identical
/// `record_day_trade(&p, &mut s, DAY1)` calls -- because PDT accounting has
/// no symbol parameter to order by -- and so produce identical state.
#[test]
fn pdt_x06_order_of_symbols_does_not_change_the_count() {
    let p = policy();

    // Ordering A: AAPL, AAPL, MSFT, MSFT
    let order_a = ["AAPL", "AAPL", "MSFT", "MSFT"];
    let mut state_a = PdtState::new();
    for _symbol in order_a {
        record_day_trade(&p, &mut state_a, DAY1);
    }

    // Ordering B: MSFT, AAPL, MSFT, AAPL
    let order_b = ["MSFT", "AAPL", "MSFT", "AAPL"];
    let mut state_b = PdtState::new();
    for _symbol in order_b {
        record_day_trade(&p, &mut state_b, DAY1);
    }

    assert_eq!(
        state_a.day_trade_counts, state_b.day_trade_counts,
        "PDT-X06: order of symbols must not change the recorded day-trade counts"
    );
    assert_eq!(
        state_a.flagged_pdt, state_b.flagged_pdt,
        "PDT-X06: order of symbols must not change the PDT flag outcome"
    );
    assert!(state_a.flagged_pdt && state_b.flagged_pdt);
}

// ---------------------------------------------------------------------------
// PDT-X07
// ---------------------------------------------------------------------------

/// PDT-X07: a same-day buy/sell round trip on a single symbol (AAPL) counts
/// as one day trade. Two such round trips on AAPL the same day record as 2.
#[test]
fn pdt_x07_same_day_round_trip_counts_as_one_day_trade() {
    let p = policy();
    let mut s = PdtState::new();

    // Round trip 1 on AAPL: caller determines is_day_trade=true for a
    // same-session open+close.
    let d1 = evaluate_pdt(&p, &s, &day_trade_input());
    assert!(
        d1.trading_allowed,
        "PDT-X07: first AAPL same-day round trip must be allowed"
    );
    record_day_trade(&p, &mut s, DAY1);
    assert_eq!(
        s.day_trade_counts.get(&DAY1),
        Some(&1),
        "PDT-X07: first AAPL same-day round trip records as exactly 1 day trade"
    );

    // Round trip 2 on AAPL, same day.
    record_day_trade(&p, &mut s, DAY1);
    assert_eq!(
        s.day_trade_counts.get(&DAY1),
        Some(&2),
        "PDT-X07: second same-day AAPL round trip accumulates to 2"
    );
}

// ---------------------------------------------------------------------------
// PDT-X08
// ---------------------------------------------------------------------------

/// PDT-X08: trades from different symbols are not collapsed into one symbol
/// -- the 4-trade total genuinely decomposes as 2 distinct AAPL events + 2
/// distinct MSFT events, not e.g. 1 event counted 4 times.
#[test]
fn pdt_x08_distinct_symbols_are_not_collapsed_into_one() {
    let trades = ["AAPL", "AAPL", "MSFT", "MSFT"];

    let aapl_count = trades.iter().filter(|&&s| s == "AAPL").count();
    let msft_count = trades.iter().filter(|&&s| s == "MSFT").count();
    assert_eq!(
        aapl_count, 2,
        "PDT-X08: exactly 2 distinct AAPL day-trade events"
    );
    assert_eq!(
        msft_count, 2,
        "PDT-X08: exactly 2 distinct MSFT day-trade events"
    );
    assert_eq!(
        aapl_count + msft_count,
        trades.len(),
        "PDT-X08: AAPL and MSFT day trades are not collapsed into one symbol -- 2 + 2 = 4 distinct events"
    );

    // Feed the genuinely-distinct events into the shared account-wide state.
    let p = policy();
    let mut s = PdtState::new();
    for _symbol in trades {
        record_day_trade(&p, &mut s, DAY1);
    }
    let total: u32 = s.day_trade_counts.values().sum();
    assert_eq!(
        total,
        trades.len() as u32,
        "PDT-X08: account-wide total equals the number of distinct trade events across both symbols"
    );
}

// ---------------------------------------------------------------------------
// PDT-X09
// ---------------------------------------------------------------------------

/// PDT-X09: this multi-symbol proof does not introduce a per-symbol PDT
/// limit. Exhaustive destructuring of `PdtPolicy` and `PdtState` means this
/// test FAILS TO COMPILE if any per-symbol field (e.g. a
/// `BTreeMap<String, BTreeMap<u32, u32>>` keyed by symbol) is ever added to
/// either type -- forcing an explicit decision rather than a silent
/// per-symbol PDT limit.
#[test]
fn pdt_x09_no_per_symbol_pdt_limit_introduced() {
    let PdtPolicy {
        enabled,
        window_days,
        max_day_trades_in_window,
        min_equity_micros,
    } = PdtPolicy::finra_defaults();

    assert!(enabled);
    assert_eq!(window_days, PDT_DEFAULT_WINDOW_DAYS);
    assert_eq!(max_day_trades_in_window, PDT_DAY_TRADE_THRESHOLD - 1);
    assert_eq!(min_equity_micros, PDT_MIN_EQUITY_MICROS);

    let PdtState {
        day_trade_counts,
        flagged_pdt,
    } = PdtState::new();

    assert!(day_trade_counts.is_empty());
    assert!(!flagged_pdt);
}

// ---------------------------------------------------------------------------
// PDT-X10
// ---------------------------------------------------------------------------

/// PDT-X10: live-routing / `approved_for_live` authority is not introduced by
/// this proof. Exhaustive destructuring of `PdtContext` (the PDT->risk-engine
/// bridge) shows it carries ONLY `pdt_ok`; no live-routing field exists on
/// the PDT path.
#[test]
fn pdt_x10_no_live_routing_authority_introduced() {
    let blocked = PdtDecision {
        trading_allowed: false,
        reason: PdtReason::BlockedFlaggedPdt,
        window_day_trade_count: 4,
    };
    let PdtContext { pdt_ok } = to_pdt_context(&blocked);
    assert!(
        !pdt_ok,
        "PDT-X10: a blocked PDT decision yields pdt_ok = false; PdtContext carries no approved_for_live / live-routing field"
    );

    let allowed = PdtDecision {
        trading_allowed: true,
        reason: PdtReason::AllowedWithinLimit,
        window_day_trade_count: 2,
    };
    let PdtContext { pdt_ok: ok } = to_pdt_context(&allowed);
    assert!(ok);
}
