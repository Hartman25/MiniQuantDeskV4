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
//! RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01 supersession (formerly
//! RECONCILE-DRIFT-BASELINE-SEED-01 / RECONCILE-DRIFT-LIVE-01):
//!
//! A third blocker was identified by HOSTILE-AUDIT-REPO-STATE-01:
//! `seed_portfolio_from_baseline` (RUNTIME-POSITION-SEED-ON-START-01, commit
//! 16209ba) now mutates the live `PortfolioState` directly at run start, so
//! `execution_snapshot.portfolio.positions` ALREADY carries
//! `baseline + same_run_fill_delta`.  The RECONCILE-DRIFT-BASELINE-SEED-01 merge
//! (re-adding `baseline` on top of the already-seeded snapshot) therefore
//! double-counted it: local = fills + 2x baseline, while broker = fills +
//! baseline — a guaranteed `PositionQtyMismatch` → `ReconcileDrift` halt/disarm
//! the moment any baseline position existed.
//!
//! Fix (RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01): `seed_portfolio_from_baseline`
//! is the SOLE baseline-entry point.  `local_snapshot_provider` /
//! `local_fn` now derive local truth directly from the seeded execution
//! snapshot via `reconcile_local_snapshot_from_runtime_with_sides` — no
//! re-merge.  This remains correct at every stage of the run lifecycle:
//!   - before any fills: snapshot already shows baseline (seeded once at start)
//!   - after order ACK (no fill yet): snapshot still shows baseline only
//!   - after fills: snapshot shows baseline + delta (folded by apply_entry on
//!     top of the seeded portfolio)
//! Real broker drift (broker != seeded snapshot total) continues to halt.
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
//! | RDL01  | pure  | seeded exec_snap (baseline=1, no fills) + broker=1 → clean (read-through)   |
//! | RDL02  | pure  | exec_snap Some+empty (no baseline) + broker-with-position → dirty           |
//! | RDL03  | pure  | seeded exec_snap (baseline=1) + broker drifted to 2 → dirty (mismatch)      |
//! | RDL04  | pure  | seeded exec_snap (baseline=1 + fill=1, folded=2) + broker=2 → clean         |
//! | RDL05  | pure  | exec_snap with realized_pnl (no positions) + no baseline + broker flat→clean|
//! | RDL06  | pure  | exec_snap with active_orders, no baseline → empty positions (no seed)       |

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
// RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01 tests (formerly RECONCILE-DRIFT-LIVE-01)
//
// These prove the patched `local_snapshot_provider` behavior when
// `execution_snapshot` is `Some`.
//
// Updated model (post RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01): once a baseline
// is adopted, `seed_portfolio_from_baseline` (RUNTIME-POSITION-SEED-ON-START-01,
// commit 16209ba) folds it into the live `PortfolioState` ONCE, before the run
// loop starts — so `exec_snap.portfolio.positions` already shows
// `baseline + same_run_fill_delta`.  A "fresh" snapshot with empty positions is
// only realistic when NO baseline was adopted (seeding is then a no-op).
//
// The helper `patched_local_provider` mirrors the corrected closure: a direct,
// read-only extraction of `(symbol, net_qty)` from the snapshot — no baseline
// re-merge (that re-merge is exactly the double-count bug this patch removes).
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
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
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

/// Simulate the PATCHED `local_snapshot_provider` closure
/// (RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01).
///
/// `exec_snap.portfolio.positions` already reflects `seed_portfolio_from_baseline`
/// having folded the adopted baseline into the live `PortfolioState` ONCE at run
/// start (RUNTIME-POSITION-SEED-ON-START-01), plus any same-run fill delta
/// applied via `apply_entry` on top.  The provider derives local truth directly
/// from the snapshot — re-merging `baseline` here would double-count it.
fn patched_local_provider(
    exec_snap: Option<&ExecutionSnapshot>,
    baseline: Option<&LocalSnapshot>,
    fallback_seed: &LocalSnapshot,
) -> LocalSnapshot {
    let Some(snapshot) = exec_snap else {
        // RSB01 path (execution_snapshot is None): use baseline, else seed.
        // Unaffected by this fix — baseline has not yet been folded into any
        // portfolio because no run/snapshot exists yet.
        return baseline.cloned().unwrap_or_else(|| fallback_seed.clone());
    };

    // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: read the seeded snapshot directly
    // — no baseline re-merge.  The snapshot already carries
    // `baseline + same_run_fill_delta` (folded in once, upstream, by
    // seed_portfolio_from_baseline + apply_entry).
    let mut local = LocalSnapshot::empty();
    for pos in &snapshot.portfolio.positions {
        if pos.net_qty != 0 {
            local.positions.insert(pos.symbol.clone(), pos.net_qty);
        }
    }
    local
}

// ---------------------------------------------------------------------------
// RDL01 — exec_snap Some, seeded with adopted baseline + broker matching → clean
//
// Proves: once a baseline is adopted, `seed_portfolio_from_baseline` folds it
// into the portfolio BEFORE the first execution snapshot is taken — so even a
// "fresh" (no-fills-yet) snapshot already shows the baseline qty.  Reading it
// directly (no re-merge) gives local == baseline == broker → clean.
//
// This is the live-smoke scenario: the first post-seed snapshot must already
// agree with the broker without any further arithmetic.
// ---------------------------------------------------------------------------

#[test]
fn rdl01_seeded_exec_snap_with_matching_baseline_is_clean() {
    // Baseline AAPL=1 was adopted and folded into the portfolio at run start;
    // the snapshot taken at the first tick already shows AAPL=1 (no fills yet).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);
    let broker = broker_with_position("AAPL", 1, ts());
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "RDL01: provider must read the seeded snapshot's AAPL=1 directly (a re-merge would yield 2)"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RDL01: seeded exec_snap(AAPL=1) vs broker(AAPL=1) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RDL02 — exec_snap Some+empty + absent baseline + broker-with-position → dirty
//
// Proves: when no baseline was adopted, `seed_portfolio_from_baseline` is a
// no-op and the snapshot genuinely stays flat.  The provider then returns an
// empty local truth.  If the broker has a position, reconcile correctly halts
// (fail-closed — no baseline, no free pass).
// ---------------------------------------------------------------------------

#[test]
fn rdl02_fresh_exec_snap_absent_baseline_broker_position_is_dirty() {
    let exec_snap = fresh_exec_snapshot();
    let broker = broker_with_position("AAPL", 1, ts());
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    assert!(
        local.positions.is_empty(),
        "RDL02: no baseline (no-op seeding) → provider must return empty local"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RDL02: fresh exec_snap + absent baseline vs broker-with-position must be dirty"
    );
}

// ---------------------------------------------------------------------------
// RDL03 — seeded exec_snap (baseline=1, no fill delta yet) + broker drifted to 2 → dirty
//
// Proves: the no-merge provider does NOT mask genuine drift.  The snapshot
// already shows the seeded baseline qty=1 (no fills yet); if the broker has
// drifted to qty=2 since adoption, reconcile still halts.
// ---------------------------------------------------------------------------

#[test]
fn rdl03_seeded_exec_snap_broker_drift_is_dirty() {
    // Baseline AAPL=1 was adopted and seeded; snapshot shows AAPL=1 (no fills yet).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);
    let broker = broker_with_position("AAPL", 2, ts()); // broker drifted to 2 since adoption
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "RDL03: provider must read the seeded snapshot's AAPL=1 directly"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RDL03: seeded local(1) vs broker(2) must be dirty; fix must not bypass real mismatches"
    );
}

// ---------------------------------------------------------------------------
// RDL04 — seeded exec_snap (baseline=1 + same-run fill +1 = 2) + broker=2 → clean
//
// Proves: once a same-run fill is applied on top of the seeded baseline, the
// snapshot shows the FOLDED total `baseline + delta = 1 + 1 = 2`
// (seed_portfolio_from_baseline ran once at start; apply_entry added the fill
// on top of the same live PortfolioState).  Reading it directly — no re-merge
// — gives local=2, matching broker=2.
//
// RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: the prior model treated the snapshot
// as carrying only the same-run delta (+1) and merged the baseline (+1) back
// in to reach 2 — but the snapshot already contained 2.  The merge produced
// local=3 (delta 1 + baseline 1 + re-merged baseline 1), a guaranteed
// PositionQtyMismatch against broker=2.  Fixed fixture: `exec_snapshot_with_
// position("AAPL", 2)` — the realistic post-seed-and-fill snapshot total —
// proves the corrected (no-merge) provider reads it through cleanly.
// ---------------------------------------------------------------------------

#[test]
fn rdl04_seeded_baseline_plus_fill_reads_through_to_correct_total() {
    let exec_snap = exec_snapshot_with_position("AAPL", 2); // seeded baseline(1) + fill(+1), folded
    let broker = broker_with_position("AAPL", 2, ts()); // 1 baseline + 1 fill
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    // Direct read: snapshot already shows the folded total = 2. No re-merge.
    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(2),
        "RDL04: seeded snapshot AAPL=2 (baseline 1 + fill 1, folded) must read through as local=2 \
         (a re-merge would have produced local=3)"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RDL04: local(AAPL=2) vs broker(AAPL=2) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RDL05 — exec_snap with realized_pnl (round-trip closed), no baseline → clean
//
// Proves: when no baseline is adopted and the portfolio shows realized P&L
// (position was opened and closed, now flat), the local snapshot is empty
// and the broker is also flat.  Reconcile must be clean.  Unaffected by this
// fix — there is no baseline to double-count.
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
// RDL06 — exec_snap with active_orders, no baseline adopted → no positions
//
// Proves: with no baseline adopted (seeding is a no-op) and a pending
// (unfilled) order, the snapshot's portfolio positions stay empty, and the
// provider — reading directly, no merge — returns an empty local truth.
// ---------------------------------------------------------------------------

#[test]
fn rdl06_active_order_no_baseline_has_no_positions() {
    let exec_snap = exec_snapshot_with_active_order("order-abc-001");
    let seed = LocalSnapshot::empty();

    let local = patched_local_provider(Some(&exec_snap), None, &seed);

    // portfolio positions empty (no baseline to seed, no fill yet) → empty local.
    assert!(
        local.positions.is_empty(),
        "RDL06: no baseline + unfilled order must produce empty positions"
    );
}
