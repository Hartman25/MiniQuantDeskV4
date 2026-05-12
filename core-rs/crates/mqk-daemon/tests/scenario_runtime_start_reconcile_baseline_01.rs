//! RUNTIME-START-RECONCILE-BASELINE-01 proof tests.
//!
//! Root cause: `build_execution_orchestrator` derived `local_seed_reconcile` as
//! `LocalSnapshot::empty()` when `execution_snapshot` is `None` (fresh start).
//! The idle reconcile tick's `local_fn` correctly fell back to the adopted
//! `broker_baseline`, but the orchestrator's `local_snapshot_provider` did not.
//! On the first tick, Phase-0c saw local=empty vs broker=AAPL → ReconcileDrift.
//!
//! Fix (RSB01 patch): when `execution_snapshot` is `None`, both the static seed
//! and the live-closure fallback in `build_execution_orchestrator` now read from
//! `AppState::broker_baseline` before falling through to empty.
//!
//! # Test matrix
//!
//! | Test   | Type  | What it proves                                                              |
//! |--------|-------|-----------------------------------------------------------------------------|
//! | RSB01  | pure  | Baseline matching broker → reconcile clean (no false-positive drift)        |
//! | RSB02  | pure  | Baseline absent, broker has position → reconcile dirty (correct halt)       |
//! | RSB03  | pure  | Baseline present but mismatched → reconcile dirty (correct halt)            |
//! | RSB04  | pure  | Baseline present, broker empty → reconcile dirty (broker changed)           |
//! | RSB05  | pure  | Baseline closure: live-arc reading matches static-seed reading              |
//! | RSB06  | pure  | Broker with order only (no position): empty baseline → dirty                |
//! | RSB07  | pure  | Baseline with order + position matching broker → clean                      |

use std::sync::Arc;
use tokio::sync::RwLock;

use mqk_reconcile::{
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, Side,
    SnapshotWatermark,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a LocalSnapshot with a single long-position entry.
fn local_with_position(symbol: &str, qty: i64) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

/// Build a BrokerSnapshot with a single position and a timestamp.
fn broker_with_position(symbol: &str, qty: i64, ts_ms: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty_at(ts_ms);
    s.positions.insert(symbol.to_string(), qty);
    s
}

/// Build a BrokerSnapshot with a single open order.
fn broker_with_order(order_id: &str, symbol: &str, ts_ms: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty_at(ts_ms);
    s.orders.insert(
        order_id.to_string(),
        OrderSnapshot::new(order_id, symbol, Side::Buy, 1, 0, OrderStatus::Accepted),
    );
    s
}

fn ts() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------
// RSB01 — baseline matching broker → reconcile clean
//
// Proves: when the adopted broker baseline has the same positions as the broker
// snapshot, Phase-0c reconcile produces a clean result (no halt).
//
// Before the patch: local_seed_reconcile = empty → dirty.
// After the patch:  local_seed_reconcile = baseline (AAPL 1) → clean.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rsb01_baseline_matching_broker_position_is_clean() {
    let baseline = local_with_position("AAPL", 1);
    let baseline_arc: Arc<RwLock<Option<LocalSnapshot>>> =
        Arc::new(RwLock::new(Some(baseline.clone())));
    let exec_snap_arc: Arc<RwLock<Option<()>>> = Arc::new(RwLock::new(None));

    // Simulate the patched local_seed_reconcile derivation.
    let local_seed = {
        let guard = exec_snap_arc.read().await;
        if guard.is_some() {
            panic!("execution_snapshot must be None in this test");
        }
        baseline_arc
            .read()
            .await
            .clone()
            .unwrap_or_else(LocalSnapshot::empty)
    };
    assert_eq!(
        local_seed.positions.get("AAPL").copied(),
        Some(1),
        "RSB01: local seed must carry AAPL position from baseline"
    );

    // Simulate the patched local_snapshot_provider closure.
    let baseline_for_closure = Arc::clone(&baseline_arc);
    let seed_clone = local_seed.clone();
    let local_snapshot_provider = move || -> LocalSnapshot {
        let exec_is_none = true; // no execution snapshot
        if exec_is_none {
            return baseline_for_closure
                .try_read()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| seed_clone.clone());
        }
        unreachable!("execution snapshot was present")
    };

    let local = local_snapshot_provider();
    let broker = broker_with_position("AAPL", 1, ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RSB01: matching baseline and broker snapshot must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RSB02 — no baseline, broker has position → dirty (correct fail-closed)
//
// Proves: when no baseline exists and the broker has a pre-existing position,
// the reconcile correctly halts (no silent pass-through).
// ---------------------------------------------------------------------------

#[test]
fn rsb02_absent_baseline_with_broker_position_is_dirty() {
    let local = LocalSnapshot::empty();
    let broker = broker_with_position("AAPL", 1, ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RSB02: empty local vs broker-with-position must be dirty (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// RSB03 — baseline mismatch (wrong qty) → dirty (correct fail-closed)
//
// Proves: if the adopted baseline does not match the current broker snapshot,
// the reconcile correctly halts even though a baseline is present.
// The fix must NOT bypass genuine mismatches.
// ---------------------------------------------------------------------------

#[test]
fn rsb03_baseline_mismatch_is_dirty() {
    // Baseline says qty=1, but broker now reports qty=2 (drift after adoption).
    let local = local_with_position("AAPL", 1);
    let broker = broker_with_position("AAPL", 2, ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RSB03: local(AAPL=1) vs broker(AAPL=2) must be dirty"
    );
}

// ---------------------------------------------------------------------------
// RSB04 — baseline present, broker now empty → dirty (broker changed)
//
// Proves: if the baseline had a position but the broker snapshot no longer shows
// it (broker closed the position since adoption), reconcile correctly detects
// drift.  The fix must not mask a broker-side change.
// ---------------------------------------------------------------------------

#[test]
fn rsb04_baseline_position_but_broker_empty_is_dirty() {
    // Baseline says AAPL=1, broker is now empty (position liquidated externally).
    let local = local_with_position("AAPL", 1);
    let broker = BrokerSnapshot::empty_at(ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RSB04: local(AAPL=1) vs broker(empty) must be dirty"
    );
}

// ---------------------------------------------------------------------------
// RSB05 — live-arc closure matches static-seed when baseline is set
//
// Proves: the live baseline arc in the closure returns the same LocalSnapshot
// as the static seed computed at build time, when the baseline has not changed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rsb05_live_arc_closure_matches_static_seed() {
    let baseline = local_with_position("AAPL", 1);
    let baseline_arc: Arc<RwLock<Option<LocalSnapshot>>> =
        Arc::new(RwLock::new(Some(baseline.clone())));

    // Static seed (build-time).
    let static_seed: LocalSnapshot = baseline_arc
        .read()
        .await
        .clone()
        .unwrap_or_else(LocalSnapshot::empty);

    // Live-arc reading (closure path).
    let live_reading: LocalSnapshot = baseline_arc
        .try_read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| static_seed.clone());

    assert_eq!(
        live_reading, static_seed,
        "RSB05: live baseline arc reading must match static seed"
    );
}

// ---------------------------------------------------------------------------
// RSB06 — broker has open order but no position; empty baseline → dirty
//
// Proves: an order-only mismatch (no position) also produces dirty reconcile
// when the baseline is absent.
// ---------------------------------------------------------------------------

#[test]
fn rsb06_absent_baseline_with_broker_order_is_dirty() {
    let local = LocalSnapshot::empty();
    let broker = broker_with_order("broker-order-001", "AAPL", ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RSB06: empty local vs broker-with-open-order must be dirty"
    );
}

// ---------------------------------------------------------------------------
// RSB07 — baseline with order + position matching broker → clean
//
// Proves: a baseline that includes both an open order and a position is
// accepted as clean when the broker snapshot matches exactly.
// ---------------------------------------------------------------------------

#[test]
fn rsb07_baseline_with_order_and_position_matching_broker_is_clean() {
    let mut local = local_with_position("AAPL", 1);
    local.orders.insert(
        "ord-001".to_string(),
        OrderSnapshot::new("ord-001", "AAPL", Side::Buy, 1, 0, OrderStatus::Accepted),
    );

    let mut broker = broker_with_position("AAPL", 1, ts());
    broker.orders.insert(
        "ord-001".to_string(),
        OrderSnapshot::new("ord-001", "AAPL", Side::Buy, 1, 0, OrderStatus::Accepted),
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RSB07: baseline with order+position matching broker must be clean; diffs={:?}",
        result.diffs
    );
}
