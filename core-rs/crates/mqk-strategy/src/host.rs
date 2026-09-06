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
        self.spec
            .clone()
            .ok_or(StrategyHostError::NoStrategyRegistered)
    }

    /// S1: the currently-registered strategy's `semantic_fingerprint()`.
    /// Queried from the exact same boxed instance `on_bar` dispatches to —
    /// never reconstructed from `spec` or any other cached value.
    pub fn semantic_fingerprint(&self) -> Result<String, StrategyHostError> {
        self.strategy
            .as_ref()
            .map(|s| s.semantic_fingerprint())
            .ok_or(StrategyHostError::NoStrategyRegistered)
    }

    /// W06-REPLAY-NO-DECISION-SEMANTICS-01 (Patch A): the registered
    /// strategy's `Strategy::empty_output_is_noop()` declaration. `false`
    /// (the universal safe default) if no strategy is registered — callers
    /// that care have already errored out of `on_bar`/`spec` before this
    /// could matter.
    pub fn empty_output_is_noop(&self) -> bool {
        self.strategy
            .as_ref()
            .map(|s| s.empty_output_is_noop())
            .unwrap_or(false)
    }

    /// Run one bar evaluation. Validates timeframe and returns LIVE/SHADOW intents.
    pub fn on_bar(
        &mut self,
        ctx: &StrategyContext,
    ) -> Result<StrategyBarResult, StrategyHostError> {
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

        // S1 hardening: captured BEFORE on_bar runs, so this fingerprint
        // unambiguously describes the semantic state that entered the
        // decision, not whatever the instance's own mutable state happens to
        // read back as immediately afterward. Built-in engines are
        // config-static during on_bar today (the fingerprint cannot change
        // as a result of evaluating this one bar), so this is a hardening
        // of intent, not a behavior change for any current engine.
        let semantic_fingerprint = s.semantic_fingerprint();
        let output = s.on_bar(ctx);

        let mode = match self.shadow {
            ShadowMode::Off => IntentMode::Live,
            ShadowMode::On => IntentMode::Shadow,
        };

        Ok(StrategyBarResult {
            spec,
            semantic_fingerprint,
            intents: StrategyIntents { mode, output },
        })
    }
}
