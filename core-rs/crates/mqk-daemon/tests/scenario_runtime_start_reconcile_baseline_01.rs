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
//! RECONCILE-DRIFT-BASELINE-SEED-01 extension:
//!
//! Third blocker: after an order is ACK'd by the broker, `active_orders` becomes
//! non-empty.  The prior RDL guard (RECONCILE-DRIFT-LIVE-01) switched to using
//! the execution snapshot at this point, which has empty positions (no fills yet).
//! The broker still holds the adopted baseline position (e.g. AAPL=4) → local=0
//! vs broker=4 → RECONCILE_DRIFT.
//!
//! Fix (RECONCILE-DRIFT-BASELINE-SEED-01): remove the `has_local_activity` guard.
//! Instead, always merge adopted baseline positions into the local portfolio
//! positions: total_local = baseline + portfolio_delta.  This is correct at every
//! stage — before fills (delta=0), after ACK (delta=0), after fills (delta>0).
//! Real broker drift (broker != baseline + delta) continues to halt.
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
//! | RDL01  | pure  | exec_snap Some+empty + matching baseline + broker → clean (no false drift)  |
//! | RDL02  | pure  | exec_snap Some+empty + absent baseline + broker-with-position → dirty       |
//! | RDL03  | pure  | exec_snap Some+empty + baseline(1) + broker(2) → dirty (mismatch)          |
//! | RDL04  | pure  | exec_snap with fill (+1) + baseline(1) → merged total=2; broker=2 → clean  |
//! | RDL05  | pure  | exec_snap with realized_pnl (no positions) + no baseline + broker flat→clean|
//! | RDL06  | pure  | exec_snap with active_orders + empty baseline + broker order → empty pos    |

use std::sync::Arc;
use tokio::sync::RwLock;

use mqk_reconcile::{
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, Side,
    SnapshotWatermark,
};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot};

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

// ===========================================================================
// RECONCILE-DRIFT-LIVE-01 tests
//
// These prove the patched `local_snapshot_provider` behavior when
// `execution_snapshot` is `Some` (the case that caused the second live blocker).
//
// The helper `patched_local_provider` simulates the exact logic of the
// patched closure in `build_execution_orchestrator`.
// ===========================================================================

/// Build an `ExecutionSnapshot` representing a fresh run state:
/// no positions, no realized P&L, no active orders.
fn fresh_exec_snapshot() -> ExecutionSnapshot {
    ExecutionSnapshot {
        run_id: Some(uuid::Uuid::new_v4()),
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros: 100_000_000_000,
            realized_pnl_micros: 0,
            positions: vec![],
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        snapshot_at_utc: chrono::Utc::now(),
        has_recent_terminal_fill: false,
    }
}

/// Build an `ExecutionSnapshot` that shows local portfolio activity:
/// a single AAPL long position (fill already applied).
fn exec_snapshot_with_position(symbol: &str, qty: i64) -> ExecutionSnapshot {
    let mut snap = fresh_exec_snapshot();
    snap.portfolio
        .positions
        .push(mqk_runtime::observability::PositionSnapshot {
            symbol: symbol.to_string(),
            net_qty: qty,
        });
    snap
}

/// Build an `ExecutionSnapshot` that shows realized P&L activity (sell applied).
fn exec_snapshot_with_pnl(realized_pnl_micros: i64) -> ExecutionSnapshot {
    let mut snap = fresh_exec_snapshot();
    snap.portfolio.realized_pnl_micros = realized_pnl_micros;
    snap
}

/// Build an `ExecutionSnapshot` that shows an active OMS order (order dispatched
/// but not yet filled).
fn exec_snapshot_with_active_order(order_id: &str) -> ExecutionSnapshot {
    let mut snap = fresh_exec_snapshot();
    snap.active_orders
        .push(mqk_runtime::observability::OrderSnapshot {
            order_id: order_id.to_string(),
            broker_order_id: None,
            symbol: "AAPL".to_string(),
            total_qty: 1,
            filled_qty: 0,
            status: "Open".to_string(),
        });
    snap
}

/// Simulate the `local_snapshot_provider` closure logic after
/// RECONCILE-DRIFT-BASELINE-SEED-01.
///
/// This mirrors the production behavior in `build_execution_orchestrator`:
/// when an execution snapshot is present, merge the adopted baseline positions
/// into the portfolio positions (baseline = starting position; portfolio = delta).
fn patched_local_provider(
    exec_snap: Option<&ExecutionSnapshot>,
    baseline: Option<&LocalSnapshot>,
    fallback_seed: &LocalSnapshot,
) -> LocalSnapshot {
    let Some(snapshot) = exec_snap else {
        // RSB01 path: no execution snapshot → use baseline or seed.
        return baseline.cloned().unwrap_or_else(|| fallback_seed.clone());
    };

    // RECONCILE-DRIFT-BASELINE-SEED-01: always merge baseline positions into
    // exec_snap portfolio positions.  baseline = starting broker position;
    // portfolio = delta accumulated via fills during the run.
    let mut local = LocalSnapshot::empty();
    for pos in &snapshot.portfolio.positions {
        if pos.net_qty != 0 {
            *local.positions.entry(pos.symbol.clone()).or_insert(0) += pos.net_qty;
        }
    }
    if let Some(bl) = baseline {
        for (sym, &bl_qty) in &bl.positions {
            *local.positions.entry(sym.clone()).or_insert(0) += bl_qty;
        }
    }
    local.positions.retain(|_, qty| *qty != 0);
    local
}

// ---------------------------------------------------------------------------
// RDL01 — exec_snap Some+empty + matching baseline + broker → clean
//
// Proves: when the execution snapshot is present but fresh (no activity),
// and the baseline matches the broker, Phase-0c does NOT fire ReconcileDrift.
//
// This is the live-smoke failure: loop tick 2 (exec_snap is Some after the
// initial lifecycle tick) caused a false halt.
// ---------------------------------------------------------------------------

#[test]
fn rdl01_fresh_exec_snap_with_matching_baseline_is_clean() {
    let baseline = local_with_position("AAPL", 1);
    let exec_snap = fresh_exec_snapshot();
    let broker = broker_with_position("AAPL", 1, ts());
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), Some(&baseline), &seed);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "RDL01: local provider must return baseline position when exec_snap is fresh"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RDL01: fresh exec_snap + matching baseline must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RDL02 — exec_snap Some+empty + absent baseline + broker-with-position → dirty
//
// Proves: when the execution snapshot is fresh AND no baseline has been adopted,
// the provider returns empty local truth.  If the broker has a position, the
// reconcile correctly halts (fail-closed — no baseline, no free pass).
// ---------------------------------------------------------------------------

#[test]
fn rdl02_fresh_exec_snap_absent_baseline_broker_position_is_dirty() {
    let exec_snap = fresh_exec_snapshot();
    let broker = broker_with_position("AAPL", 1, ts());
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    assert!(
        local.positions.is_empty(),
        "RDL02: no baseline → provider must return empty local"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RDL02: fresh exec_snap + absent baseline vs broker-with-position must be dirty"
    );
}

// ---------------------------------------------------------------------------
// RDL03 — exec_snap Some+empty + baseline(qty=1) + broker(qty=2) → dirty
//
// Proves: the baseline fallback does NOT bypass genuine mismatches.  If the
// broker has drifted from the baseline since adoption, reconcile still halts.
// ---------------------------------------------------------------------------

#[test]
fn rdl03_fresh_exec_snap_baseline_mismatch_is_dirty() {
    let baseline = local_with_position("AAPL", 1);
    let exec_snap = fresh_exec_snapshot();
    let broker = broker_with_position("AAPL", 2, ts()); // qty=2 but baseline=1
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), Some(&baseline), &seed);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "RDL03: local must carry baseline qty=1"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RDL03: baseline(1) vs broker(2) must be dirty; fix must not bypass real mismatches"
    );
}

// ---------------------------------------------------------------------------
// RDL04 — exec_snap with fill (+1) + baseline (AAPL=1) → merged total=2; broker=2 → clean
//
// Proves: after a fill is applied, baseline positions are merged into the
// portfolio delta.  Total local = baseline(1) + fill(+1) = 2.  Broker also
// has 2 (1 inherited + 1 from fill).  Reconcile must be clean.
//
// This is the correct behaviour replacing the old "exec_snap is authoritative
// and ignores baseline" guard from RECONCILE-DRIFT-LIVE-01.
// ---------------------------------------------------------------------------

#[test]
fn rdl04_fill_plus_baseline_merges_to_correct_total() {
    let baseline = local_with_position("AAPL", 1); // adopted before run
    let exec_snap = exec_snapshot_with_position("AAPL", 1); // +1 fill applied
    let broker = broker_with_position("AAPL", 2, ts()); // 1 baseline + 1 fill
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), Some(&baseline), &seed);

    // Merge: portfolio AAPL=1 + baseline AAPL=1 = 2.
    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(2),
        "RDL04: portfolio(AAPL=1) + baseline(AAPL=1) must merge to total=2"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RDL04: merged(AAPL=2) vs broker(AAPL=2) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RDL05 — exec_snap with realized_pnl (round-trip closed), no baseline → clean
//
// Proves: when no baseline is adopted and the portfolio shows realized P&L
// (position was opened and closed, now flat), the local snapshot is empty
// and the broker is also flat.  Reconcile must be clean.
// ---------------------------------------------------------------------------

#[test]
fn rdl05_realized_pnl_no_baseline_flat_broker_is_clean() {
    let exec_snap = exec_snapshot_with_pnl(500_000); // $0.50 realized profit; positions flat
    let broker = BrokerSnapshot::empty_at(ts()); // broker: flat
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    // No baseline, exec_snap has no positions (flat after round-trip) → empty local.
    assert!(
        local.positions.is_empty(),
        "RDL05: no baseline + flat exec_snap must return empty positions"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RDL05: empty local vs flat broker must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RDL06 — exec_snap with active_orders + empty baseline → empty positions
//
// Proves: with an empty baseline and a pending (unfilled) order, the merged
// local snapshot has no positions (portfolio delta=0, baseline=empty).
// ---------------------------------------------------------------------------

#[test]
fn rdl06_active_order_with_empty_baseline_has_no_positions() {
    let baseline = LocalSnapshot::empty(); // no baseline adopted
    let exec_snap = exec_snapshot_with_active_order("order-abc-001");
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), Some(&baseline), &seed);

    // portfolio positions empty + baseline empty → merged positions empty.
    assert!(
        local.positions.is_empty(),
        "RDL06: empty baseline + unfilled order must produce empty positions"
    );
}
