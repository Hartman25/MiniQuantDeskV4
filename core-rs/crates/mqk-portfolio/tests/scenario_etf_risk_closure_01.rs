//! ETF-RISK-CLOSURE-01: proof tests for the pure, bps-based, live-weight-aware
//! sector exposure evaluator (`mqk_portfolio::evaluate_sector_risk`).
//!
//! Distinct from `scenario_portfolio_live_weights_01.rs` (proves
//! `compute_portfolio_weights` itself) and from `constraints.rs`'s existing
//! `check_sector_limits` unit tests (the unrelated, fractional-weight,
//! research-allocator post-allocation check). This file proves the new
//! live, mark-price-driven, pre-trade sector exposure gate.
//!
//! # Proof matrix
//!
//! | Test  | Proves                                                                |
//! |-------|------------------------------------------------------------------------|
//! | SR-01 | Empty limits map -> disabled/allowed, marks/sector_map never consulted |
//! | SR-02 | Configured cap, prospective exposure within cap -> allowed             |
//! | SR-03 | Configured cap, prospective exposure exceeds cap -> denied             |
//! | SR-04 | Sell that reduces an over-cap sector's exposure -> allowed (reducing)  |
//! | SR-05 | Missing mark fails closed only when a cap applies to the symbol       |
//! | SR-06 | NAV unavailable fails closed only when a cap applies to the symbol    |
//! | SR-07 | Unknown/unclassified sector does not panic; allowed, no marks needed  |
//! | SR-08 | Sector known but has no configured cap -> allowed, no marks needed    |
//! | SR-09 | i128-safe math: extreme quantities/prices never panic or wrap         |
//! | SR-10 | Opening a brand-new (currently flat) position in a capped sector      |
//! | SR-11 | Multi-sector isolation: an unrelated sector's exposure is ignored     |
//! | SR-12 | Exactly-at-cap boundary is allowed (only strict `>` denies)           |

use std::collections::{BTreeMap, HashMap};

use mqk_portfolio::{evaluate_sector_risk, PositionMark, PositionWeightInput};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn positions(items: &[(&str, i64)]) -> Vec<PositionWeightInput> {
    items
        .iter()
        .map(|(sym, qty)| PositionWeightInput {
            symbol: sym.to_string(),
            signed_qty: *qty,
        })
        .collect()
}

fn mark(symbol: &str, price_micros: i64) -> (String, PositionMark) {
    (
        symbol.to_string(),
        PositionMark {
            symbol: symbol.to_string(),
            mark_price_micros: price_micros,
            mark_ts_utc: Some(1_790_000_000),
            source: "test:fixture".to_string(),
        },
    )
}

fn marks(items: &[(&str, i64)]) -> BTreeMap<String, PositionMark> {
    items.iter().map(|(sym, px)| mark(sym, *px)).collect()
}

fn sector_map(items: &[(&str, &str)]) -> HashMap<String, String> {
    items
        .iter()
        .map(|(sym, sector)| (sym.to_string(), sector.to_string()))
        .collect()
}

fn limits(items: &[(&str, i64)]) -> HashMap<String, i64> {
    items
        .iter()
        .map(|(sector, bps)| (sector.to_string(), *bps))
        .collect()
}

// ---------------------------------------------------------------------------
// SR-01: disabled (empty limits) never needs marks/sector_map
// ---------------------------------------------------------------------------

#[test]
fn sr01_disabled_config_allows_without_marks_or_sector_map() {
    let cash = 1_000_000_000; // $1000
    let pos = positions(&[("AAPL", 10)]);
    // Deliberately empty/absent marks and sector_map — if the evaluator
    // touched either when disabled, this would panic or return a different
    // truth_state.
    let no_marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    let no_sectors: HashMap<String, String> = HashMap::new();
    let no_limits: HashMap<String, i64> = HashMap::new();

    let eval = evaluate_sector_risk(cash, &pos, &no_marks, &no_sectors, &no_limits, "AAPL", 10);

    assert!(eval.allowed);
    assert_eq!(eval.truth_state, "sector_risk_disabled");
    assert!(eval.reason_code.is_none());
    assert!(eval.sector.is_none());
    assert!(eval.current_weight_bps.is_none());
    assert!(eval.prospective_weight_bps.is_none());
    assert!(eval.max_weight_bps.is_none());
}

// ---------------------------------------------------------------------------
// SR-02 / SR-03: configured cap allows within limit, denies when exceeded
// ---------------------------------------------------------------------------

#[test]
fn sr02_configured_cap_allows_within_limit() {
    // NAV = cash(0) + 10 * $100 = $1000. Buying 2 more -> 12 * $100 = $1200,
    // but cash goes unmodeled (exposure-only check), so NAV is recomputed
    // from cash+positions at the SAME cash baseline: nav = 0 + 1200 = 1200.
    // weight = 1200/1200 = 10000 bps (single-symbol, single-sector porfolio).
    // Cap of 10000 bps must allow this exactly.
    let cash = 0;
    let pos = positions(&[("XLK", 10)]);
    let mk = marks(&[("XLK", 100_000_000)]);
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 10_000)]);

    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 2);

    assert!(eval.allowed, "eval={eval:?}");
    assert_eq!(eval.truth_state, "sector_limit_ok");
    assert_eq!(eval.sector, Some("sector_technology".to_string()));
    assert_eq!(eval.max_weight_bps, Some(10_000));
}

#[test]
fn sr03_configured_cap_denies_exceed() {
    // Two positions in the same sector, cap = 5000 bps (50%).
    // Current: XLK 5 shares @ $100 = $500; XLF 5 @ $100 = $500. cash=0.
    // NAV(current) = 1000; sector gross = (500+500)/1000 = 10000 bps (both
    // tagged same sector for this test) -- already illustrates aggregation.
    // Use a cleaner single-symbol case instead for a crisp boundary:
    let cash = 0;
    let pos = positions(&[("XLK", 10)]); // $100/sh -> $1000 notional
    let mk = marks(&[("XLK", 100_000_000)]);
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 5_000)]); // 50% cap

    // Buying 5 more: prospective qty=15, notional=$1500, nav=$1500 (cash=0),
    // weight = 1500/1500 = 10000 bps > 5000 bps cap. Current weight was
    // already 1000/1000=10000 bps too (single-position portfolio is always
    // 100% of its own NAV) -- so this is NOT risk-reducing (same exposure),
    // and must be denied.
    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 5);

    assert!(!eval.allowed, "eval={eval:?}");
    assert_eq!(eval.truth_state, "sector_limit_exceeded");
    assert!(eval.reason_code.is_some());
    assert_eq!(eval.max_weight_bps, Some(5_000));
}

// ---------------------------------------------------------------------------
// SR-04: risk-reducing sell allowed even though still over cap
// ---------------------------------------------------------------------------

#[test]
fn sr04_risk_reducing_sell_allowed_when_it_lowers_exposure_even_if_still_over_cap() {
    // Single position (XLK, capped sector) plus a fixed cash anchor so NAV
    // does not move in lockstep with XLK's own notional as it shrinks.
    // cash=$1000, price=$100/share.
    // current qty=40: XLK_mv=$4000, NAV=$5000, weight=4000*10000/5000=8000 bps.
    // sell 10 -> qty=30: XLK_mv=$3000, NAV=$4000, weight=3000*10000/4000=7500 bps.
    // 7500 < 8000 -> risk-reducing; still > 6000 bps cap -> must be allowed
    // via the risk-reducing override, not "sector_limit_ok".
    let cash = 1_000_000_000; // $1000
    let pos = positions(&[("XLK", 40)]);
    let mk = marks(&[("XLK", 100_000_000)]); // $100/share
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 6_000)]); // 60% cap

    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", -10);

    assert!(eval.allowed, "eval={eval:?}");
    assert_eq!(eval.truth_state, "sector_risk_reducing_allowed");
    assert_eq!(eval.current_weight_bps, Some(8_000));
    assert_eq!(eval.prospective_weight_bps, Some(7_500));
    assert_eq!(eval.max_weight_bps, Some(6_000));
    assert!(
        eval.prospective_weight_bps.unwrap() > eval.max_weight_bps.unwrap(),
        "sanity: still over cap after the reducing trade"
    );
}

// ---------------------------------------------------------------------------
// SR-05: missing mark fails closed only when a cap applies
// ---------------------------------------------------------------------------

#[test]
fn sr05_missing_mark_fails_closed_only_when_enabled_for_this_sector() {
    let cash = 100_000_000;
    let pos = positions(&[("XLK", 10)]);
    let no_marks: BTreeMap<String, PositionMark> = BTreeMap::new(); // XLK mark absent
    let sectors = sector_map(&[("XLK", "sector_technology")]);

    // Disabled: must allow even though the mark is missing.
    let disabled = evaluate_sector_risk(
        cash,
        &pos,
        &no_marks,
        &sectors,
        &HashMap::new(),
        "XLK",
        5,
    );
    assert!(disabled.allowed);
    assert_eq!(disabled.truth_state, "sector_risk_disabled");

    // Enabled for sector_technology: must fail closed.
    let lims = limits(&[("sector_technology", 5_000)]);
    let enabled = evaluate_sector_risk(cash, &pos, &no_marks, &sectors, &lims, "XLK", 5);
    assert!(!enabled.allowed, "eval={enabled:?}");
    assert_eq!(enabled.truth_state, "sector_weights_missing");
    assert!(enabled.reason_code.unwrap().contains("XLK"));
}

// ---------------------------------------------------------------------------
// SR-06: NAV unavailable fails closed only when a cap applies
// ---------------------------------------------------------------------------

#[test]
fn sr06_nav_unavailable_fails_closed_only_when_enabled_for_this_sector() {
    // Deeply negative cash with no offsetting position value -> NAV <= 0.
    let cash = -1_000_000_000;
    let pos = positions(&[("XLK", 1)]);
    let mk = marks(&[("XLK", 1_000_000)]); // $1/share: nav = -1000 + 1 = still negative
    let sectors = sector_map(&[("XLK", "sector_technology")]);

    let disabled = evaluate_sector_risk(cash, &pos, &mk, &sectors, &HashMap::new(), "XLK", 1);
    assert!(disabled.allowed);
    assert_eq!(disabled.truth_state, "sector_risk_disabled");

    let lims = limits(&[("sector_technology", 5_000)]);
    let enabled = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 1);
    assert!(!enabled.allowed, "eval={enabled:?}");
    assert_eq!(enabled.truth_state, "sector_nav_unavailable");
}

// ---------------------------------------------------------------------------
// SR-07: unknown/unclassified sector does not panic
// ---------------------------------------------------------------------------

#[test]
fn sr07_unclassified_symbol_does_not_panic_and_is_allowed() {
    let cash = 0;
    let pos = positions(&[("AAPL", 10)]);
    let no_marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    let sectors: HashMap<String, String> = HashMap::new(); // AAPL untagged
    let lims = limits(&[("sector_technology", 5_000)]);

    let eval = evaluate_sector_risk(cash, &pos, &no_marks, &sectors, &lims, "AAPL", 5);

    assert!(eval.allowed);
    assert_eq!(eval.truth_state, "sector_metadata_missing");
    assert!(eval.sector.is_none());
}

// ---------------------------------------------------------------------------
// SR-08: sector known, but no cap configured for it -> allowed, no marks
// ---------------------------------------------------------------------------

#[test]
fn sr08_known_sector_without_configured_cap_allows_without_marks() {
    let cash = 0;
    let pos = positions(&[("GLD", 10)]);
    let no_marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    let sectors = sector_map(&[("GLD", "commodity_gold")]);
    // Cap configured for a different sector only.
    let lims = limits(&[("sector_technology", 5_000)]);

    let eval = evaluate_sector_risk(cash, &pos, &no_marks, &sectors, &lims, "GLD", 5);

    assert!(eval.allowed);
    assert_eq!(eval.truth_state, "sector_limit_ok");
    assert_eq!(eval.sector, Some("commodity_gold".to_string()));
    assert!(eval.max_weight_bps.is_none());
}

// ---------------------------------------------------------------------------
// SR-09: i128-safe math under extreme magnitudes
// ---------------------------------------------------------------------------

#[test]
fn sr09_extreme_quantities_and_prices_never_panic() {
    let cash = i64::MAX / 4;
    let pos = positions(&[("XLK", i64::MAX / 4)]);
    let mk = marks(&[("XLK", i64::MAX / 4)]);
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 1)]); // tiny cap, guaranteed breach

    // Must not panic regardless of outcome; this is the proof, not the
    // specific allowed/denied value.
    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", i64::MIN / 4);
    assert!(!eval.truth_state.is_empty());

    // i64::MIN delta is the abs() panic edge case for a naive implementation.
    let eval2 = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", i64::MIN);
    assert!(!eval2.truth_state.is_empty());
}

// ---------------------------------------------------------------------------
// SR-10: opening a brand-new position in a capped sector
// ---------------------------------------------------------------------------

#[test]
fn sr10_opening_new_position_in_capped_sector_is_evaluated() {
    let cash = 1_000_000_000; // $1000, currently all cash, flat everywhere
    let pos: Vec<PositionWeightInput> = vec![]; // no existing positions at all
    let mk = marks(&[("XLK", 100_000_000)]); // candidate symbol's mark must be supplied
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 5_000)]); // 50% cap

    // Buy 6 shares @ $100 = $600 notional. NAV = 1000 (cash unmodeled) + 600 = 1600.
    // weight = 600/1600 = 3750 bps <= 5000 -> allowed.
    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 6);
    assert!(eval.allowed, "eval={eval:?}");
    assert_eq!(eval.current_weight_bps, Some(0), "no prior XLK position");
    assert_eq!(eval.prospective_weight_bps, Some(3_750));

    // Buy 20 shares @ $100 = $2000 notional. NAV = 1000 + 2000 = 3000.
    // weight = 2000/3000 = 6667 bps > 5000 -> denied (not risk-reducing from 0).
    let eval2 = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 20);
    assert!(!eval2.allowed, "eval2={eval2:?}");
    assert_eq!(eval2.truth_state, "sector_limit_exceeded");
}

// ---------------------------------------------------------------------------
// SR-11: multi-sector isolation
// ---------------------------------------------------------------------------

#[test]
fn sr11_unrelated_sector_exposure_is_not_aggregated_in() {
    let cash = 0;
    // XLK (technology) and XLF (financials) both large positions; only
    // sector_technology is capped. XLF must never count toward XLK's check.
    let pos = positions(&[("XLK", 10), ("XLF", 1_000)]);
    let mk = marks(&[("XLK", 100_000_000), ("XLF", 100_000_000)]);
    let sectors = sector_map(&[("XLK", "sector_technology"), ("XLF", "sector_financials")]);
    let lims = limits(&[("sector_technology", 5_000)]);

    // current: XLK=$1000, XLF=$100,000, NAV=$101,000.
    // XLK weight = 1000/101000 ~= 99 bps, far under cap regardless of XLF size.
    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 1);
    assert!(eval.allowed, "eval={eval:?}");
    assert_eq!(eval.truth_state, "sector_limit_ok");
}

// ---------------------------------------------------------------------------
// SR-12: exactly-at-cap boundary is allowed (only strict `>` denies)
// ---------------------------------------------------------------------------

#[test]
fn sr12_exactly_at_cap_boundary_is_allowed() {
    // cash=$1000 anchor (so the currently-flat NAV is positive, not zero);
    // buying 30 shares @ $100 against a cap of exactly 7500 bps:
    // weight = 30*100,000,000*10000 / (1,000,000,000 + 30*100,000,000)
    //        = 30,000,000,000,000 / 4,000,000,000 = 7500 bps exactly.
    let cash = 1_000_000_000; // $1000
    let pos: Vec<PositionWeightInput> = vec![]; // currently flat, not even listed
    let mk = marks(&[("XLK", 100_000_000)]);
    let sectors = sector_map(&[("XLK", "sector_technology")]);
    let lims = limits(&[("sector_technology", 7_500)]);

    // This must be allowed (<=), not denied.
    let eval = evaluate_sector_risk(cash, &pos, &mk, &sectors, &lims, "XLK", 30);
    assert!(eval.allowed, "eval={eval:?}");
    assert_eq!(eval.current_weight_bps, Some(0));
    assert_eq!(eval.prospective_weight_bps, Some(7_500));
    assert_eq!(eval.max_weight_bps, Some(7_500));
    assert_eq!(eval.truth_state, "sector_limit_ok");
}
