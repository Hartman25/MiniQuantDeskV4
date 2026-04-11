//! B1B: Native strategy input bridge — proof tests.
//!
//! All tests are pure in-process; no DB or network required.
//!
//! # What is proved
//!
//! | ID     | Condition                               | Expected                          |
//! |--------|-----------------------------------------|-----------------------------------|
//! | B1B-01 | Active bootstrap + limit signal         | callback invoked (Some returned)  |
//! | B1B-02 | Active bootstrap → result is Shadow     | IntentMode::Shadow preserved      |
//! | B1B-03 | Dormant bootstrap                       | no callback (None)                |
//! | B1B-04 | Failed bootstrap                        | no callback (None)                |
//! | B1B-05 | Market order (no limit_price)           | callback invoked; bar incomplete  |
//! | B1B-06 | build_signal_context pure-fn correctness| context fields match inputs       |
//!
//! B1B-03 and B1B-04 prove fail-closed behavior: no active bootstrap → no
//! callback is ever made, regardless of the signal payload.
//!
//! B1B-05 proves that market orders do not silently suppress the callback —
//! the strategy receives an incomplete bar and returns empty targets, which is
//! the correct conservative behavior from the strategy engine, not our gate.

use mqk_runtime::native_strategy::{build_signal_context, NativeStrategyBootstrap};
use mqk_strategy::{
    IntentMode, PluginRegistry, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec,
};

// ---------------------------------------------------------------------------
// Minimal inline stub strategy — not a production strategy.
// ---------------------------------------------------------------------------

struct StubStrategy {
    name: &'static str,
    timeframe_secs: i64,
}

impl mqk_strategy::Strategy for StubStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.name, self.timeframe_secs)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput { targets: vec![] }
    }
}

fn stub_registry(name: &'static str, tf: i64) -> PluginRegistry {
    let mut reg = PluginRegistry::new();
    reg.register(
        StrategyMeta::new(name, "1.0.0", tf, "B1B test stub"),
        move || {
            Box::new(StubStrategy {
                name,
                timeframe_secs: tf,
            }) as Box<dyn mqk_strategy::Strategy>
        },
    )
    .expect("stub_registry: register must succeed");
    reg
}

fn active_bootstrap(name: &'static str, tf: i64) -> NativeStrategyBootstrap {
    let reg = stub_registry(name, tf);
    let ids = vec![name.to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

// ---------------------------------------------------------------------------
// B1B-01: Active bootstrap + limit signal → callback invoked
// ---------------------------------------------------------------------------

/// B1B-01: Active bootstrap + limit-order signal → on_bar callback invoked.
///
/// Proves: the primary happy path — active bootstrap, price-bearing signal,
/// callback returns Some. This is the core B1B wiring proof.
#[test]
fn b1b_01_active_bootstrap_limit_signal_invokes_callback() {
    let mut b = active_bootstrap("stub", 300);
    assert!(b.is_active(), "precondition: bootstrap must be active");

    let result = b.invoke_on_bar_from_signal(
        1,                 // now_tick
        1_700_000_000,     // end_ts
        Some(150_000_000), // limit_price in micros
        10,                // qty
    );

    assert!(
        result.is_some(),
        "B1B-01: active bootstrap + limit signal must invoke callback and return Some"
    );
}

// ---------------------------------------------------------------------------
// B1B-02: Shadow mode preserved in bar result
// ---------------------------------------------------------------------------

/// B1B-02: Bar result carries IntentMode::Live after B1C.
///
/// B1C lifted the shadow-mode constraint (ShadowMode::Off).  The host now
/// produces Live intents so the decision submission bridge in the execution
/// loop can forward them to the canonical admission seam.
#[test]
fn b1b_02_callback_result_is_live_mode() {
    let mut b = active_bootstrap("shadow_stub", 300);

    let result = b
        .invoke_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5)
        .expect("B1B-02: active bootstrap must return Some");

    assert_eq!(
        result.intents.mode,
        IntentMode::Live,
        "B1B-02: B1C lifted shadow mode; result must carry Live intents"
    );
}

// ---------------------------------------------------------------------------
// B1B-03: Dormant bootstrap → no callback
// ---------------------------------------------------------------------------

/// B1B-03: Dormant bootstrap → invoke_on_bar_from_signal returns None.
///
/// Proves fail-closed: operators who have not configured MQK_STRATEGY_IDS get
/// a Dormant bootstrap.  No callback is made; the signal route proceeds to
/// Gate 7 unchanged.
#[test]
fn b1b_03_dormant_bootstrap_no_callback() {
    let reg = PluginRegistry::new();
    let mut b = NativeStrategyBootstrap::bootstrap(None, &reg);
    assert!(b.is_dormant(), "precondition: bootstrap must be dormant");

    let result = b.invoke_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5);

    assert!(
        result.is_none(),
        "B1B-03: dormant bootstrap must not invoke callback (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// B1B-04: Failed bootstrap → no callback
// ---------------------------------------------------------------------------

/// B1B-04: Failed bootstrap → invoke_on_bar_from_signal returns None.
///
/// Proves fail-closed: a fleet with an unregistered strategy produces a Failed
/// bootstrap.  Even with a valid signal payload, no callback is made.
#[test]
fn b1b_04_failed_bootstrap_no_callback() {
    let reg = PluginRegistry::new(); // empty — fleet entry will not resolve
    let ids = vec!["missing_strategy".to_string()];
    let mut b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_failed(), "precondition: bootstrap must be failed");

    let result = b.invoke_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5);

    assert!(
        result.is_none(),
        "B1B-04: failed bootstrap must not invoke callback (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// B1B-05: Market order (no limit_price) → callback still invoked
// ---------------------------------------------------------------------------

/// B1B-05: Market order (limit_price=None) → callback is still invoked.
///
/// Proves: market orders do not silently suppress the callback.  A bar with
/// `is_complete=false` and `close_micros=0` is constructed and passed to the
/// strategy.  The strategy returns empty targets (correct: no price reference),
/// but the callback IS made and the result is Some.
///
/// The suppression decision belongs to the strategy engine, not the input
/// bridge.  The bridge passes all signals through honestly.
#[test]
fn b1b_05_market_order_callback_invoked_with_incomplete_bar() {
    let mut b = active_bootstrap("stub", 300);

    // No limit_price → market order → bar will be incomplete
    let result = b.invoke_on_bar_from_signal(1, 1_700_000_000, None, 10);

    assert!(
        result.is_some(),
        "B1B-05: market order must still invoke callback; strategy decides on incomplete bar"
    );
    // B1C: shadow mode lifted; Live intents produced.
    assert_eq!(result.unwrap().intents.mode, IntentMode::Live);
}

// ---------------------------------------------------------------------------
// B1B-06: build_signal_context pure-function correctness
// ---------------------------------------------------------------------------

/// B1B-06: build_signal_context produces correct StrategyContext fields.
///
/// Proves: the pure context-builder is honest about every field.
/// - Limit order → is_complete=true, close_micros=limit_price.
/// - Market order → is_complete=false, close_micros=0.
/// - Window length is always 1 (single-bar, no fabricated history).
#[test]
fn b1b_06_build_signal_context_pure_fn_correctness() {
    // Limit order case.
    let ctx = build_signal_context(
        300,               // timeframe_secs
        42,                // now_tick
        1_700_000_000,     // end_ts
        Some(150_000_000), // limit_price
        7,                 // qty
    );

    assert_eq!(ctx.timeframe_secs, 300);
    assert_eq!(ctx.now_tick, 42);
    assert_eq!(
        ctx.recent.len(),
        1,
        "single-bar window: no fabricated history"
    );

    let bar = ctx.recent.last().expect("window must have one bar");
    assert_eq!(bar.end_ts, 1_700_000_000);
    assert!(bar.is_complete, "limit order bar must be complete");
    assert_eq!(bar.close_micros, 150_000_000);
    assert_eq!(bar.volume, 7);

    // Market order case.
    let ctx2 = build_signal_context(300, 1, 1_700_000_000, None, 5);
    let bar2 = ctx2.recent.last().expect("window must have one bar");
    assert!(!bar2.is_complete, "market order bar must be incomplete");
    assert_eq!(bar2.close_micros, 0, "no price reference → close_micros=0");
    assert_eq!(bar2.volume, 5);
}
