//! Reconcile freshness guard — Patch B3.
//!
//! Provides [`ReconcileFreshnessGuard`]: a [`ReconcileGate`] implementation
//! that enforces both freshness (reconcile ran recently) and cleanliness (last
//! result was CLEAN) before broker dispatch is permitted.
//!
//! The clock is an injectable `Fn() -> i64` returning epoch-milliseconds,
//! enabling deterministic unit tests without mocking system time.

use crate::gateway::ReconcileGate;

/// Freshness-aware [`ReconcileGate`] implementation — Patch B3.
///
/// `BrokerGateway` evaluates `ReconcileGate::is_clean()` before every
/// broker submit/cancel/replace.  Before B3 the only implementations were
/// boolean stubs.  `ReconcileFreshnessGuard` is the production implementation:
/// it fails **closed** when:
///
/// - Reconcile has **never run** (fail-closed at boot).
/// - The most recent clean reconcile is **older than `freshness_bound_ms`**
///   (stale watermark).
/// - The most recent reconcile result was **dirty** (clears the timestamp).
///
/// # Clock injection
///
/// The guard takes `C: Fn() -> i64` returning epoch-milliseconds.
/// In production, pass the system wall clock.  In tests, pass a closure over a
/// [`std::cell::Cell<i64>`] for deterministic time control without mocks.
pub struct ReconcileFreshnessGuard<C>
where
    C: Fn() -> i64,
{
    /// Maximum allowed age (ms) of the last clean reconcile.
    /// Dispatch is blocked if `clock() - last_clean_at_ms > freshness_bound_ms`.
    freshness_bound_ms: i64,
    /// Epoch-ms when the last CLEAN reconcile was recorded.
    /// `None` if reconcile has never run or the last result was dirty.
    last_clean_at_ms: Option<i64>,
    /// Clock returning epoch-milliseconds. Injected for deterministic testing.
    clock: C,
}

impl<C: Fn() -> i64> ReconcileFreshnessGuard<C> {
    /// Create a new guard with the given freshness bound and clock.
    ///
    /// Starts with no recorded clean reconcile (`last_clean_at_ms = None`), so
    /// `is_clean()` returns `false` until the first clean reconcile is recorded.
    pub fn new(freshness_bound_ms: i64, clock: C) -> Self {
        Self {
            freshness_bound_ms,
            last_clean_at_ms: None,
            clock,
        }
    }

    /// Record the result of a reconcile pass.
    ///
    /// - `is_clean = true` — records the current clock time as the last clean
    ///   reconcile timestamp; the gate returns `true` until the freshness bound
    ///   is exceeded.
    /// - `is_clean = false` — clears the recorded timestamp; the gate returns
    ///   `false` immediately (dirty reconcile is fail-closed).
    pub fn record_reconcile_result(&mut self, is_clean: bool) {
        if is_clean {
            self.last_clean_at_ms = Some((self.clock)());
        } else {
            self.last_clean_at_ms = None;
        }
    }

    /// Epoch-ms of the last recorded clean reconcile, or `None` if none has run.
    pub fn last_clean_at_ms(&self) -> Option<i64> {
        self.last_clean_at_ms
    }

    fn eval_gate(&self) -> bool {
        match self.last_clean_at_ms {
            None => false,
            Some(t) => {
                let elapsed = (self.clock)() - t;
                elapsed <= self.freshness_bound_ms
            }
        }
    }
}

impl<C: Fn() -> i64> ReconcileGate for ReconcileFreshnessGuard<C> {
    /// Returns `true` only if a clean reconcile was recorded within the
    /// freshness bound.
    ///
    /// Called by `BrokerGateway::enforce_gates` before every broker operation.
    /// A stale or absent clean reconcile blocks dispatch (fail-closed).
    fn is_clean(&self) -> bool {
        self.eval_gate()
    }
}
