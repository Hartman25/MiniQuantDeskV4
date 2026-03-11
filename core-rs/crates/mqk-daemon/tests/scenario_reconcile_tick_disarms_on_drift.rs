//! Scenario: Periodic reconcile tick disarms on drift — R3-1 / REC-01R
//!
//! # Invariants under test
//!
//! 1. The daemon reconcile loop uses monotonic reconcile and disarms on fresh drift.
//! 2. A stale broker snapshot cannot clear a newer dirty reconcile state.
//! 3. Missing or placeholder broker snapshots fail closed instead of reporting clean.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use mqk_daemon::state::{self, AppState};
use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};

async fn armed_state() -> Arc<AppState> {
    let st = Arc::new(AppState::new());
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    {
        let mut s = st.status.write().await;
        s.integrity_armed = true;
        s.state = "running".to_string();
    }
    st
}

fn broker_snapshot_with_position(fetched_at_ms: i64, qty: i64) -> BrokerSnapshot {
    let mut snap = BrokerSnapshot::empty_at(fetched_at_ms);
    snap.positions.insert("SPY".to_string(), qty);
    snap
}

#[tokio::test]
async fn daemon_or_runtime_path_uses_monotonic_reconcile() {
    let st = armed_state().await;

    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = || Some(broker_snapshot_with_position(2_000, 200));

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(reconcile.status, "dirty");

    let ig = st.integrity.read().await;
    assert!(ig.disarmed, "integrity must be disarmed after fresh drift");
    assert!(ig.halted, "integrity must be halted after fresh drift");
}

#[tokio::test]
async fn reconcile_tick_does_not_disarm_on_clean_fresh_snapshot() {
    let st = armed_state().await;

    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = || Some(broker_snapshot_with_position(2_000, 100));

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(reconcile.status, "ok");

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "integrity must remain armed when reconcile is clean"
    );
    drop(ig);

    let s = st.status.read().await;
    assert_eq!(s.state, "running");
    assert!(s.integrity_armed);
}

#[tokio::test]
async fn stale_snapshot_cannot_reenable_dispatch() {
    let st = armed_state().await;
    let calls = Arc::new(AtomicUsize::new(0));

    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = {
        let calls = Arc::clone(&calls);
        move || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Some(broker_snapshot_with_position(2_000, 200))
            } else {
                Some(broker_snapshot_with_position(1_000, 100))
            }
        }
    };

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(reconcile.status, "dirty");
    assert!(
        reconcile
            .note
            .as_deref()
            .is_some_and(|note| note.contains("stale broker snapshot rejected")),
        "stale snapshot evidence must be retained while preserving the prior dirty reconcile state"
    );

    let ig = st.integrity.read().await;
    assert!(ig.disarmed, "stale snapshot must not re-enable dispatch");
    assert!(ig.halted, "stale snapshot must leave the daemon halted");
}

#[tokio::test]
async fn placeholder_snapshot_path_fails_closed() {
    let st = armed_state().await;

    let local_fn = LocalSnapshot::empty;
    let broker_fn = || Some(BrokerSnapshot::empty());

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(reconcile.status, "stale");
    assert!(
        reconcile
            .note
            .as_deref()
            .is_some_and(|note| note.contains("no timestamp") || note.contains("ambiguous")),
        "placeholder broker snapshots must fail closed with timestamp ambiguity evidence"
    );

    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "placeholder broker snapshot must disarm the daemon"
    );
}

#[tokio::test]
async fn missing_broker_snapshot_fails_closed() {
    let st = armed_state().await;

    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = || -> Option<BrokerSnapshot> { None };

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(reconcile.status, "unknown");
    assert!(
        reconcile
            .note
            .as_deref()
            .is_some_and(|note| note.contains("broker snapshot absent")),
        "missing broker snapshots must remain not-proven and fail closed"
    );

    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "missing broker snapshot must disarm the daemon"
    );
    assert!(ig.halted, "missing broker snapshot must halt the daemon");
}
