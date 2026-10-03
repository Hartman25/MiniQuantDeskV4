//! FixedInitialCapitalFractionV1 pure resolver: integer math, refusals,
//! reduce-only caps, no +1 fallback.

use mqk_execution::{AssetClass, QTY_MICROS_SCALE};
use mqk_strategy::{
    resolve_capital_fraction_target, CapitalFractionRefusal, SizingPolicy, TargetSizing,
};

const USD: i64 = 1_000_000;

fn uncapped() -> TargetSizing {
    TargetSizing::equity_default()
}

fn shares(n: i64) -> i64 {
    n * QTY_MICROS_SCALE
}

#[test]
fn floors_budget_and_quantity_exactly() {
    // 100_000 USD * 25% = 25_000 USD budget; / 333.33 USD -> floor 75 shares.
    let r = resolve_capital_fraction_target(2_500, 100_000 * USD, 333_330_000, &uncapped())
        .expect("resolves");
    assert_eq!(r.position_budget_micros, 25_000 * USD);
    assert_eq!(r.resolved_target_qty.raw(), shares(75));
    assert_eq!(r.uncapped_target_qty.raw(), shares(75));
    assert_eq!(r.capped_by, "none");
    assert_eq!(r.sizing_policy_id, "fixed_initial_capital_fraction_v1");
    assert_eq!(r.allocation_fraction_bps, 2_500);
    assert_eq!(r.causal_reference_price_micros, 333_330_000);
}

#[test]
fn never_rounds_up_at_exact_boundary_minus_one_micro() {
    // budget exactly 10 * price - 1 micro => 9 shares, not 10.
    let price = 10 * USD;
    let capital = 10 * price - 1; // 100% bps
    let r = resolve_capital_fraction_target(10_000, capital, price, &uncapped()).unwrap();
    assert_eq!(r.resolved_target_qty.raw(), shares(9));
    let r = resolve_capital_fraction_target(10_000, capital + 1, price, &uncapped()).unwrap();
    assert_eq!(r.resolved_target_qty.raw(), shares(10));
}

#[test]
fn budget_floors_before_division() {
    // 1 bps of 9_999 micros = floor(0.9999)=0 -> refused as non-positive budget.
    let e = resolve_capital_fraction_target(1, 9_999, USD, &uncapped()).unwrap_err();
    assert!(matches!(
        e,
        CapitalFractionRefusal::NonPositiveBudget { .. }
    ));
}

#[test]
fn insufficient_budget_is_refused_not_one_share() {
    // Budget 50 USD vs price 100 USD: cannot buy one share.
    let e = resolve_capital_fraction_target(500, 1_000 * USD, 100 * USD, &uncapped()).unwrap_err();
    assert_eq!(e.reason_code(), "insufficient_budget_for_minimum_quantity");
}

#[test]
fn fraction_bounds_are_enforced() {
    for bps in [0i64, -1, 10_001, i64::MAX, i64::MIN] {
        let e =
            resolve_capital_fraction_target(bps, 100_000 * USD, 10 * USD, &uncapped()).unwrap_err();
        assert_eq!(e.reason_code(), "invalid_allocation_fraction_bps", "{bps}");
        assert!(SizingPolicy::capital_fraction_v1(bps).is_err());
    }
    assert!(SizingPolicy::capital_fraction_v1(1).is_ok());
    assert!(SizingPolicy::capital_fraction_v1(10_000).is_ok());
    assert_eq!(
        resolve_capital_fraction_target(10_000, 100 * USD, 10 * USD, &uncapped())
            .unwrap()
            .resolved_target_qty
            .raw(),
        shares(10)
    );
}

#[test]
fn non_positive_capital_and_price_are_refused() {
    for cap in [0i64, -5 * USD] {
        let e = resolve_capital_fraction_target(1_000, cap, 10 * USD, &uncapped()).unwrap_err();
        assert_eq!(e.reason_code(), "non_positive_initial_capital");
    }
    for px in [0i64, -1] {
        let e = resolve_capital_fraction_target(1_000, 100 * USD, px, &uncapped()).unwrap_err();
        assert_eq!(e.reason_code(), "non_positive_reference_price");
    }
}

#[test]
fn large_values_use_wide_math_without_overflow_or_refuse() {
    // capital * bps overflows i64 but fits i128 (no panic/wrap); a 1-micro
    // price makes the share count overflow QtyMicros and must be refused.
    let r = resolve_capital_fraction_target(10_000, i64::MAX, 1, &uncapped());
    // Quantity micros overflow i64 -> refused deterministically, never wrapped.
    assert_eq!(r.unwrap_err(), CapitalFractionRefusal::BudgetOverflow);
}

#[test]
fn caps_can_only_reduce_or_refuse() {
    // Budget buys 100 shares; max_target_qty 40 reduces.
    let caps = TargetSizing::equity_whole_units(1, Some(40), None).unwrap();
    let r = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &caps).unwrap();
    assert_eq!(r.uncapped_target_qty.raw(), shares(100));
    assert_eq!(r.resolved_target_qty.raw(), shares(40));
    assert_eq!(r.capped_by, "max_qty");

    // Notional cap $250 at $10 => 25 shares.
    let caps = TargetSizing::equity_whole_units(1, Some(40), Some(250)).unwrap();
    let r = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &caps).unwrap();
    assert_eq!(r.resolved_target_qty.raw(), shares(25));
    assert_eq!(r.capped_by, "max_notional");

    // A cap larger than the budget never raises the quantity.
    let caps = TargetSizing::equity_whole_units(1, Some(1_000), Some(1_000_000)).unwrap();
    let r = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &caps).unwrap();
    assert_eq!(r.resolved_target_qty.raw(), shares(100));
    assert_eq!(r.capped_by, "none");

    // The legacy target_qty field is ignored: target 7 does not set Q.
    let caps = TargetSizing::equity_whole_units(7, None, None).unwrap();
    let r = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &caps).unwrap();
    assert_eq!(r.resolved_target_qty.raw(), shares(100));
}

#[test]
fn caps_reducing_below_one_share_refuse_instead_of_falling_back() {
    // Notional cap $5 at $10 => 0 shares => refused (never +1).
    let caps = TargetSizing::equity_whole_units(1, None, Some(5)).unwrap();
    let e = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &caps).unwrap_err();
    assert_eq!(
        e,
        CapitalFractionRefusal::CapsReducedBelowMinimumQuantity {
            capped_by: "max_notional"
        }
    );
}

#[test]
fn non_equity_asset_class_is_refused() {
    let crypto = TargetSizing::resolve(AssetClass::Crypto, Some("0.5"), None, None).unwrap();
    let e = resolve_capital_fraction_target(10_000, 1_000 * USD, 10 * USD, &crypto).unwrap_err();
    assert_eq!(e.reason_code(), "unsupported_asset_class");
}

#[test]
fn resolution_is_deterministic() {
    let a = resolve_capital_fraction_target(1_234, 98_765 * USD, 123_456_789, &uncapped());
    let b = resolve_capital_fraction_target(1_234, 98_765 * USD, 123_456_789, &uncapped());
    assert_eq!(a, b);
}

#[test]
fn apply_caps_matches_legacy_scalper_semantics() {
    let caps = TargetSizing::equity_whole_units(1, Some(10), Some(1_000)).unwrap();
    // $1000 cap at $200 => 5 shares, below the 10 qty cap.
    let (q, why) = caps.apply_caps(mqk_execution::QtyMicros::new(shares(8)), 200 * USD);
    assert_eq!((q.raw(), why), (shares(5), "max_notional"));
    // No price + notional cap fails closed to zero.
    let (q, why) = caps.apply_caps(mqk_execution::QtyMicros::new(shares(8)), 0);
    assert_eq!((q.raw(), why), (0, "max_notional_no_price"));
}
