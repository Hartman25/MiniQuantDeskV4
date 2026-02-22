//! Scenario: Snapshot Monotonicity Enforced — Patch L8
//!
//! # Invariants under test
//!
//! 1. A snapshot with a positive timestamp is fresh on an empty watermark.
//! 2. A snapshot with the same timestamp as the watermark is accepted
//!    (non-decreasing, not strictly increasing).
//! 3. A snapshot with a timestamp older than the watermark is rejected as Stale.
//! 4. A snapshot with `fetched_at_ms == 0` is rejected as NoTimestamp.
//! 5. Watermark advances to the accepted snapshot's timestamp after `accept`.
//! 6. Watermark does NOT advance after a Stale rejection.
//! 7. Watermark does NOT advance after a NoTimestamp rejection.
//! 8. A sequence of snapshots: only monotonically non-decreasing ones are accepted.
//! 9. `check` is read-only — it does not advance the watermark.
//! 10. `has_accepted_any` reflects whether at least one snapshot has been accepted.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_reconcile::{BrokerSnapshot, SnapshotFreshness, SnapshotWatermark};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn snap_at(fetched_at_ms: i64) -> BrokerSnapshot {
    BrokerSnapshot::empty_at(fetched_at_ms)
}

fn snap_no_ts() -> BrokerSnapshot {
    BrokerSnapshot::empty() // fetched_at_ms == 0
}

// ---------------------------------------------------------------------------
// 1. Fresh snapshot on empty watermark
// ---------------------------------------------------------------------------

#[test]
fn fresh_snapshot_is_accepted_on_empty_watermark() {
    let mut wm = SnapshotWatermark::new();
    let snap = snap_at(1_000);

    let result = wm.accept(&snap);
    assert_eq!(
        result,
        SnapshotFreshness::Fresh,
        "first snapshot must be Fresh"
    );
    assert!(result.is_fresh());
    assert!(!result.is_rejected());
}

// ---------------------------------------------------------------------------
// 2. Same-timestamp snapshot is accepted (non-decreasing)
// ---------------------------------------------------------------------------

#[test]
fn same_timestamp_snapshot_is_accepted() {
    let mut wm = SnapshotWatermark::new();

    assert_eq!(wm.accept(&snap_at(5_000)), SnapshotFreshness::Fresh);

    // Same timestamp as watermark → still fresh (non-decreasing, not strict).
    assert_eq!(
        wm.accept(&snap_at(5_000)),
        SnapshotFreshness::Fresh,
        "same-ms snapshot must be accepted (non-decreasing semantics)"
    );
}

// ---------------------------------------------------------------------------
// 3. Stale snapshot is rejected
// ---------------------------------------------------------------------------

#[test]
fn stale_snapshot_is_rejected() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(10_000));

    let result = wm.accept(&snap_at(5_000)); // older
    assert!(
        result.is_rejected(),
        "snapshot older than watermark must be rejected"
    );
    match result {
        SnapshotFreshness::Stale {
            watermark_ms,
            got_ms,
        } => {
            assert_eq!(watermark_ms, 10_000);
            assert_eq!(got_ms, 5_000);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. No-timestamp snapshot is rejected
// ---------------------------------------------------------------------------

#[test]
fn no_timestamp_snapshot_is_rejected() {
    let mut wm = SnapshotWatermark::new();

    let result = wm.accept(&snap_no_ts());
    assert_eq!(
        result,
        SnapshotFreshness::NoTimestamp,
        "snapshot with fetched_at_ms == 0 must be rejected as NoTimestamp"
    );
    assert!(result.is_rejected());
}

#[test]
fn no_timestamp_rejected_even_after_prior_fresh_acceptance() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(1_000));

    let result = wm.accept(&snap_no_ts());
    assert_eq!(
        result,
        SnapshotFreshness::NoTimestamp,
        "no-timestamp snapshot must be rejected regardless of watermark state"
    );
}

// ---------------------------------------------------------------------------
// 5. Watermark advances on acceptance
// ---------------------------------------------------------------------------

#[test]
fn watermark_advances_on_each_accepted_snapshot() {
    let mut wm = SnapshotWatermark::new();
    assert!(!wm.has_accepted_any());

    wm.accept(&snap_at(1_000));
    assert_eq!(wm.last_accepted_ms(), 1_000);
    assert!(wm.has_accepted_any());

    wm.accept(&snap_at(2_000));
    assert_eq!(wm.last_accepted_ms(), 2_000);

    wm.accept(&snap_at(3_500));
    assert_eq!(wm.last_accepted_ms(), 3_500);
}

// ---------------------------------------------------------------------------
// 6. Watermark does NOT advance on Stale rejection
// ---------------------------------------------------------------------------

#[test]
fn watermark_unchanged_after_stale_rejection() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(10_000));
    assert_eq!(wm.last_accepted_ms(), 10_000);

    // Stale snapshot — watermark must stay at 10_000.
    let result = wm.accept(&snap_at(9_999));
    assert!(result.is_rejected());
    assert_eq!(
        wm.last_accepted_ms(),
        10_000,
        "watermark must not advance after Stale rejection"
    );
}

// ---------------------------------------------------------------------------
// 7. Watermark does NOT advance on NoTimestamp rejection
// ---------------------------------------------------------------------------

#[test]
fn watermark_unchanged_after_no_timestamp_rejection() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(7_000));

    let result = wm.accept(&snap_no_ts());
    assert!(result.is_rejected());
    assert_eq!(
        wm.last_accepted_ms(),
        7_000,
        "watermark must not advance after NoTimestamp rejection"
    );
}

// ---------------------------------------------------------------------------
// 8. Sequence of snapshots — only monotonic ones accepted
// ---------------------------------------------------------------------------

#[test]
fn sequence_of_snapshots_only_monotonic_ones_accepted() {
    let mut wm = SnapshotWatermark::new();

    // t=100 — fresh
    assert_eq!(wm.accept(&snap_at(100)), SnapshotFreshness::Fresh);
    assert_eq!(wm.last_accepted_ms(), 100);

    // t=200 — fresh, advances
    assert_eq!(wm.accept(&snap_at(200)), SnapshotFreshness::Fresh);
    assert_eq!(wm.last_accepted_ms(), 200);

    // t=150 — stale (out-of-order), rejected
    let r = wm.accept(&snap_at(150));
    assert!(r.is_rejected());
    assert_eq!(wm.last_accepted_ms(), 200, "watermark must stay at 200");

    // t=300 — fresh again
    assert_eq!(wm.accept(&snap_at(300)), SnapshotFreshness::Fresh);
    assert_eq!(wm.last_accepted_ms(), 300);

    // t=0 (no timestamp) — rejected
    assert_eq!(wm.accept(&snap_no_ts()), SnapshotFreshness::NoTimestamp);
    assert_eq!(wm.last_accepted_ms(), 300, "watermark must stay at 300");
}

// ---------------------------------------------------------------------------
// 9. check() is read-only — does not advance the watermark
// ---------------------------------------------------------------------------

#[test]
fn check_is_read_only_and_does_not_advance_watermark() {
    let wm = SnapshotWatermark::new();

    // check() returns Fresh for a positive-ts snapshot on empty watermark...
    let result = wm.check(&snap_at(5_000));
    assert_eq!(result, SnapshotFreshness::Fresh);

    // ...but the watermark has NOT advanced (still at initial state).
    assert!(
        !wm.has_accepted_any(),
        "check() must not advance the watermark"
    );
    assert_eq!(wm.last_accepted_ms(), i64::MIN);
}

#[test]
fn check_after_accept_reflects_watermark_correctly() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(8_000));

    // check() on a stale snapshot returns Stale but doesn't mutate wm.
    let result = wm.check(&snap_at(4_000));
    assert!(matches!(result, SnapshotFreshness::Stale { .. }));
    assert_eq!(
        wm.last_accepted_ms(),
        8_000,
        "check() must not change the watermark"
    );
}

// ---------------------------------------------------------------------------
// 10. has_accepted_any reflects acceptance state
// ---------------------------------------------------------------------------

#[test]
fn has_accepted_any_is_false_initially() {
    let wm = SnapshotWatermark::new();
    assert!(!wm.has_accepted_any());
}

#[test]
fn has_accepted_any_is_true_after_first_acceptance() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(1));
    assert!(wm.has_accepted_any());
}

#[test]
fn has_accepted_any_stays_true_even_after_subsequent_stale_rejection() {
    let mut wm = SnapshotWatermark::new();
    wm.accept(&snap_at(100));
    wm.accept(&snap_at(1)); // stale — rejected
    assert!(
        wm.has_accepted_any(),
        "has_accepted_any must remain true after a rejection"
    );
}
