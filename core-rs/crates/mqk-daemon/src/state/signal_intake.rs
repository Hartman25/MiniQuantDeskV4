//! PT-AUTO-02: Per-run autonomous signal intake bound.
//! DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: Per-run dedup state for signal-blocked alerts.
//!
//! Extracted from `state.rs` (MT-07A).  Contains the enforcement constant and
//! the accessor methods for the day-signal limit gate and B5/day-limit alert dedup.
//!
//! The backing fields are defined on `AppState`; resets on run-start are in `lifecycle.rs`.

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

    // -----------------------------------------------------------------------
    // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: dedup helpers
    // -----------------------------------------------------------------------

    /// Returns `true` the FIRST time this symbol is claimed for a B5 alert this
    /// run; `false` on subsequent calls (already alerted).
    ///
    /// Thread-safe: uses an async RwLock acquire + insert.  Called from the
    /// execution loop tick context so the await point is acceptable.
    pub(crate) async fn try_claim_b5_alert(&self, symbol: &str) -> bool {
        let mut set = self.b5_alerted_symbols.write().await;
        set.insert(symbol.to_string())
    }

    /// Returns `true` the FIRST time the day-limit Discord alert is claimed for
    /// the current run; `false` on all subsequent calls.
    ///
    /// Uses an atomic CAS so multiple concurrent signal POSTs that all hit the
    /// limit simultaneously produce at most one Discord alert.
    pub(crate) fn try_claim_day_limit_alert(&self) -> bool {
        self.day_limit_alert_fired
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Reset B5 dedup set and day-limit alert flag.  Called at run start so each
    /// new run gets fresh dedup state.
    pub(super) fn reset_signal_blocked_alert_state(&self) {
        // Clear the B5 symbol set synchronously using try_write; always succeeds
        // because lifecycle.rs holds the exclusive lifecycle_op lock during start.
        if let Ok(mut set) = self.b5_alerted_symbols.try_write() {
            set.clear();
        }
        self.day_limit_alert_fired.store(false, Ordering::SeqCst);
    }
}
