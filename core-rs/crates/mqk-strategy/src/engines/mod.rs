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
