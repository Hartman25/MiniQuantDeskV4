use crate::{PluginRegistry, RegistryError, Strategy};

pub mod intraday_scalper;
pub mod mean_reversion;
pub mod swing_momentum;
pub mod volatility_breakout;

pub use intraday_scalper::{
    compute_diagnostics as intraday_scalper_compute_diagnostics, IntradayScalperDiagnostics,
    IntradayScalperStrategy,
};
pub use mean_reversion::MeanReversionStrategy;
pub use swing_momentum::SwingMomentumStrategy;
pub use volatility_breakout::VolatilityBreakoutStrategy;

/// IR9: the single authoritative list of every strategy identity
/// [`register_builtin_strategies`] registers, in registration order. Four
/// engine *implementations* (`swing_momentum`, `mean_reversion`,
/// `volatility_breakout`, `intraday_scalper`) back five registered strategy
/// *identities* — `intraday_scalper`'s short-only variant
/// (`intraday_short_scalper`) is a distinct registered identity sharing the
/// same engine implementation. Any bound or guard elsewhere in the
/// workspace that needs to know the size or membership of the built-in
/// strategy universe (e.g. `mqk_portfolio::MAX_STRATEGY_UNIVERSE`) must
/// consume or cross-check against this list, not a hand-maintained count —
/// see `registered_strategy_ids_match_this_constant` below and the
/// cross-crate bound test in `mqk-daemon`. Adding a new registration to
/// [`register_builtin_strategies`] without updating this constant fails
/// that structural test.
pub const REGISTERED_STRATEGY_IDS: &[&str] = &[
    swing_momentum::NAME,
    mean_reversion::NAME,
    volatility_breakout::NAME,
    intraday_scalper::NAME,
    intraday_scalper::SHORT_NAME,
];

/// Register the built-in deterministic strategy engines.
///
/// Tier A intent:
/// - registry/discovery only
/// - no runtime wiring here
/// - no IO
/// - deterministic factories
pub fn register_builtin_strategies(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
) -> Result<(), RegistryError> {
    let symbol = symbol.into();

    let swing_symbol = symbol.clone();
    registry.register(swing_momentum::meta(), move || {
        Box::new(SwingMomentumStrategy::new(swing_symbol.clone())) as Box<dyn Strategy>
    })?;

    let mr_symbol = symbol.clone();
    registry.register(mean_reversion::meta(), move || {
        Box::new(MeanReversionStrategy::new(mr_symbol.clone())) as Box<dyn Strategy>
    })?;

    let vb_symbol = symbol.clone();
    registry.register(volatility_breakout::meta(), move || {
        Box::new(VolatilityBreakoutStrategy::new(vb_symbol.clone())) as Box<dyn Strategy>
    })?;

    let scalp_symbol = symbol.clone();
    registry.register(intraday_scalper::meta(), move || {
        Box::new(IntradayScalperStrategy::new(scalp_symbol.clone())) as Box<dyn Strategy>
    })?;

    // SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01: register the short-only variant
    // as a distinct strategy identity.  Disabled by default in
    // `sys_strategy_registry`; runtime selects at most one strategy via
    // `MQK_STRATEGY_IDS`.  True concurrent long+short dispatch requires
    // MULTI-STRATEGY-RUNTIME-DISPATCH-01.
    let short_scalp_symbol = symbol;
    registry.register(intraday_scalper::meta_short(), move || {
        Box::new(IntradayScalperStrategy::new_short(
            short_scalp_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    Ok(())
}

/// Register built-in strategies with explicit sizing parameters for deterministic backtests.
///
/// BACKTEST-CONFIG-DETERMINISM-SIZING-01: backtest callers must use this variant
/// so the strategy is constructed from `BacktestConfig.sizing` rather than ambient
/// env vars. This ensures `config_id` and strategy behavior are consistent.
///
/// For live/paper runtime use, continue calling `register_builtin_strategies` which
/// reads from env vars at strategy construction time (correct live behavior).
pub fn register_builtin_strategies_with_sizing(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_notional_usd: Option<i64>,
) -> Result<(), RegistryError> {
    let symbol = symbol.into();

    // Non-scalper strategies do not yet have configurable sizing; they are
    // registered with their default env-reading constructors. When they gain
    // explicit sizing support, add parameters here.
    let swing_symbol = symbol.clone();
    registry.register(swing_momentum::meta(), move || {
        Box::new(SwingMomentumStrategy::new(swing_symbol.clone())) as Box<dyn Strategy>
    })?;

    let mr_symbol = symbol.clone();
    registry.register(mean_reversion::meta(), move || {
        Box::new(MeanReversionStrategy::new(mr_symbol.clone())) as Box<dyn Strategy>
    })?;

    let vb_symbol = symbol.clone();
    registry.register(volatility_breakout::meta(), move || {
        Box::new(VolatilityBreakoutStrategy::new(vb_symbol.clone())) as Box<dyn Strategy>
    })?;

    let scalp_symbol = symbol.clone();
    registry.register(intraday_scalper::meta(), move || {
        Box::new(IntradayScalperStrategy::with_caps(
            scalp_symbol.clone(),
            target_qty,
            max_target_qty,
            max_notional_usd,
        )) as Box<dyn Strategy>
    })?;

    // SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01: short-only variant registered
    // with the same env-based sizing as new_short().  Disabled by default.
    let short_scalp_symbol = symbol;
    registry.register(intraday_scalper::meta_short(), move || {
        Box::new(IntradayScalperStrategy::new_short(
            short_scalp_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    Ok(())
}

#[cfg(test)]
mod registered_strategy_ids_tests {
    use super::*;

    /// IR9: [`REGISTERED_STRATEGY_IDS`] must track [`register_builtin_strategies`]
    /// exactly — same identities, same count, same order. A new registration
    /// added to that function without updating the constant fails this test,
    /// which is the whole point: the bound can never silently drift out from
    /// under the strategies actually registered.
    #[test]
    fn registered_strategy_ids_match_register_builtin_strategies_exactly() {
        let mut registry = PluginRegistry::new();
        register_builtin_strategies(&mut registry, "TEST_SYMBOL".to_string())
            .expect("register_builtin_strategies must succeed for a fresh registry");

        let actual: Vec<&str> = registry.list().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            actual, REGISTERED_STRATEGY_IDS,
            "REGISTERED_STRATEGY_IDS has drifted from register_builtin_strategies' actual \
             registrations -- update the constant to match"
        );
    }

    #[test]
    fn registered_strategy_ids_has_five_distinct_entries() {
        let mut unique = REGISTERED_STRATEGY_IDS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            REGISTERED_STRATEGY_IDS.len(),
            "every registered strategy identity must be distinct"
        );
        assert_eq!(REGISTERED_STRATEGY_IDS.len(), 5);
    }

    #[test]
    fn short_scalper_identity_is_present_and_distinct_from_long() {
        assert!(REGISTERED_STRATEGY_IDS.contains(&"intraday_scalper"));
        assert!(REGISTERED_STRATEGY_IDS.contains(&"intraday_short_scalper"));
    }
}

// ---------------------------------------------------------------------------
// STRATEGY-SEMANTIC-IDENTITY-SEAM-01 (S1): registry/host-wide identity checks.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod semantic_identity_tests {
    use super::*;

    /// Every built-in engine explicitly overrides `semantic_fingerprint` —
    /// none of the five registered identities silently falls back to the
    /// trait's spec-only default (which would carry forward exactly the
    /// defect S1 exists to fix). Detected by comparing against two
    /// deliberately mismatched-config instances of the SAME registered
    /// name/timeframe: the trait default cannot see the mismatch (it only
    /// sees `spec()`), so if any built-in were still using the default, the
    /// pair below would incorrectly fingerprint identically for at least one
    /// identity that has decision-affecting config beyond name/timeframe
    /// (intraday_scalper / intraday_short_scalper here, since the other three
    /// currently have no per-instance runtime-configurable field beyond
    /// symbol, which this test also covers via a symbol change).
    #[test]
    fn every_registered_builtin_overrides_the_default_fingerprint() {
        for symbol in ["AAPL", "MSFT"] {
            let registry = build_registry_for(symbol);
            for name in REGISTERED_STRATEGY_IDS {
                let instance = registry.instantiate(name).unwrap();
                let other_symbol = if symbol == "AAPL" { "MSFT" } else { "AAPL" };
                let other_registry = build_registry_for(other_symbol);
                let other_instance = other_registry.instantiate(name).unwrap();
                assert_ne!(
                    instance.semantic_fingerprint(),
                    other_instance.semantic_fingerprint(),
                    "identity '{name}': fingerprint did not change when symbol changed \
                     ({symbol} -> {other_symbol}) — suspect it is still using the \
                     spec-only trait default"
                );
            }
        }
        // intraday_scalper/intraday_short_scalper additionally vary target_qty.
        let default_caps = build_registry_for("AAPL");
        let custom_caps = {
            let mut r = PluginRegistry::new();
            register_builtin_strategies_with_sizing(&mut r, "AAPL", 9, Some(2), Some(3000))
                .unwrap();
            r
        };
        for name in ["intraday_scalper"] {
            let a = default_caps.instantiate(name).unwrap();
            let b = custom_caps.instantiate(name).unwrap();
            assert_ne!(
                a.semantic_fingerprint(),
                b.semantic_fingerprint(),
                "identity '{name}': sizing change did not change fingerprint"
            );
        }
    }

    fn build_registry_for(symbol: &str) -> PluginRegistry {
        let mut r = PluginRegistry::new();
        register_builtin_strategies(&mut r, symbol.to_string()).unwrap();
        r
    }

    /// Fixed built-in descriptors (no per-instance runtime config beyond
    /// symbol) are fully deterministic across repeated registry
    /// construction: same symbol in -> same fingerprint out, every time.
    #[test]
    fn fixed_builtin_descriptors_are_deterministic_across_registrations() {
        for name in REGISTERED_STRATEGY_IDS {
            let r1 = build_registry_for("AAPL");
            let r2 = build_registry_for("AAPL");
            let fp1 = r1.instantiate(name).unwrap().semantic_fingerprint();
            let fp2 = r2.instantiate(name).unwrap().semantic_fingerprint();
            assert_eq!(fp1, fp2, "identity '{name}' is not deterministic");
        }
    }

    /// `PluginRegistry::instantiate` produces an instance whose
    /// `semantic_fingerprint()` is exactly what `StrategyHost` observes
    /// after registration — the host neither loses nor reconstructs it.
    #[test]
    fn registry_instance_fingerprint_matches_what_host_observes_after_registration() {
        use crate::{ShadowMode, StrategyHost};

        let registry = build_registry_for("AAPL");
        for name in REGISTERED_STRATEGY_IDS {
            let instance = registry.instantiate(name).unwrap();
            let expected = instance.semantic_fingerprint();

            let mut host = StrategyHost::new(ShadowMode::Off);
            host.register(instance).unwrap();

            assert_eq!(
                host.semantic_fingerprint().unwrap(),
                expected,
                "identity '{name}': host-observed fingerprint diverged from the \
                 registry-instantiated instance's own fingerprint"
            );
        }
    }
}
