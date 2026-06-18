//! SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01 — parallel long/short strategy identity proof.
//!
//! Proves that two independent strategy identities can coexist in the plugin
//! registry and produce distinguishable, non-overlapping outputs:
//!
//! - `"intraday_scalper"`       — long-only (bearish → flat; bullish → +N)
//! - `"intraday_short_scalper"` — short-only (bearish → -N; bullish → flat)
//!
//! # Runtime concurrency verdict
//!
//! `StrategyHost` enforces `MultiStrategyNotAllowed` (exactly one active
//! strategy at a time).  Both identities can be registered in a
//! `PluginRegistry` simultaneously, but the daemon can only select one via
//! `MQK_STRATEGY_IDS`.  True concurrent long + short dispatch on the same
//! symbol requires a later patch: `MULTI-STRATEGY-RUNTIME-DISPATCH-01`.
//!
//! # Test inventory
//!
//! | ID   | Subject                                                          |
//! |------|------------------------------------------------------------------|
//! | P01  | Long strategy default is long-only: bearish → flat (target=0)   |
//! | P02  | Long strategy bullish → positive target                          |
//! | P03  | Long strategy bearish → explicitly zero, not negative            |
//! | P04  | Short strategy constructor exists; spec name = SHORT_NAME        |
//! | P05  | Short strategy bearish → negative target qty                     |
//! | P06  | Short strategy bullish → flat/zero (long direction suppressed)   |
//! | P07  | Short strategy below-threshold → flat/zero                       |
//! | P08  | Long and short strategy spec names are distinct                  |
//! | P09  | Both identities register in PluginRegistry without conflict      |
//! | P10  | PluginRegistry can hold both; instantiate produces correct names |
//! | P11  | StrategyHost enforces single-strategy (documents dispatch limit) |
//! | P12  | Short target magnitude matches sizing (caps apply to magnitude)  |
//! | P13  | B5 logic blocks short-from-flat; target=-1 is not forwarded      |

use mqk_strategy::engines::intraday_scalper::{IntradayScalperStrategy, SHORT_NAME};
use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, ShadowMode, StrategyContext, StrategyHostError,
};
use mqk_strategy::{Strategy, StrategyHost};

const LONG_NAME: &str = "intraday_scalper";
const TIMEFRAME_SECS: i64 = 300; // 5m — must match the strategy constant
const LOOKBACK: usize = 5;

// ---------------------------------------------------------------------------
// Bar helpers
// ---------------------------------------------------------------------------

fn bar(close_micros: i64, is_complete: bool) -> BarStub {
    BarStub::new(0, is_complete, close_micros, 1)
}

fn ctx(bars: Vec<BarStub>) -> StrategyContext {
    StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(LOOKBACK + 5, bars))
}

fn bullish_bars() -> Vec<BarStub> {
    let base = 200_000_000i64; // $200
    vec![
        bar(base, true),
        bar(base, true),
        bar(base, true),
        bar(base, true),
        bar(base + base / 400, true), // +25 bps ≥ 20 bps threshold
    ]
}

fn bearish_bars() -> Vec<BarStub> {
    let base = 200_000_000i64;
    vec![
        bar(base + base / 400, true),
        bar(base + base / 400, true),
        bar(base + base / 400, true),
        bar(base + base / 400, true),
        bar(base, true), // -25 bps ≤ -20 bps threshold
    ]
}

fn flat_bars() -> Vec<BarStub> {
    let base = 200_000_000i64;
    vec![
        bar(base, true),
        bar(base, true),
        bar(base, true),
        bar(base, true),
        bar(base, true), // 0 bps — below threshold
    ]
}

// ---------------------------------------------------------------------------
// P01 — Long strategy default: bearish → flat (long-only by default)
// ---------------------------------------------------------------------------

#[test]
fn p01_long_strategy_bearish_returns_flat() {
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
    let out = s.on_bar(&ctx(bearish_bars()));
    assert_eq!(
        out.targets[0].qty, 0,
        "P01: long-only default: bearish displacement → target=0 (flat, not short)"
    );
}

// ---------------------------------------------------------------------------
// P02 — Long strategy bullish → positive target
// ---------------------------------------------------------------------------

#[test]
fn p02_long_strategy_bullish_returns_positive() {
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 3);
    let out = s.on_bar(&ctx(bullish_bars()));
    assert_eq!(
        out.targets[0].qty, 3,
        "P02: long strategy bullish → target=+3"
    );
}

// ---------------------------------------------------------------------------
// P03 — Long strategy bearish → explicitly not negative
// ---------------------------------------------------------------------------

#[test]
fn p03_long_strategy_bearish_is_not_negative() {
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
    let out = s.on_bar(&ctx(bearish_bars()));
    assert!(
        out.targets[0].qty >= 0,
        "P03: long-only strategy: bearish target must never be negative; got {}",
        out.targets[0].qty
    );
}

// ---------------------------------------------------------------------------
// P04 — Short strategy constructor exists; spec name = SHORT_NAME
// ---------------------------------------------------------------------------

#[test]
fn p04_short_strategy_constructor_and_spec_name() {
    let s = IntradayScalperStrategy::new_short("AAPL");
    assert_eq!(
        s.spec().name,
        SHORT_NAME,
        "P04: new_short() spec.name == SHORT_NAME (\"intraday_short_scalper\")"
    );
    assert_eq!(
        s.spec().timeframe_secs,
        TIMEFRAME_SECS,
        "P04: timeframe_secs matches long variant"
    );
}

// ---------------------------------------------------------------------------
// P05 — Short strategy bearish → negative target qty
// ---------------------------------------------------------------------------

#[test]
fn p05_short_strategy_bearish_returns_negative_target() {
    let mut s = IntradayScalperStrategy::new_short("AAPL");
    let out = s.on_bar(&ctx(bearish_bars()));
    assert!(
        out.targets[0].qty < 0,
        "P05: short strategy bearish → negative target; got {}",
        out.targets[0].qty
    );
    assert_eq!(
        out.targets[0].qty, -1,
        "P05: default target_qty=1 → bearish short target=-1"
    );
}

// ---------------------------------------------------------------------------
// P06 — Short strategy bullish → flat/zero (long direction suppressed)
// ---------------------------------------------------------------------------

#[test]
fn p06_short_strategy_bullish_returns_flat() {
    let mut s = IntradayScalperStrategy::new_short("AAPL");
    let out = s.on_bar(&ctx(bullish_bars()));
    assert_eq!(
        out.targets[0].qty, 0,
        "P06: short-only strategy: bullish displacement → target=0 (long direction suppressed)"
    );
}

// ---------------------------------------------------------------------------
// P07 — Short strategy below-threshold → flat/zero
// ---------------------------------------------------------------------------

#[test]
fn p07_short_strategy_below_threshold_returns_flat() {
    let mut s = IntradayScalperStrategy::new_short("AAPL");
    let out = s.on_bar(&ctx(flat_bars()));
    assert_eq!(
        out.targets[0].qty, 0,
        "P07: short strategy below-threshold → target=0 (no signal)"
    );
}

// ---------------------------------------------------------------------------
// P08 — Long and short strategy spec names are distinct
// ---------------------------------------------------------------------------

#[test]
fn p08_long_and_short_strategy_names_are_distinct() {
    let long_s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
    let short_s = IntradayScalperStrategy::new_short("AAPL");
    assert_ne!(
        long_s.spec().name,
        short_s.spec().name,
        "P08: long and short strategy spec names must be distinct"
    );
    assert_eq!(
        long_s.spec().name,
        LONG_NAME,
        "P08: long name = \"intraday_scalper\""
    );
    assert_eq!(
        short_s.spec().name,
        SHORT_NAME,
        "P08: short name = \"intraday_short_scalper\""
    );
}

// ---------------------------------------------------------------------------
// P09 — Both identities register in PluginRegistry without conflict
// ---------------------------------------------------------------------------

#[test]
fn p09_both_strategies_register_in_plugin_registry() {
    use mqk_strategy::engines::register_builtin_strategies;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies(&mut reg, "AAPL").expect("P09: registration must succeed");

    assert!(
        reg.contains(LONG_NAME),
        "P09: registry must contain \"intraday_scalper\""
    );
    assert!(
        reg.contains(SHORT_NAME),
        "P09: registry must contain \"intraday_short_scalper\""
    );
}

// ---------------------------------------------------------------------------
// P10 — PluginRegistry can hold both; instantiate produces correct names
// ---------------------------------------------------------------------------

#[test]
fn p10_registry_instantiate_produces_correct_names() {
    use mqk_strategy::engines::register_builtin_strategies;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies(&mut reg, "AAPL").unwrap();

    let long_inst = reg.instantiate(LONG_NAME).unwrap();
    let short_inst = reg.instantiate(SHORT_NAME).unwrap();

    assert_eq!(
        long_inst.spec().name,
        LONG_NAME,
        "P10: instantiated long strategy has correct name"
    );
    assert_eq!(
        short_inst.spec().name,
        SHORT_NAME,
        "P10: instantiated short strategy has correct name"
    );
    assert_ne!(
        long_inst.spec().name,
        short_inst.spec().name,
        "P10: instantiated strategies are distinguishable by name"
    );
}

// ---------------------------------------------------------------------------
// P11 — StrategyHost enforces single-strategy (documents MULTI-STRATEGY-RUNTIME-DISPATCH-01)
// ---------------------------------------------------------------------------

/// Proves StrategyHost rejects a second strategy registration.
///
/// The runtime currently supports exactly one active strategy.  Both
/// `intraday_scalper` and `intraday_short_scalper` can be registered in a
/// `PluginRegistry` (discovery catalog), but only one can be loaded into
/// `StrategyHost` per session.  True concurrent long+short dispatch on the
/// same symbol requires a separate patch: `MULTI-STRATEGY-RUNTIME-DISPATCH-01`.
#[test]
fn p11_strategy_host_enforces_single_strategy() {
    use mqk_strategy::engines::register_builtin_strategies;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies(&mut reg, "AAPL").unwrap();

    let long_inst = reg.instantiate(LONG_NAME).unwrap();
    let short_inst = reg.instantiate(SHORT_NAME).unwrap();

    let mut host = StrategyHost::new(ShadowMode::Off);
    host.register(long_inst)
        .expect("P11: first strategy registers OK");

    let err = host.register(short_inst).unwrap_err();
    assert_eq!(
        err,
        StrategyHostError::MultiStrategyNotAllowed,
        "P11: StrategyHost blocks second strategy — MULTI-STRATEGY-RUNTIME-DISPATCH-01 required"
    );
}

// ---------------------------------------------------------------------------
// P12 — Short target magnitude follows sizing caps
// ---------------------------------------------------------------------------

#[test]
fn p12_short_target_magnitude_follows_sizing_caps() {
    // new_short uses env-based sizing; for this test use short_signals builder
    // on a with_caps instance to verify the magnitude capping path.
    let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(3), None).short_signals(true);
    // Note: this is NOT is_short_only — it can express both directions.
    // For the short-only variant the same cap logic applies on the negative path.
    let out = s.on_bar(&ctx(bearish_bars()));
    assert_eq!(
        out.targets[0].qty, -3,
        "P12: target_qty=10 capped to 3; bearish + short_signals → target=-3"
    );
}

// ---------------------------------------------------------------------------
// P13 — B5 logic blocks short-from-flat; strategy target is not forwarded
// ---------------------------------------------------------------------------

/// Mirrors the B5 guard classification in `decision.rs`.  Proves that even
/// when `new_short()` emits `target=-1`, the delta against a flat position
/// triggers the B5 short-from-flat block condition.
///
/// Full daemon B5 proof is in `scenario_native_strategy_b5_short_guard`.
#[test]
fn p13_b5_logic_blocks_short_from_flat_for_new_short_variant() {
    let mut s = IntradayScalperStrategy::new_short("AAPL");
    let out = s.on_bar(&ctx(bearish_bars()));

    assert_eq!(
        out.targets[0].qty, -1,
        "P13 precondition: new_short() emits target=-1 on bearish"
    );

    // Replicate decision.rs B5 guard: delta < 0 AND (current <= 0 OR abs(delta) > current).
    let current = 0i64; // flat position
    let delta = out.targets[0].qty - current; // -1 - 0 = -1
    let is_b5_blocked = delta < 0 && current <= 0;
    assert!(
        is_b5_blocked,
        "P13: delta={delta}, current={current} → B5 classifies ShortOpen and blocks; \
         target never reaches broker without explicit short-entry policy authorization"
    );
}
