//! Scenario: Stale broker snapshot blocked by reconcile_monotonic — Patch B2
//!
//! # Invariant under test
//!
//! Before Patch B2, `reconcile(local, broker)` accepted any `BrokerSnapshot`
//! regardless of its timestamp.  A caller could pass a snapshot with
//! `fetched_at_ms=100` after one with `fetched_at_ms=200` and the engine would
//! silently compare stale broker state against current local state — potentially
//! masking real position drift.
//!
//! After Patch B2, `reconcile_monotonic(wm, local, broker)` is the required
//! production entry point.  It enforces snapshot monotonicity via
//! `SnapshotWatermark` before running any content comparison:
//!
//! - Fresh snapshot (timestamp ≥ watermark): watermark advances, reconcile runs.
//! - Stale snapshot (timestamp < watermark): `Err(StaleBrokerSnapshot)` returned
//!   immediately — no content comparison performed.
//! - No-timestamp snapshot (fetched_at_ms == 0): `Err(StaleBrokerSnapshot)` with
//!   `freshness = NoTimestamp` (fail-closed semantics).
//!
//! All tests are pure in-process; no DB or network required.

use mqk_reconcile::{
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, SnapshotFreshness, SnapshotWatermark,
    StaleBrokerSnapshot,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_local() -> LocalSnapshot {
    LocalSnapshot::empty()
}

fn snap_at(fetched_at_ms: i64) -> BrokerSnapshot {
    BrokerSnapshot::empty_at(fetched_at_ms)
}

// ---------------------------------------------------------------------------
// 1. Fresh snapshot on an empty watermark passes to reconcile
// ---------------------------------------------------------------------------

#[test]
fn fresh_snapshot_passes_monotonicity_and_runs_reconcile() {
    let mut wm = SnapshotWatermark::new();
    let local = empty_local();
    let broker = snap_at(1_000);

    let report =
        reconcile_monotonic(&mut wm, &local, &broker).expect("fresh snapshot must not be rejected");

    assert!(
        report.is_clean(),
        "flat empty snapshots must produce a clean reconcile"
    );
    assert_eq!(
        wm.last_accepted_ms(),
        1_000,
        "watermark must advance to accepted snapshot timestamp"
    );
}

// ---------------------------------------------------------------------------
// 2. Stale snapshot (older timestamp) is rejected — the core B2 invariant
// ---------------------------------------------------------------------------

#[test]
fn stale_snapshot_is_rejected_by_reconcile_monotonic() {
    let mut wm = SnapshotWatermark::new();
    let local = empty_local();

    // Accept t=200 (fresh on empty watermark).
    reconcile_monotonic(&mut wm, &local, &snap_at(200))
        .expect("t=200 must be fresh on an empty watermark");
    assert_eq!(wm.last_accepted_ms(), 200);

    // Now present t=100 — older than the accepted 200.
    let err = reconcile_monotonic(&mut wm, &local, &snap_at(100))
        .expect_err("PATCH B2: stale snapshot must be rejected");

    match &err.freshness {
        SnapshotFreshness::Stale {
            watermark_ms,
            got_ms,
        } => {
            assert_eq!(*watermark_ms, 200, "watermark evidence must be 200");
            assert_eq!(*got_ms, 100, "rejected timestamp evidence must be 100");
        }
        other => panic!("expected Stale freshness, got {other:?}"),
    }

    // Watermark must not advance after rejection.
    assert_eq!(
        wm.last_accepted_ms(),
        200,
        "watermark must not advance after stale rejection"
    );
}

// ---------------------------------------------------------------------------
// 3. No-timestamp snapshot is rejected (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn no_timestamp_snapshot_is_rejected_fail_closed() {
    let mut wm = SnapshotWatermark::new();
    let local = empty_local();
    let broker = BrokerSnapshot::empty(); // fetched_at_ms == 0

    let err = reconcile_monotonic(&mut wm, &local, &broker)
        .expect_err("no-timestamp snapshot must be rejected under fail-closed semantics");

    assert_eq!(
        err.freshness,
        SnapshotFreshness::NoTimestamp,
        "fetched_at_ms=0 must produce NoTimestamp freshness"
    );

    assert!(
        !wm.has_accepted_any(),
        "watermark must not advance after NoTimestamp rejection"
    );
}

// ---------------------------------------------------------------------------
// 4. StaleBrokerSnapshot implements Display for logging/audit
// ---------------------------------------------------------------------------

#[test]
fn stale_broker_snapshot_display_includes_diagnostic_evidence() {
    let stale_err = StaleBrokerSnapshot {
        freshness: SnapshotFreshness::Stale {
            watermark_ms: 500,
            got_ms: 100,
        },
    };
    let msg = stale_err.to_string();
    assert!(msg.contains("500"), "display must include watermark_ms=500");
    assert!(msg.contains("100"), "display must include got_ms=100");

    let no_ts_err = StaleBrokerSnapshot {
        freshness: SnapshotFreshness::NoTimestamp,
    };
    let msg2 = no_ts_err.to_string();
    assert!(
        msg2.contains("fetched_at_ms") || msg2.contains("no timestamp"),
        "display must mention timestamp absence; got: {msg2}"
    );
}

// ---------------------------------------------------------------------------
// 5. Sequence: fresh → stale → same → advance — only monotonic accepted
// ---------------------------------------------------------------------------

#[test]
fn sequence_only_monotonically_non_decreasing_snapshots_accepted() {
    let mut wm = SnapshotWatermark::new();
    let local = empty_local();

    // t=100 — fresh
    reconcile_monotonic(&mut wm, &local, &snap_at(100)).expect("t=100 must be fresh");
    assert_eq!(wm.last_accepted_ms(), 100);

    // t=50 — stale
    reconcile_monotonic(&mut wm, &local, &snap_at(50)).expect_err("t=50 must be rejected as stale");
    assert_eq!(
        wm.last_accepted_ms(),
        100,
        "watermark unchanged after stale"
    );

    // t=100 — same timestamp: non-decreasing semantics allow this
    reconcile_monotonic(&mut wm, &local, &snap_at(100)).expect("t=100 same must be fresh");
    assert_eq!(wm.last_accepted_ms(), 100);

    // t=200 — fresh, advances watermark
    reconcile_monotonic(&mut wm, &local, &snap_at(200)).expect("t=200 must be fresh");
    assert_eq!(wm.last_accepted_ms(), 200);

    // t=0 — no timestamp, rejected
    reconcile_monotonic(&mut wm, &local, &BrokerSnapshot::empty())
        .expect_err("t=0 must be rejected as NoTimestamp");
    assert_eq!(
        wm.last_accepted_ms(),
        200,
        "watermark unchanged after NoTimestamp"
    );
}
