use std::collections::{BTreeMap, BTreeSet};

/// Identifies a feed source (deterministic ordering for tests/logs).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeedId(pub String);

impl FeedId {
    pub fn new<S: Into<String>>(s: S) -> Self {
        Self(s.into())
    }
}

/// Timeframe is expressed as a bar interval in seconds.
/// PATCH 08 stays minimal: caller supplies interval_secs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timeframe {
    pub interval_secs: i64,
}

impl Timeframe {
    pub fn secs(interval_secs: i64) -> Self {
        debug_assert!(interval_secs > 0);
        Self { interval_secs }
    }
}

/// A deterministic bar identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BarKey {
    pub symbol: String,
    pub tf: Timeframe,
    /// Bar end time in epoch seconds (deterministic; provided by runtime/data adapter).
    pub end_ts: i64,
}

impl BarKey {
    pub fn new<S: Into<String>>(symbol: S, tf: Timeframe, end_ts: i64) -> Self {
        Self {
            symbol: symbol.into(),
            tf,
            end_ts,
        }
    }
}

/// Minimal bar payload (enough to validate lookahead + disagreement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub key: BarKey,
    /// If false, this bar is not closed/complete (anti-lookahead must reject).
    pub is_complete: bool,
    /// Fingerprint fields used for feed-disagreement checks.
    pub close_micros: i64,
    pub volume: i64,
}

impl Bar {
    pub fn new(key: BarKey, is_complete: bool, close_micros: i64, volume: i64) -> Self {
        Self {
            key,
            is_complete,
            close_micros,
            volume,
        }
    }
}

/// Policy config for integrity checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityConfig {
    /// Number of missing bars tolerated before failing (0 = fail on any gap).
    pub gap_tolerance_bars: u32,

    /// If now_tick - last_feed_tick > stale_threshold_ticks => DISARM.
    pub stale_threshold_ticks: u64,

    /// If true, require feeds to agree on fingerprint for same BarKey (when both seen).
    pub enforce_feed_disagreement: bool,
}

impl IntegrityConfig {
    pub fn strict_defaults() -> Self {
        Self {
            gap_tolerance_bars: 0,
            stale_threshold_ticks: 0,
            enforce_feed_disagreement: true,
        }
    }
}

/// Integrity engine state (persisted by runtime later).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityState {
    /// Last complete bar end_ts per (symbol, timeframe).
    pub last_complete_end_ts: BTreeMap<(String, Timeframe), i64>,

    /// Last tick observed per feed.
    pub last_feed_tick: BTreeMap<FeedId, u64>,

    /// Fingerprints per bar key by feed (for disagreement detection).
    /// Stored as (close_micros, volume).
    pub fingerprints: BTreeMap<BarKey, BTreeMap<FeedId, (i64, i64)>>,

    /// Sticky flags.
    pub disarmed: bool,
    pub halted: bool,
}

impl IntegrityState {
    pub fn new() -> Self {
        Self {
            last_complete_end_ts: BTreeMap::new(),
            last_feed_tick: BTreeMap::new(),
            fingerprints: BTreeMap::new(),
            disarmed: false,
            halted: false,
        }
    }

    pub fn known_feeds(&self) -> BTreeSet<FeedId> {
        self.last_feed_tick.keys().cloned().collect()
    }
}

/// Decision returned for each bar evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityDecision {
    pub action: IntegrityAction,
    pub reason: IntegrityReason,
}

/// Actions are deterministic and side-effect free (caller enforces them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrityAction {
    Allow,
    Reject,
    Disarm,
    Halt,
}

/// Reasons for decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrityReason {
    Allowed,
    IncompleteBar,
    GapDetected,
    StaleFeed,
    FeedDisagreement,
    AlreadyHalted,
    AlreadyDisarmed,
}
