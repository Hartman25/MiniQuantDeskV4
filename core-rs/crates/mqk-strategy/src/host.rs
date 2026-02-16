use crate::{
    IntentMode, ShadowMode, Strategy, StrategyBarResult, StrategyContext, StrategyHostError,
    StrategyIntents, StrategySpec,
};

/// StrategyHost enforces Tier A rules:
/// - exactly one strategy
/// - single timeframe match
/// - shadow mode gating (returns SHADOW intents)
pub struct StrategyHost {
    strategy: Option<Box<dyn Strategy>>,
    spec: Option<StrategySpec>,
    shadow: ShadowMode,
}

impl StrategyHost {
    pub fn new(shadow: ShadowMode) -> Self {
        Self {
            strategy: None,
            spec: None,
            shadow,
        }
    }

    pub fn shadow_mode(&self) -> ShadowMode {
        self.shadow
    }

    pub fn set_shadow_mode(&mut self, shadow: ShadowMode) {
        self.shadow = shadow;
    }

    /// Register a strategy. Tier A: only one.
    pub fn register(&mut self, s: Box<dyn Strategy>) -> Result<(), StrategyHostError> {
        if self.strategy.is_some() {
            return Err(StrategyHostError::MultiStrategyNotAllowed);
        }
        let spec = s.spec();
        self.spec = Some(spec);
        self.strategy = Some(s);
        Ok(())
    }

    pub fn spec(&self) -> Result<StrategySpec, StrategyHostError> {
        self.spec.clone().ok_or(StrategyHostError::NoStrategyRegistered)
    }

    /// Run one bar evaluation. Validates timeframe and returns LIVE/SHADOW intents.
    pub fn on_bar(&mut self, ctx: &StrategyContext) -> Result<StrategyBarResult, StrategyHostError> {
        let spec = self.spec()?;

        if ctx.timeframe_secs != spec.timeframe_secs {
            return Err(StrategyHostError::TimeframeMismatch {
                expected_secs: spec.timeframe_secs,
                got_secs: ctx.timeframe_secs,
            });
        }

        let s = self
            .strategy
            .as_mut()
            .ok_or(StrategyHostError::NoStrategyRegistered)?;

        let output = s.on_bar(ctx);

        let mode = match self.shadow {
            ShadowMode::Off => IntentMode::Live,
            ShadowMode::On => IntentMode::Shadow,
        };

        Ok(StrategyBarResult {
            spec,
            intents: StrategyIntents { mode, output },
        })
    }
}
