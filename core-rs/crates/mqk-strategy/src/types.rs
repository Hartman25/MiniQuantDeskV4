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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarStub {
    pub end_ts: i64,
    pub is_complete: bool,
    pub close_micros: i64,
    pub volume: i64,
}

impl BarStub {
    pub fn new(end_ts: i64, is_complete: bool, close_micros: i64, volume: i64) -> Self {
        Self {
            end_ts,
            is_complete,
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
    pub intents: StrategyIntents,
}
