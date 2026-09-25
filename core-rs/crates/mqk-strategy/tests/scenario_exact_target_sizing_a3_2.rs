//! CUTOVER-1D-A3-2: exact QtyMicros target sizing.
//!
//! Proves: Equity behavior/identity unchanged; Crypto sizing exact and never
//! defaulted; fractional targets survive `on_bar` exactly; asset class gates
//! which engines can be registered.

use mqk_execution::{AssetClass, QtyMicros, QTY_MICROS_SCALE};
use mqk_strategy::engines::intraday_scalper::{IntradayScalperStrategy, SHORT_NAME};
use mqk_strategy::engines::{
    register_builtin_strategies_with_sizing, register_builtin_strategies_with_target_sizing,
};
const NAME: &str = "intraday_scalper";

use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, SizingError, Strategy, StrategyContext, TargetSizing,
};

fn bar(close_micros: i64) -> BarStub {
    BarStub::new(0, true, close_micros, 1)
}

fn ctx(bars: Vec<BarStub>) -> StrategyContext {
    StrategyContext::new(300, 0, RecentBarsWindow::new(10, bars))
}

fn bullish(base: i64) -> Vec<BarStub> {
    vec![
        bar(base),
        bar(base),
        bar(base),
        bar(base),
        bar(base + base / 400),
    ]
}

fn bearish(base: i64) -> Vec<BarStub> {
    let hi = base + base / 400;
    vec![bar(hi), bar(hi), bar(hi), bar(hi), bar(base)]
}

fn crypto(target: &str, max: Option<&str>, notional: Option<&str>) -> TargetSizing {
    TargetSizing::resolve(AssetClass::Crypto, Some(target), max, notional).unwrap()
}

fn target_raw(s: &mut IntradayScalperStrategy, bars: Vec<BarStub>) -> i64 {
    let out = s.on_bar(&ctx(bars));
    assert_eq!(out.targets.len(), 1);
    out.targets[0].qty.raw()
}

/// Equity fingerprints are byte-identical to the pre-QtyMicros encoding.
/// Golden values were captured from the whole-unit implementation.
#[test]
fn equity_fingerprints_are_unchanged() {
    let a = IntradayScalperStrategy::with_caps("AAPL", 3, Some(5), Some(1000)).short_signals(true);
    let b = IntradayScalperStrategy::with_caps("AAPL", 1, None, None);
    assert_eq!(
        a.semantic_fingerprint(),
        "615c62525b1f76a60dd02af9bd9688e0d6692910e33e7e3304c7f99a50d9720c"
    );
    assert_eq!(
        b.semantic_fingerprint(),
        "40413c2fd48ed8fed134ab578ebb28bbe83bf740f731b3d3ff90ac4b609e86f0"
    );
    // The resolver's Equity default is the very same effective sizing.
    let via_sizing = IntradayScalperStrategy::with_sizing("AAPL", TargetSizing::equity_default());
    assert_eq!(via_sizing.semantic_fingerprint(), b.semantic_fingerprint());
}

#[test]
fn equity_one_share_is_scale_micros() {
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
    assert_eq!(target_raw(&mut s, bullish(200_000_000)), QTY_MICROS_SCALE);
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
    assert_eq!(
        target_raw(&mut s, bullish(200_000_000)),
        5 * QTY_MICROS_SCALE
    );
}

#[test]
fn crypto_fractional_target_survives_on_bar_exactly() {
    let mut s = IntradayScalperStrategy::with_sizing("BTC/USD", crypto("0.0001", None, None));
    assert_eq!(target_raw(&mut s, bullish(60_000_000_000)), 100);
    // Bearish long-only => exact flat, not a fractional residue.
    assert_eq!(target_raw(&mut s, bearish(60_000_000_000)), 0);

    let mut short =
        IntradayScalperStrategy::new_short_with_sizing("BTC/USD", crypto("0.000123", None, None));
    assert_eq!(target_raw(&mut short, bearish(60_000_000_000)), -123);
    assert_eq!(target_raw(&mut short, bullish(60_000_000_000)), 0);
}

#[test]
fn fractional_qty_cap_and_notional_cap_floor_exactly() {
    // Quantity cap below target caps to the exact fractional cap.
    let mut s = IntradayScalperStrategy::with_sizing("BTC/USD", crypto("1", Some("0.25"), None));
    assert_eq!(target_raw(&mut s, bullish(60_000_000_000)), 250_000);

    // $100 notional at a 60_000 * (1+1/400) close: floor(100e6*1e6/close).
    let bars = bullish(60_000_000_000);
    let close = bars.last().unwrap().close_micros as i128;
    let expected = ((100i128 * 1_000_000 * QTY_MICROS_SCALE as i128) / close) as i64;
    let mut s = IntradayScalperStrategy::with_sizing("BTC/USD", crypto("1", None, Some("100")));
    let got = target_raw(&mut s, bars);
    assert_eq!(got, expected);
    assert!(
        got > 0 && got < QTY_MICROS_SCALE,
        "fractional, never rounded up to a whole unit"
    );

    // Equity notional cap keeps whole-share flooring: $700 @ 200.5 => 3 shares.
    let mut e = IntradayScalperStrategy::with_caps("AAPL", 10, None, Some(700));
    assert_eq!(
        target_raw(&mut e, bullish(200_000_000)),
        3 * QTY_MICROS_SCALE
    );
}

#[test]
fn quantity_changes_identity_and_asset_class_is_bound() {
    let f = |sizing| IntradayScalperStrategy::with_sizing("BTC/USD", sizing).semantic_fingerprint();
    let a = f(crypto("0.0001", None, None));
    assert_eq!(a, f(crypto("0.0001", None, None)), "deterministic");
    assert_ne!(a, f(crypto("0.0002", None, None)));
    assert_ne!(a, f(crypto("0.0001", Some("0.5"), None)));
    assert_ne!(a, f(crypto("0.0001", None, Some("10"))));
    // 1 whole crypto unit does not collide with 1 equity share.
    let one_crypto = f(crypto("1", None, None));
    let one_equity =
        IntradayScalperStrategy::with_sizing("BTC/USD", TargetSizing::equity_default())
            .semantic_fingerprint();
    assert_ne!(one_crypto, one_equity);
    // Fractional encodings never alias a whole quantity.
    assert_ne!(
        f(crypto("0.000001", None, None)),
        f(crypto("1", None, None))
    );
}

#[test]
fn crypto_sizing_fails_closed_before_any_target() {
    assert_eq!(
        TargetSizing::resolve(AssetClass::Crypto, None, None, None),
        Err(SizingError::MissingExplicitSize {
            asset_class: AssetClass::Crypto
        })
    );
    for bad in ["", "abc", "0", "-1", "0.1234567", "9999999999999"] {
        assert!(
            TargetSizing::resolve(AssetClass::Crypto, Some(bad), None, None).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn non_equity_registration_excludes_fixed_one_share_engines() {
    let mut equity = PluginRegistry::new();
    register_builtin_strategies_with_target_sizing(
        &mut equity,
        "SPY",
        TargetSizing::equity_default(),
    )
    .unwrap();
    assert_eq!(equity.list().len(), 5);

    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_target_sizing(
        &mut reg,
        "BTC/USD",
        crypto("0.0001", None, None),
    )
    .unwrap();
    let names: Vec<&str> = reg.list().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec![NAME, SHORT_NAME]);
    for fixed in ["swing_momentum", "mean_reversion", "volatility_breakout"] {
        assert!(reg.instantiate(fixed).is_err(), "{fixed} must be absent");
    }
    // The sized identity carries the exact fractional target end to end.
    let mut inst = reg.instantiate(NAME).unwrap();
    let out = inst.on_bar(&ctx(bullish(60_000_000_000)));
    assert_eq!(out.targets[0].qty, QtyMicros::new(100));
}

#[test]
fn legacy_whole_unit_registration_is_unchanged() {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 2, None, None).unwrap();
    assert_eq!(reg.list().len(), 5);
    let mut inst = reg.instantiate(NAME).unwrap();
    let out = inst.on_bar(&ctx(bullish(200_000_000)));
    assert_eq!(out.targets[0].qty, QtyMicros::new(2 * QTY_MICROS_SCALE));
}
