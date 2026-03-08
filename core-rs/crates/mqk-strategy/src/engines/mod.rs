use crate::{PluginRegistry, RegistryError, Strategy};

pub mod intraday_scalper;
pub mod mean_reversion;
pub mod swing_momentum;
pub mod volatility_breakout;

pub use intraday_scalper::IntradayScalperStrategy;
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

    let scalp_symbol = symbol;
    registry.register(intraday_scalper::meta(), move || {
        Box::new(IntradayScalperStrategy::new(scalp_symbol.clone())) as Box<dyn Strategy>
    })?;

    Ok(())
}
