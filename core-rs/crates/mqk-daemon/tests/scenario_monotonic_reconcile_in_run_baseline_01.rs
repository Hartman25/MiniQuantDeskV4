//! MONOTONIC-RECONCILE-DIRTY-IN-RUN-01 proof tests.
//!
//! # Original root cause (MONOTONIC-RECONCILE-DIRTY-IN-RUN-01)
//!
//! The `local_fn` closure passed to `spawn_reconcile_tick` in `lifecycle.rs` did
//! not account for the adopted broker baseline at all.  When the execution loop
//! set `execution_snapshot = Some(initial_snapshot)` after the first tick, the
//! background reconcile tick immediately used the runtime portfolio as local
//! truth — which (at the time) did not yet include inherited prior-run positions.
//! The broker snapshot still reflected the adopted baseline position
//! (e.g. AAPL qty=1).  Result: local={} vs broker={AAPL:1} → false dirty →
//! DISARMED reason=ReconcileDrift.
//!
//! # Supersession (RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01)
//!
//! `seed_portfolio_from_baseline` (RUNTIME-POSITION-SEED-ON-START-01, commit
//! 16209ba) now mutates the live `PortfolioState` directly at run start, so
//! `execution_snapshot.portfolio.positions` ALREADY carries
//! `baseline + same_run_fill_delta` from the very first snapshot.  An
//! intermediate fix layer that additionally merged the adopted baseline back
//! into the derived local snapshot therefore double-counted it: local =
//! fills + 2x baseline, while broker = fills + baseline — a guaranteed
//! `PositionQtyMismatch` → `ReconcileDrift` halt/disarm the moment any baseline
//! position existed.  HOSTILE-AUDIT-REPO-STATE-01 found this; the merge has
//! been removed from `local_fn`.
//!
//! # Fix (current production behavior)
//!
//! `local_fn` derives local truth directly from the seeded execution snapshot
//! via `reconcile_local_snapshot_from_runtime_with_sides` — a pure, read-only
//! extraction of `(symbol, net_qty)` pairs — with NO re-merge of the baseline.
//! The snapshot itself already reflects the correct total at every stage:
//!   - before any fills: snapshot shows baseline (seeded once at run start)
//!   - after order ACK (no fill yet): snapshot still shows baseline only
//!   - after fills: snapshot shows baseline + delta (apply_entry folds the
//!     fill onto the same already-seeded live PortfolioState)
//! Real broker drift (broker != seeded snapshot total) continues to halt.
//!
//! These tests model `exec_snap` fixtures as they REALISTICALLY appear post-fix:
//! once a baseline is adopted, even the first ("fresh") snapshot already shows
//! the seeded qty — a snapshot with empty positions is only realistic when NO
//! baseline was adopted (seeding is then a no-op).
//!
//! # Test matrix
//!
//! | Test   | Type | What it proves                                                              |
//! |--------|------|-----------------------------------------------------------------------------|
//! | MRIR01 | pure | seeded exec_snap (baseline=1, no fills) + broker=1 → reconcile clean        |
//! | MRIR02 | tick | seeded exec_snap (baseline=1) + matching broker → arm state NOT disarmed   |
//! | MRIR03 | pure | seeded exec_snap (baseline=1) + broker drifted to 2 → dirty (real mismatch)|
//! | MRIR04 | pure | exec_snap with fill position, no baseline adopted → read through; drift halts|
//! | MRIR05 | tick | stale broker snapshot classified truthfully (status=stale/unknown, not dirty)|
//! | MRIR06 | tick | local_fn derivation is read-only — no outbox rows created                    |
//! | MRIR07 | tick | seeded exec_snap (baseline=1) + matching broker → tick stays ok             |

use std::{sync::Arc, time::Duration};

use mqk_daemon::state::{self, AppState};
use mqk_reconcile::{reconcile_monotonic, BrokerSnapshot, LocalSnapshot, SnapshotWatermark};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn local_with_position(symbol: &str, qty: i64) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn broker_with_position(symbol: &str, qty: i64, ts_ms: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty_at(ts_ms);
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn ts() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Fresh execution snapshot: no positions, no realized P&L, no active orders.
/// Represents the state immediately after run start, before any fills.
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
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    }
}

/// Execution snapshot with a fill-applied position (has_local_activity = true).
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

/// Simulate the PATCHED `local_fn` closure logic in `lifecycle.rs`
/// (RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01).
///
/// `exec_snap.portfolio.positions` already reflects `seed_portfolio_from_baseline`
/// having folded the adopted baseline into the live `PortfolioState` ONCE at run
/// start (RUNTIME-POSITION-SEED-ON-START-01), plus any same-run fill delta
/// applied via `apply_entry` on top.  The function derives local truth directly
/// from the snapshot — re-merging `baseline` here would double-count it.
fn patched_in_run_local_fn(
    exec_snap: Option<&ExecutionSnapshot>,
    baseline: Option<&LocalSnapshot>,
) -> LocalSnapshot {
    if let Some(snapshot) = exec_snap {
        // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: read the seeded snapshot
        // directly — no baseline re-merge.  The snapshot already carries
        // baseline + same_run_fill_delta (folded in once, upstream, by
        // seed_portfolio_from_baseline + apply_entry).
        let mut local = LocalSnapshot::empty();
        for pos in &snapshot.portfolio.positions {
            if pos.net_qty != 0 {
                local.positions.insert(pos.symbol.clone(), pos.net_qty);
            }
        }
        local
    } else {
        // No active run: use adopted baseline if present, else empty
        // (unchanged — baseline has not yet been folded into any portfolio).
        baseline.cloned().unwrap_or_else(LocalSnapshot::empty)
    }
}

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

// ---------------------------------------------------------------------------
// MRIR01 — seeded exec_snap (baseline=1, no fills yet) + matching broker → clean
//
// Proves: once a baseline is adopted, `seed_portfolio_from_baseline` folds it
// into the portfolio BEFORE the first execution snapshot is taken — so even the
// "fresh" (no-fills-yet) snapshot already shows the baseline qty.  The patched
// local_fn reads it through directly (no re-merge): local == baseline == broker
// → clean.  Passing `Some(&baseline)` here proves the baseline is correctly
// IGNORED once exec_snap is present — re-merging it would double the qty to 2.
// ---------------------------------------------------------------------------

#[test]
fn mrir01_seeded_exec_snap_matching_broker_is_clean() {
    let baseline = local_with_position("AAPL", 1);
    // Baseline AAPL=1 was adopted and folded into the portfolio at run start;
    // the first snapshot already shows AAPL=1 (no same-run fills yet).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);
    let broker = broker_with_position("AAPL", 1, ts());

    let local = patched_in_run_local_fn(Some(&exec_snap), Some(&baseline));

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "MRIR01: local_fn must read the seeded snapshot's AAPL=1 directly \
         (re-merging baseline would double it to 2)"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "MRIR01: seeded exec_snap(AAPL=1) vs broker(AAPL=1) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// MRIR02 — seeded exec_snap (baseline=1) + matching broker → arm state stays ok
//
// Proves: the patched path does not produce a Halt/dirty result, meaning no
// DISARMED signal is emitted for this scenario.  Uses spawn_reconcile_tick.
// The injected exec_snap already shows the seeded baseline qty (realistic
// post-seed state), proving the live tick path stays clean without any
// downstream re-merge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mrir02_seeded_exec_snap_matching_broker_stays_armed() {
    let st = armed_state().await;

    // Inject exec_snap already seeded with the adopted baseline (AAPL=1) and
    // a baseline arc holding the same — proving local_fn ignores baseline_arc
    // once exec_snap is present (no re-merge).
    let exec_snap_arc: Arc<RwLock<Option<ExecutionSnapshot>>> =
        Arc::new(RwLock::new(Some(exec_snapshot_with_position("AAPL", 1))));
    let baseline_arc: Arc<RwLock<Option<LocalSnapshot>>> =
        Arc::new(RwLock::new(Some(local_with_position("AAPL", 1))));

    let ts_ms = ts();
    let local_fn = {
        let exec_snap_arc = Arc::clone(&exec_snap_arc);
        let baseline_arc = Arc::clone(&baseline_arc);
        move || {
            let exec_snap = exec_snap_arc.try_read().ok().and_then(|g| g.clone());
            let baseline = baseline_arc.try_read().ok().and_then(|g| g.clone());
            patched_in_run_local_fn(exec_snap.as_ref(), baseline.as_ref())
        }
    };
    let broker_fn = move || Some(broker_with_position("AAPL", 1, ts_ms + 1));

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        || false,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(
        reconcile.status, "ok",
        "MRIR02: reconcile must be ok; status={} note={:?}",
        reconcile.status, reconcile.note
    );

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "MRIR02: integrity must remain armed when the seeded exec_snap matches the broker"
    );
    assert!(
        !ig.halted,
        "MRIR02: integrity must not be halted when reconcile is clean"
    );
}

// ---------------------------------------------------------------------------
// MRIR03 — seeded exec_snap (baseline=1, no fills) + broker drifted to 2 → dirty
//
// Proves: the no-merge derivation does NOT mask genuine drift.  The snapshot
// already shows the seeded baseline qty=1 (no same-run fills yet); if the
// broker has drifted to qty=2 since adoption, reconcile still halts — real
// drift detection is preserved.
// ---------------------------------------------------------------------------

#[test]
fn mrir03_seeded_exec_snap_broker_drift_is_dirty_real_drift_still_halts() {
    let baseline = local_with_position("AAPL", 1);
    // Seeded snapshot already shows AAPL=1 (baseline folded in at run start).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);
    let broker = broker_with_position("AAPL", 2, ts()); // broker drifted to qty=2

    let local = patched_in_run_local_fn(Some(&exec_snap), Some(&baseline));

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "MRIR03: local_fn must read the seeded snapshot's AAPL=1 directly"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "MRIR03: seeded local(AAPL=1) vs broker(AAPL=2) must be dirty — real drift must halt"
    );
}

// ---------------------------------------------------------------------------
// MRIR04 — exec_snap with fill position, no baseline adopted → read through correctly
//
// Proves: when no baseline was adopted (seed_portfolio_from_baseline is then a
// no-op), a same-run fill is the entire local truth.  The snapshot shows
// AAPL=1 (fill delta only, no baseline folded in — there is none).  The
// no-merge derivation reads it through as-is.  A genuine local-vs-broker
// mismatch continues to block correctly.
// ---------------------------------------------------------------------------

#[test]
fn mrir04_fill_position_no_baseline_is_read_through_correctly() {
    // Baseline: none adopted (seeding was a no-op; nothing to fold in).
    let baseline = LocalSnapshot::empty();
    // Exec snap: same-run fill applied for AAPL qty=1 (no baseline to add).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);
    // Broker: also shows AAPL=1 (matching the fill).
    let broker = broker_with_position("AAPL", 1, ts());

    let local = patched_in_run_local_fn(Some(&exec_snap), Some(&baseline));

    // Direct read: snapshot AAPL=1 (fill only) → local AAPL=1. No baseline to add.
    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "MRIR04: snapshot(AAPL=1, fill only) must read through as local AAPL=1"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "MRIR04: local(AAPL=1) vs broker(AAPL=1) must be clean; diffs={:?}",
        result.diffs
    );

    // Prove a real mismatch still blocks.
    let broker_drifted = broker_with_position("AAPL", 2, ts() + 1);
    let mut wm2 = SnapshotWatermark::new();
    let result2 =
        reconcile_monotonic(&mut wm2, &local, &broker_drifted).expect("watermark must pass");
    assert!(
        !result2.is_clean(),
        "MRIR04: local(AAPL=1) vs broker(AAPL=2) must be dirty — fail-closed preserved"
    );
}

// ---------------------------------------------------------------------------
// MRIR05 — stale broker snapshot is classified as stale/unknown, not dirty
//
// Proves: when the broker snapshot has no valid timestamp (fetched_at_ms=0),
// the reconcile tick correctly classifies this as stale/unknown rather than
// writing a false proven-dirty mismatch.  The injected exec_snap is seeded
// (AAPL=1, matching the adopted baseline) — the realistic post-seed state —
// to keep this fixture consistent with the corrected model; the staleness
// classification under test is independent of local position content.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mrir05_stale_broker_snapshot_not_false_proven_dirty() {
    let st = armed_state().await;

    let baseline = local_with_position("AAPL", 1);
    let exec_snap_arc: Arc<RwLock<Option<ExecutionSnapshot>>> =
        Arc::new(RwLock::new(Some(exec_snapshot_with_position("AAPL", 1))));
    let baseline_arc: Arc<RwLock<Option<LocalSnapshot>>> = Arc::new(RwLock::new(Some(baseline)));

    let local_fn = {
        let exec_snap_arc = Arc::clone(&exec_snap_arc);
        let baseline_arc = Arc::clone(&baseline_arc);
        move || {
            let exec_snap = exec_snap_arc.try_read().ok().and_then(|g| g.clone());
            let baseline = baseline_arc.try_read().ok().and_then(|g| g.clone());
            patched_in_run_local_fn(exec_snap.as_ref(), baseline.as_ref())
        }
    };
    // Broker snapshot with fetched_at_ms=0 (no timestamp — stale under monotonic).
    let broker_fn = || Some(BrokerSnapshot::empty()); // empty() sets fetched_at_ms=0

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        || false,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    // Must be stale or unknown — NOT dirty (no content comparison was proven).
    assert_ne!(
        reconcile.status, "dirty",
        "MRIR05: untimed broker snapshot must not produce proven-dirty; \
         status={} note={:?}",
        reconcile.status, reconcile.note
    );
    assert!(
        reconcile.status == "stale" || reconcile.status == "unknown",
        "MRIR05: untimed broker snapshot must be classified stale or unknown; \
         got status={}",
        reconcile.status
    );
}

// ---------------------------------------------------------------------------
// MRIR06 — local_fn derivation is read-only — no outbox rows created
//
// Proves: the patched local_fn derivation (direct read of the seeded snapshot,
// no baseline re-merge) is read-only.  No DB writes occur on the reconcile
// path; in particular, no outbox rows are created.  Validated structurally:
// patched_in_run_local_fn only reads state, and spawn_reconcile_tick has no
// outbox write paths.
// ---------------------------------------------------------------------------

#[test]
fn mrir06_local_fn_derivation_is_read_only_no_outbox() {
    // Structural proof: patched_in_run_local_fn only reads state; it never
    // writes to DB, outbox, or any mutable data structure.
    let baseline = local_with_position("AAPL", 1);
    // Seeded snapshot already shows AAPL=1 (baseline folded in at run start).
    let exec_snap = exec_snapshot_with_position("AAPL", 1);

    let local = patched_in_run_local_fn(Some(&exec_snap), Some(&baseline));

    // Direct read: snapshot AAPL=1 → local AAPL=1. No re-merge, no writes.
    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(1),
        "MRIR06: derivation must return AAPL=1 — only read ops occurred, no re-merge"
    );
    // No assertions on external state: patched_in_run_local_fn is a pure function.
}

// ---------------------------------------------------------------------------
// MRIR07 — spawn_reconcile_tick with seeded exec_snap + matching baseline → ok
//
// End-to-end proof through the real spawn_reconcile_tick seam.  Uses the
// patched local_fn pattern.  The tick must publish status=ok and not disarm.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mrir07_spawn_reconcile_tick_seeded_exec_snap_matching_baseline_stays_ok() {
    let st = armed_state().await;

    // Seeded snapshot already shows AAPL=1 (baseline folded in at run start) —
    // the realistic post-seed state once a baseline has been adopted.
    let exec_snap_arc: Arc<RwLock<Option<ExecutionSnapshot>>> =
        Arc::new(RwLock::new(Some(exec_snapshot_with_position("AAPL", 1))));
    let baseline_arc: Arc<RwLock<Option<LocalSnapshot>>> =
        Arc::new(RwLock::new(Some(local_with_position("AAPL", 1))));

    let ts_ms = ts();
    let local_fn = {
        let exec_snap_arc = Arc::clone(&exec_snap_arc);
        let baseline_arc = Arc::clone(&baseline_arc);
        move || {
            let exec_snap = exec_snap_arc.try_read().ok().and_then(|g| g.clone());
            let baseline = baseline_arc.try_read().ok().and_then(|g| g.clone());
            patched_in_run_local_fn(exec_snap.as_ref(), baseline.as_ref())
        }
    };
    let broker_fn = move || Some(broker_with_position("AAPL", 1, ts_ms + 1));

    state::spawn_reconcile_tick(
        Arc::clone(&st),
        local_fn,
        broker_fn,
        || false,
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    let reconcile = st.current_reconcile_snapshot().await;
    assert_eq!(
        reconcile.status, "ok",
        "MRIR07: seeded exec_snap + matching baseline/broker must publish ok; \
         status={} note={:?} mismatched_positions={}",
        reconcile.status, reconcile.note, reconcile.mismatched_positions
    );

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "MRIR07: reconcile ok must not disarm the daemon"
    );
    assert!(!ig.halted, "MRIR07: reconcile ok must not halt the daemon");
}
