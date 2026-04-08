//! B1A: Native strategy runtime bootstrap — proof tests.
//!
//! All tests are pure in-process; no DB or network required.
//!
//! Proves:
//! 1. No canonical strategy truth (fleet absent/empty) → Dormant (not an error, not a fake).
//! 2. Canonical single strategy truth + registry match → bootstrap Active.
//! 3. Fleet present but registry miss → Failed (no fake fallback created).

use mqk_runtime::native_strategy::{NativeStrategyBootstrap, NativeStrategyBootstrapOutcome};
use mqk_strategy::{PluginRegistry, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec};

// ---------------------------------------------------------------------------
// Minimal inline stub strategy — not a production strategy.
// Used only to populate the registry for bootstrap proof tests.
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
        StrategyMeta::new(name, "1.0.0", tf, "test stub"),
        move || Box::new(StubStrategy { name, timeframe_secs: tf }) as Box<dyn mqk_strategy::Strategy>,
    )
    .expect("stub_registry: register must succeed");
    reg
}

// ---------------------------------------------------------------------------
// 1. No canonical strategy truth → Dormant
// ---------------------------------------------------------------------------

/// B1A-01: fleet_ids=None → Dormant (no strategy fleet configured; not an error).
#[test]
fn b1a_01_none_fleet_is_dormant() {
    let reg = PluginRegistry::new();
    let b = NativeStrategyBootstrap::bootstrap(None, &reg);
    assert!(b.is_dormant(), "expected dormant when fleet is None");
    assert!(!b.is_active(), "must not be active");
    assert!(!b.is_failed(), "must not be failed");
    assert_eq!(b.truth_state(), "dormant");
    assert!(b.active_strategy_id().is_none());
    assert!(b.failure_reason().is_none());
}

/// B1A-02: fleet_ids=Some([]) → Dormant (empty fleet is equivalent to absent).
#[test]
fn b1a_02_empty_fleet_is_dormant() {
    let reg = PluginRegistry::new();
    let ids: Vec<String> = vec![];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_dormant(), "expected dormant when fleet is empty");
    assert!(!b.is_active());
    assert!(!b.is_failed());
}

// ---------------------------------------------------------------------------
// 2. Canonical single strategy truth present → bootstrap Active
// ---------------------------------------------------------------------------

/// B1A-03: fleet with matching registry entry → Active; strategy_id correct.
#[test]
fn b1a_03_canonical_fleet_with_registry_match_is_active() {
    let reg = stub_registry("my_strat", 60);
    let ids = vec!["my_strat".to_string()];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_active(), "expected Active when fleet + registry match");
    assert!(!b.is_dormant());
    assert!(!b.is_failed());
    assert_eq!(b.active_strategy_id(), Some("my_strat"));
    assert_eq!(b.truth_state(), "active");
    assert!(b.failure_reason().is_none());
}

// ---------------------------------------------------------------------------
// 3. No fake fallback strategy created
// ---------------------------------------------------------------------------

/// B1A-04: fleet present + empty registry → Failed (not Dormant, not Active with a fake).
///
/// Proves that when an operator configures MQK_STRATEGY_IDS but the registry
/// is empty (no strategy wired), the outcome is Failed — not silently Dormant
/// or Active with a synthetic fallback.
#[test]
fn b1a_04_fleet_with_empty_registry_fails_closed() {
    let reg = PluginRegistry::new();
    let ids = vec!["some_strategy".to_string()];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_failed(), "expected Failed: fleet configured but registry empty");
    assert!(!b.is_dormant(), "must NOT be dormant — fleet was configured");
    assert!(!b.is_active(), "must NOT be active — no real strategy found in registry");
    assert_eq!(b.truth_state(), "failed");
    assert!(b.failure_reason().is_some(), "failure_reason must be populated");
}

/// B1A-05: fleet names a strategy not in registry (other entries exist) → Failed.
#[test]
fn b1a_05_fleet_with_wrong_registry_entry_fails_closed() {
    let reg = stub_registry("other_strat", 60);
    let ids = vec!["desired_strat".to_string()];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_failed(), "expected Failed: strategy ID not found in registry");
    assert!(!b.is_active(), "must not produce an active host for the wrong strategy");
    let reason = b.failure_reason().expect("failure_reason must be present");
    assert!(
        reason.contains("desired_strat"),
        "failure reason must name the missing strategy; got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Additional structural proofs
// ---------------------------------------------------------------------------

/// B1A-06: single-strategy Tier A policy — only the first fleet entry is consumed.
#[test]
fn b1a_06_only_first_fleet_entry_consumed() {
    let reg = stub_registry("first", 60);
    let ids = vec!["first".to_string(), "second".to_string()];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);
    assert!(b.is_active(), "expected Active using first fleet entry");
    assert_eq!(b.active_strategy_id(), Some("first"));
}

/// B1A-07: Active host is initialised in shadow mode (bar ingestion not yet wired).
#[test]
fn b1a_07_active_host_is_in_shadow_mode() {
    let reg = stub_registry("shad_strat", 300);
    let ids = vec!["shad_strat".to_string()];
    let b = NativeStrategyBootstrap::bootstrap(Some(&ids), &reg);

    match &b.outcome {
        NativeStrategyBootstrapOutcome::Active { host, strategy_id } => {
            assert_eq!(strategy_id, "shad_strat");
            assert_eq!(
                host.shadow_mode(),
                mqk_strategy::ShadowMode::On,
                "B1A: host must start in shadow mode until bar ingestion is wired"
            );
        }
        _ => panic!("expected Active outcome; got {}", b.truth_state()),
    }
}
