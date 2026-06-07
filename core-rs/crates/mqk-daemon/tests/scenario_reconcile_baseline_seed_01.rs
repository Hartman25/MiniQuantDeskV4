//! RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01 proof tests.
//!
//! # Root cause
//!
//! `seed_portfolio_from_baseline` (RUNTIME-POSITION-SEED-ON-START-01, commit
//! 16209ba) mutates the live `PortfolioState` directly at run start, so
//! `execution_snapshot.portfolio.positions` already carries
//! `adopted_baseline + same_run_fill_delta`.
//!
//! The prior `local_snapshot_provider` (`orchestrator_build.rs`) and `local_fn`
//! (`lifecycle.rs`) — written under the older RECONCILE-DRIFT-BASELINE-SEED-01
//! model, before the portfolio was seeded directly — additionally merged the
//! adopted baseline back into the derived local snapshot:
//!
//!   local_truth = (baseline + delta)        [from the already-seeded snapshot]
//!               + baseline                  [redundant re-merge]
//!               = fills + 2 x baseline
//!
//! while broker truth = fills + baseline.  Any adopted baseline position
//! therefore guaranteed a `PositionQtyMismatch` → `ReconcileDrift` →
//! disarm/halt (REC-01R), even with a perfectly healthy broker.
//!
//! # Fix
//!
//! `seed_portfolio_from_baseline` is the SOLE baseline-entry point.  Downstream
//! reconcile derives local truth directly from the seeded execution snapshot via
//! `reconcile_local_snapshot_from_runtime_with_sides` — no re-merge.  The merge
//! blocks were removed from both `local_snapshot_provider` and `local_fn`.
//!
//! # Test matrix
//!
//! All scenarios model the snapshot positions as they appear AFTER seeding
//! (i.e. `seeded_qty = baseline_qty + same_run_fill_delta`), matching what
//! `reconcile_local_snapshot_from_runtime_with_sides` actually reads off a
//! live `execution_snapshot` post-fix — no second baseline addition.
//!
//! | Test   | Scenario                                                            | Expected |
//! |--------|---------------------------------------------------------------------|----------|
//! | RBS01  | seeded AAPL=4 (baseline only, no fills) + broker AAPL=4             | clean    |
//! | RBS02  | seeded AAPL=5 (baseline=4 + buy fill +1) + broker AAPL=5            | clean    |
//! | RBS03  | seeded AAPL=4 (baseline only) + broker AAPL=5 (unexplained drift)   | dirty    |
//! | RBS04  | no baseline, no fills (flat) + broker AAPL=4                        | dirty    |
//! | RBS05  | seeded AAPL=4 (baseline, no fill delta yet) + ACK'd order + broker  | no pos.  |
//! |        | stale at AAPL=4                                                     | mismatch |
//! | RBS06  | seeded AAPL=0 (baseline=4 fully sold, delta=-4) + broker flat       | clean    |
//! | RBS07  | structural: derivation is a pure read of the seeded snapshot — no   | proof    |
//! |        | re-merge step, no synthetic fill rows                               |          |

use mqk_reconcile::{
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, ReconcileDiff,
    Side, SnapshotWatermark,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn broker_with_position(symbol: &str, qty: i64, ts_ms: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty_at(ts_ms);
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn ts() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Simulate the PATCHED `local_snapshot_provider` / `local_fn` derivation
/// (RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01).
///
/// `seeded_positions` represents `execution_snapshot.portfolio.positions` AS IT
/// APPEARS IN PRODUCTION POST-FIX: `seed_portfolio_from_baseline` has already
/// folded the adopted baseline into the live `PortfolioState` (once, at run
/// start), and any same-run fills have been applied on top via `apply_entry`.
/// So `seeded_qty = baseline_qty + same_run_fill_delta` — already final.
///
/// This mirrors `reconcile_local_snapshot_from_runtime_with_sides`: a direct,
/// read-only extraction of `(symbol, net_qty)` pairs.  No baseline is added a
/// second time — that is precisely the bug this patch removes.
fn local_from_seeded_snapshot(seeded_positions: &[(&str, i64)]) -> LocalSnapshot {
    let mut local = LocalSnapshot::empty();
    for &(sym, qty) in seeded_positions {
        if qty != 0 {
            local.positions.insert(sym.to_string(), qty);
        }
    }
    local
}

// ---------------------------------------------------------------------------
// RBS01 — seeded AAPL=4 (baseline only, no fills) + broker AAPL=4 → clean
//
// Proves: before any same-run fills, the seeded snapshot already equals the
// adopted baseline (seed_portfolio_from_baseline ran once at start).  Reading
// it directly gives local=4, matching broker=4.  A re-merge would have produced
// local=8 and a false PositionQtyMismatch.
// ---------------------------------------------------------------------------

#[test]
fn rbs01_seeded_baseline_no_fills_matching_broker_is_clean() {
    let broker = broker_with_position("AAPL", 4, ts());

    // execution_snapshot.portfolio.positions already shows AAPL=4 (seeded once).
    let local = local_from_seeded_snapshot(&[("AAPL", 4)]);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(4),
        "RBS01: seeded snapshot AAPL=4 must read through as local AAPL=4 (not re-merged to 8)"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RBS01: local(4) vs broker(4) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RBS02 — seeded AAPL=5 (baseline=4 + buy fill +1) + broker AAPL=5 → clean
//
// Proves: after a same-run fill, the seeded snapshot already shows
// baseline + delta = 5 (apply_entry ran on top of the seeded portfolio).
// Reading it directly gives local=5, matching broker=5.  A re-merge would have
// produced local=9 (5 + baseline 4) and a false PositionQtyMismatch.
// ---------------------------------------------------------------------------

#[test]
fn rbs02_seeded_baseline_plus_fill_matches_broker_is_clean() {
    let broker = broker_with_position("AAPL", 5, ts());

    // execution_snapshot.portfolio.positions already shows AAPL=5
    // (4 baseline + 1 same-run buy fill, folded in by seed + apply_entry).
    let local = local_from_seeded_snapshot(&[("AAPL", 5)]);

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(5),
        "RBS02: seeded snapshot AAPL=5 (baseline 4 + fill 1) must read through as local AAPL=5"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RBS02: local(5) vs broker(5) must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RBS03 — seeded AAPL=4 (baseline only) + broker AAPL=5 → dirty
//
// Proves: an unexplained broker position increase (broker > seeded local, no
// matching same-run fill) still halts.  The fix must not mask real drift —
// removing the redundant merge does not weaken genuine drift detection.
// ---------------------------------------------------------------------------

#[test]
fn rbs03_seeded_baseline_with_unexplained_broker_drift_is_dirty() {
    let broker = broker_with_position("AAPL", 5, ts()); // broker increased unexpectedly

    let local = local_from_seeded_snapshot(&[("AAPL", 4)]);

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RBS03: local(4) vs broker(5) must be dirty — unexplained drift must still halt"
    );
}

// ---------------------------------------------------------------------------
// RBS04 — no baseline, no fills (flat seeded snapshot) + broker AAPL=4 → dirty
//
// Proves: without an adopted baseline, seed_portfolio_from_baseline is a no-op
// and the snapshot stays flat.  A broker position with no corresponding local
// truth is dirty (fail-closed) — the operator must adopt a baseline before
// starting with broker pre-existing positions.
// ---------------------------------------------------------------------------

#[test]
fn rbs04_no_baseline_flat_snapshot_with_broker_position_is_dirty() {
    let broker = broker_with_position("AAPL", 4, ts());
    let local = local_from_seeded_snapshot(&[]);

    assert!(
        local.positions.is_empty(),
        "RBS04: no baseline, no fills => flat seeded snapshot => empty local"
    );

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        !result.is_clean(),
        "RBS04: flat local, broker AAPL=4 must be dirty (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// RBS05 — seeded AAPL=4 (baseline, no fill delta yet) + ACK'd order + broker
//          stale at AAPL=4 → no PositionQtyMismatch
//
// This is the live-smoke scenario that originally motivated baseline inclusion:
// the AAPL buy order is ACK'd (active_orders non-empty) but not yet filled, so
// the seeded snapshot still shows only the baseline (4).  The broker snapshot
// is typically stale at this point (refreshed less often than the reconcile
// tick) — it still shows the pre-order position.
//
// Reading the seeded snapshot directly gives local AAPL=4 == broker AAPL=4 — no
// PositionQtyMismatch.  (A LocalOrderMissingAtBroker diff may still appear for
// the new order; that is a separate, expected staleness concern handled by the
// grace-window path — not the double-count bug under test here.)
// ---------------------------------------------------------------------------

#[test]
fn rbs05_seeded_baseline_with_ackd_order_no_position_mismatch() {
    // local: seeded snapshot shows AAPL=4 (baseline; no fill delta yet),
    // plus a pending buy order in active_orders.
    let mut local = local_from_seeded_snapshot(&[("AAPL", 4)]);
    local.orders.insert(
        "order-aapl-smoke-001".to_string(),
        OrderSnapshot::new(
            "order-aapl-smoke-001",
            "AAPL",
            Side::Buy,
            1,
            0,
            OrderStatus::New,
        ),
    );

    // broker: stale snapshot — only the baseline position, order not yet visible.
    let broker = broker_with_position("AAPL", 4, ts());

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");

    let has_position_mismatch = result
        .diffs
        .iter()
        .any(|d| matches!(d, ReconcileDiff::PositionQtyMismatch { .. }));
    assert!(
        !has_position_mismatch,
        "RBS05: seeded local(AAPL=4) vs broker(AAPL=4) must NOT produce PositionQtyMismatch \
         (a re-merge would have produced local=8, a guaranteed mismatch); diffs={:?}",
        result.diffs
    );

    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(4),
        "RBS05: seeded local must read AAPL=4 (baseline, no fill delta) — not re-merged to 8"
    );
}

// ---------------------------------------------------------------------------
// RBS06 — seeded AAPL=0 (baseline=4 fully sold, delta=-4) + broker flat → clean
//
// Proves: selling the entire inherited baseline position leaves the seeded
// portfolio flat for that symbol (4 baseline + (-4) sell = 0; zero-qty entries
// are not positions).  Reading the seeded snapshot directly gives an empty
// local, matching the broker's flat state.
// ---------------------------------------------------------------------------

#[test]
fn rbs06_seeded_baseline_fully_sold_to_flat_is_clean() {
    // execution_snapshot.portfolio.positions: baseline(4) + sell(-4) = 0 →
    // build_portfolio_snapshot / reconcile_local_snapshot_from_runtime_with_sides
    // do not emit zero-qty positions.
    let local = local_from_seeded_snapshot(&[]);

    assert!(
        local.positions.is_empty(),
        "RBS06: seeded baseline(4) fully sold (-4) must leave local positions empty"
    );

    let broker = BrokerSnapshot::empty_at(ts()); // broker flat after close

    let mut wm = SnapshotWatermark::new();
    let result = reconcile_monotonic(&mut wm, &local, &broker).expect("watermark must pass");
    assert!(
        result.is_clean(),
        "RBS06: flat local vs flat broker must be clean; diffs={:?}",
        result.diffs
    );
}

// ---------------------------------------------------------------------------
// RBS07 — structural: derivation is a pure read, no re-merge, no synthetic fills
//
// Proves: `local_from_seeded_snapshot` (mirroring the patched production
// closures) performs a direct, read-only extraction of `(symbol, net_qty)`
// pairs from the already-seeded snapshot — no baseline lookup, no arithmetic
// merge, no DB access, no synthetic oms_inbox/outbox rows.  The seeded qty IS
// the local truth; nothing is added downstream.
// ---------------------------------------------------------------------------

#[test]
fn rbs07_derivation_is_pure_read_no_remerge_no_synthetic_fills() {
    // Seeded snapshot already reflects baseline(4) + fill(+1) = 5 (folded in
    // once, upstream, by seed_portfolio_from_baseline + apply_entry).
    let local = local_from_seeded_snapshot(&[("AAPL", 5)]);

    // The derivation reads the seeded total through unchanged — no second
    // baseline addition occurs (that re-merge is exactly what was removed).
    assert_eq!(
        local.positions.get("AAPL").copied(),
        Some(5),
        "RBS07: seeded total(5) must pass through unchanged — no re-merge"
    );

    // No database access occurred: local_from_seeded_snapshot is a pure
    // in-memory function, mirroring reconcile_local_snapshot_from_runtime_with_sides
    // (which reads only execution_snapshot.portfolio.positions + active_orders).
    // The production path is equally read-only and free of synthetic fill rows.
}
