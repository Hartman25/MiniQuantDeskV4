//! Scenario: Periodic reconcile tick disarms on drift — R3-1
//!
//! # Invariants under test
//!
//! 1. When `spawn_reconcile_tick` fires and `reconcile_tick` returns
//!    `DriftAction::HaltAndDisarm`, `AppState.integrity.disarmed` MUST become
//!    `true`, `status.state` MUST become `"halted"`, and
//!    `status.integrity_armed` MUST become `false`.
//!
//! 2. When reconcile is CLEAN, none of the above state must change — the
//!    system remains armed and running.
//!
//! 3. When `broker_fn` returns `None` (no snapshot available yet), the tick
//!    is silently skipped and the system remains armed.
//!
//! All tests are pure in-process; no DB or network required.

use std::{sync::Arc, time::Duration};

use mqk_daemon::state::{self, AppState};
use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};

// ---------------------------------------------------------------------------
// Helper: create an armed AppState (not the default disarmed boot state)
// ---------------------------------------------------------------------------

async fn armed_state() -> Arc<AppState> {
    let st = Arc::new(AppState::new());
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
    }
    {
        let mut s = st.status.write().await;
        s.integrity_armed = true;
        s.state = "running".to_string();
    }
    st
}

// ---------------------------------------------------------------------------
// 1. Position drift triggers disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconcile_tick_disarms_on_position_drift() {
    let st = armed_state().await;

    // Local: SPY qty = 100.  Broker: SPY qty = 200.  Positions diverge → drift.
    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = || {
        let mut snap = BrokerSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 200);
        Some(snap)
    };

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    // Allow multiple tick intervals for the background task to fire.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "integrity must be disarmed after reconcile position drift"
    );
    drop(ig);

    let s = st.status.read().await;
    assert_eq!(
        s.state, "halted",
        "status.state must be 'halted' after drift"
    );
    assert!(
        !s.integrity_armed,
        "integrity_armed must be false after drift"
    );
}

// ---------------------------------------------------------------------------
// 2. Clean reconcile does NOT disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconcile_tick_does_not_disarm_on_clean() {
    let st = armed_state().await;

    // Both local and broker agree: SPY qty = 100.  Reconcile is CLEAN.
    let local_fn = || {
        let mut snap = LocalSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        snap
    };
    let broker_fn = || {
        let mut snap = BrokerSnapshot::empty();
        snap.positions.insert("SPY".to_string(), 100);
        Some(snap)
    };

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "integrity must remain armed when reconcile is clean"
    );
    drop(ig);

    let s = st.status.read().await;
    assert_eq!(
        s.state, "running",
        "status.state must remain 'running' when reconcile is clean"
    );
    assert!(
        s.integrity_armed,
        "integrity_armed must remain true when reconcile is clean"
    );
}

// ---------------------------------------------------------------------------
// 3. No broker snapshot → tick is skipped → state unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconcile_tick_skips_when_no_broker_snapshot() {
    let st = armed_state().await;

    // Local has a position; broker always returns None.
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

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "integrity must remain armed when broker snapshot is absent"
    );
    drop(ig);

    let s = st.status.read().await;
    assert_eq!(
        s.state, "running",
        "status.state must remain 'running' when broker snapshot is absent"
    );
}
