//! Snapshot monotonicity watermark — Patch L8
//!
//! # Purpose
//!
//! Stale broker snapshots can mask position drift or drive incorrect sizing
//! decisions.  This module tracks the **fetch timestamp** of the last accepted
//! [`BrokerSnapshot`] and rejects any snapshot whose timestamp is older than
//! that watermark.
//!
//! # Invariants
//!
//! - **Non-decreasing**: a snapshot is accepted only if its `fetched_at_ms`
//!   is ≥ the last accepted snapshot's `fetched_at_ms`.
//! - **No-timestamp → stale**: a snapshot with `fetched_at_ms == 0` is always
//!   rejected (fail-closed).
//! - **Watermark advances only on acceptance**: rejections do not move the
//!   watermark.
//! - **Pure, no IO**: all logic is deterministic; the caller provides the
//!   timestamp and decides what to do with the result.

use crate::BrokerSnapshot;

// ---------------------------------------------------------------------------
// Freshness decision
// ---------------------------------------------------------------------------

/// Result of checking a [`BrokerSnapshot`] against the monotonicity watermark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotFreshness {
    /// Snapshot is fresh — its timestamp is ≥ the watermark.
    /// The watermark has been advanced to `fetched_at_ms` (on `accept`).
    Fresh,

    /// Snapshot's timestamp is strictly older than the last accepted snapshot.
    ///
    /// Fields carry the watermark value and the rejected timestamp for logging.
    Stale {
        /// The current watermark (last accepted `fetched_at_ms`).
        watermark_ms: i64,
        /// The rejected snapshot's `fetched_at_ms`.
        got_ms: i64,
    },

    /// Snapshot has no timestamp (`fetched_at_ms == 0`).
    ///
    /// Treated as stale under fail-closed semantics: a snapshot without a
    /// timestamp cannot be proven fresh and must not be trusted.
    NoTimestamp,
}

impl SnapshotFreshness {
    /// Returns `true` if the snapshot was accepted (watermark was or would be
    /// advanced to this snapshot's timestamp).
    pub fn is_fresh(&self) -> bool {
        matches!(self, SnapshotFreshness::Fresh)
    }

    /// Returns `true` if the snapshot should be rejected (stale or no timestamp).
    pub fn is_rejected(&self) -> bool {
        !self.is_fresh()
    }
}

// ---------------------------------------------------------------------------
// Watermark
// ---------------------------------------------------------------------------

/// Tracks the last accepted broker snapshot timestamp to enforce monotonicity.
///
/// Start with [`SnapshotWatermark::new`] (accepts any snapshot with a positive
/// timestamp).  Call [`accept`][SnapshotWatermark::accept] on each incoming
/// snapshot; only pass the snapshot to the reconciliation engine if the result
/// is [`SnapshotFreshness::Fresh`].
///
/// Use [`check`][SnapshotWatermark::check] for a read-only freshness probe
/// that does **not** advance the watermark.
#[derive(Clone, Debug)]
pub struct SnapshotWatermark {
    /// `fetched_at_ms` of the last accepted snapshot.
    /// Starts at `i64::MIN` so any snapshot with a positive timestamp is fresh.
    last_accepted_ms: i64,
}

impl Default for SnapshotWatermark {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotWatermark {
    /// Create a new watermark in its initial state.
    ///
    /// The initial watermark (`i64::MIN`) accepts any snapshot with a positive
    /// timestamp (i.e. any snapshot that has `fetched_at_ms > 0`).
    pub fn new() -> Self {
        Self {
            last_accepted_ms: i64::MIN,
        }
    }

    /// Check freshness **without** advancing the watermark.
    ///
    /// Useful for pre-flight validation or logging.  Does not mutate `self`.
    pub fn check(&self, snap: &BrokerSnapshot) -> SnapshotFreshness {
        if snap.fetched_at_ms == 0 {
            return SnapshotFreshness::NoTimestamp;
        }
        if snap.fetched_at_ms < self.last_accepted_ms {
            return SnapshotFreshness::Stale {
                watermark_ms: self.last_accepted_ms,
                got_ms: snap.fetched_at_ms,
            };
        }
        SnapshotFreshness::Fresh
    }

    /// Check freshness **and advance the watermark** if the snapshot is fresh.
    ///
    /// Returns the freshness decision.  If [`SnapshotFreshness::Fresh`], the
    /// internal watermark is updated to `snap.fetched_at_ms`.  Stale or
    /// no-timestamp results do not change the watermark.
    pub fn accept(&mut self, snap: &BrokerSnapshot) -> SnapshotFreshness {
        let result = self.check(snap);
        if result.is_fresh() {
            self.last_accepted_ms = snap.fetched_at_ms;
        }
        result
    }

    /// The `fetched_at_ms` of the last accepted snapshot.
    ///
    /// Returns `i64::MIN` if no snapshot has been accepted yet.
    pub fn last_accepted_ms(&self) -> i64 {
        self.last_accepted_ms
    }

    /// `true` if at least one snapshot has been accepted (watermark is no
    /// longer in its initial state).
    pub fn has_accepted_any(&self) -> bool {
        self.last_accepted_ms > i64::MIN
    }
}
