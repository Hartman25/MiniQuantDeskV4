use mqk_execution::StrategyOutput;

/// Strategy identity + Tier A constraints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategySpec {
    pub name: String,
    /// Tier A: exactly one timeframe for the strategy.
    pub timeframe_secs: i64,
}

impl StrategySpec {
    pub fn new(name: impl Into<String>, timeframe_secs: i64) -> Self {
        debug_assert!(timeframe_secs > 0);
        Self {
            name: name.into(),
            timeframe_secs,
        }
    }
}

/// A minimal, deterministic bar stub for context.
/// (No broker/DB access. Real bar schema can be unified later with mqk-integrity.)
///
/// OHLCV fields: `open_micros`, `high_micros`, `low_micros`, `close_micros`, `volume`.
/// Live bar loaders that only have close+volume use `BarStub::new`, which sets
/// open=high=low=close (conservative documented fallback). Backtest loaders use
/// `BarStub::with_ohlcv` to carry the full OHLCV from the bar source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarStub {
    pub end_ts: i64,
    pub is_complete: bool,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
}

impl BarStub {
    /// Backward-compatible constructor for live/paper loaders that only carry close + volume.
    ///
    /// Sets `open_micros = high_micros = low_micros = close_micros`. This is the correct
    /// conservative fallback when the source does not provide OHLC separately.
    pub fn new(end_ts: i64, is_complete: bool, close_micros: i64, volume: i64) -> Self {
        Self {
            end_ts,
            is_complete,
            open_micros: close_micros,
            high_micros: close_micros,
            low_micros: close_micros,
            close_micros,
            volume,
        }
    }

    /// Full OHLCV constructor. Use when the bar source provides open/high/low explicitly.
    ///
    /// Backtest engine uses this so strategies can access the full bar spread.
    pub fn with_ohlcv(
        end_ts: i64,
        is_complete: bool,
        open_micros: i64,
        high_micros: i64,
        low_micros: i64,
        close_micros: i64,
        volume: i64,
    ) -> Self {
        Self {
            end_ts,
            is_complete,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
        }
    }
}

/// Bounded recent-bars window (deterministic truncation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentBarsWindow {
    pub max_len: usize,
    pub bars: Vec<BarStub>,
}

impl RecentBarsWindow {
    /// Creates a bounded window by keeping the **most recent** bars (tail).
    pub fn new(max_len: usize, mut bars: Vec<BarStub>) -> Self {
        debug_assert!(max_len > 0);
        if bars.len() > max_len {
            let start = bars.len() - max_len;
            bars = bars.split_off(start);
        }
        Self { max_len, bars }
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn last(&self) -> Option<&BarStub> {
        self.bars.last()
    }
}

/// Context passed to strategies.
/// Intentionally minimal: deterministic inputs only; no IO handles; no broker/DB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyContext {
    pub timeframe_secs: i64,
    /// Deterministic tick/counter from runtime (not wall-clock).
    pub now_tick: u64,
    /// Bounded recent bars window (tail).
    pub recent: RecentBarsWindow,
}

impl StrategyContext {
    pub fn new(timeframe_secs: i64, now_tick: u64, recent: RecentBarsWindow) -> Self {
        debug_assert!(timeframe_secs > 0);
        Self {
            timeframe_secs,
            now_tick,
            recent,
        }
    }
}

/// Strategy trait: Tier A uses on_bar only.
/// Optional hooks (on_fill/on_timer) are explicitly deferred to later patches.
pub trait Strategy: Send + Sync {
    fn spec(&self) -> StrategySpec;

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput;

    /// STRATEGY-SEMANTIC-IDENTITY-SEAM-01 (S1): deterministic, hex-encoded
    /// SHA-256 fingerprint over this instance's effective, decision-affecting
    /// semantic configuration (see `crate::semantic_identity` for the
    /// determinism contract). `spec()` alone (name + timeframe_secs) is not
    /// sufficient — two instances can share a spec while differing in
    /// sizing, thresholds, or long/short behavior.
    ///
    /// Default: derived solely from `spec()`. This default exists only so
    /// dummy/harness `Strategy` implementations used across backtest and
    /// daemon test fixtures do not all need updating for S1 — it carries
    /// forward the exact pre-S1 collapse (two differently-configured
    /// instances of the same registered name/timeframe are indistinguishable)
    /// and must NOT be relied on by any strategy with decision-affecting
    /// configuration beyond name/timeframe. Every built-in engine in
    /// `crate::engines` overrides this explicitly.
    fn semantic_fingerprint(&self) -> String {
        let spec = self.spec();
        crate::semantic_identity::SemanticIdentityBuilder::new(
            crate::semantic_identity::SEMANTIC_IDENTITY_SCHEMA_V1,
            "strategy-default-spec-only",
            "v1",
        )
        .push_str(&spec.name)
        .push_i64(spec.timeframe_secs)
        .finish()
    }

    /// W06-REPLAY-NO-DECISION-SEMANTICS-01 (Patch A): whether an empty
    /// `StrategyOutput` from this strategy means "no new decision this bar —
    /// carry existing positions forward" rather than the ordinary
    /// complete-target-portfolio contract ("target: hold nothing"; see
    /// `mqk_execution::targets_to_order_intents`). Default `false` preserves
    /// the existing Paper/Live/backtest complete-target semantics for every
    /// strategy that does not explicitly opt in — only a strategy whose
    /// `on_bar` contract genuinely emits "not yet decided" on some calls
    /// (e.g. a replay strategy driven by an external clock) should override
    /// this to `true`.
    fn empty_output_is_noop(&self) -> bool {
        false
    }
}

/// Host-level policy errors (Tier A).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrategyHostError {
    MultiStrategyNotAllowed,
    TimeframeMismatch { expected_secs: i64, got_secs: i64 },
    NoStrategyRegistered,
}

/// Shadow mode config.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShadowMode {
    Off,
    On,
}

/// Intent mode label (doc-aligned).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IntentMode {
    Live,
    Shadow,
}

/// Output of running a strategy under the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyIntents {
    pub mode: IntentMode,
    pub output: StrategyOutput,
}

impl StrategyIntents {
    pub fn should_execute(&self) -> bool {
        self.mode == IntentMode::Live
    }
}

/// Result of a strategy bar evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyBarResult {
    pub spec: StrategySpec,
    /// S1: the exact host instance's `semantic_fingerprint()` at the moment
    /// it produced `intents` — propagated here so a downstream promotion
    /// gate can bind a decision to the precise semantic configuration that
    /// produced it, without re-deriving it from mutable ambient state.
    pub semantic_fingerprint: String,
    pub intents: StrategyIntents,
}
