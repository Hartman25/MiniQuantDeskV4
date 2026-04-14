//! PT-AUTO-02: Per-run autonomous signal intake bound.
//!
//! Extracted from `state.rs` (MT-07A).  Contains the enforcement constant and
//! the four `AppState` accessor methods for the day-signal limit gate.
//!
//! The backing field (`day_signal_count: Arc<AtomicU32>`) is defined on
//! `AppState`; the reset on run-start is in `lifecycle.rs`.

use std::sync::atomic::Ordering;

use super::AppState;

/// PT-AUTO-02: Maximum number of strategy signals accepted per execution run.
///
/// Provides a hard per-run intake bound on the paper+alpaca signal ingestion
/// path.  After this many distinct signals are enqueued (Gate 7 Ok(true)),
/// Gate 1d refuses further signals with 409/day_limit_reached until the next
/// run start resets the counter.
///
/// 100 signals per run is conservative for a supervised paper session.  It is
/// not an economics guarantee — it is a safety bound.
pub(super) const MAX_AUTONOMOUS_SIGNALS_PER_RUN: u32 = 100;

impl AppState {
    /// Returns the current per-run signal intake count.
    pub fn day_signal_count(&self) -> u32 {
        self.day_signal_count.load(Ordering::SeqCst)
    }

    /// Increment the per-run signal intake counter by one.
    ///
    /// Called from the strategy signal route on Gate 7 Ok(true) (new enqueue).
    /// Not called for duplicates (Ok(false)) or Gate failures.
    pub(crate) fn increment_day_signal_count(&self) {
        self.day_signal_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Returns `true` when the per-run signal count has reached
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`.  Gate 1d refuses signals when true.
    pub fn day_signal_limit_exceeded(&self) -> bool {
        self.day_signal_count.load(Ordering::SeqCst) >= MAX_AUTONOMOUS_SIGNALS_PER_RUN
    }

    /// Test seam: set the day signal count to an arbitrary value.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    /// Used by PT-AUTO-02 proof tests to simulate a saturated counter without
    /// submitting 100 real signals.
    pub fn set_day_signal_count_for_test(&self, count: u32) {
        self.day_signal_count.store(count, Ordering::SeqCst);
    }
}
