//! DATA-INTEGRITY-01 — Multi-cycle data integrity proof.
//!
//! # Invariants under test
//!
//! DI-01: Repeated replay/restart cycles do not create extra OMS/portfolio
//!        effects for already-applied broker events.
//!
//! DI-02: Duplicate fills, late fills, and reordered partial-fill sequences
//!        converge to exactly one correct OMS and portfolio truth.
//!
//! DI-03: Long-horizon reconcile drift is never silently accepted; any dirty
//!        tick prescribes HaltAndDisarm regardless of how many clean ticks
//!        preceded it.
//!
//! DI-04: Durable inbox lifecycle (applied_at_utc stamping) and portfolio
//!        state remain aligned across repeated cycle boundaries (D2
//!        crash-recovery contract).
//!
//! All scenarios are pure in-process; no DB, no network, no wall-clock reads.
//! These tests prove that cross-cutting data-integrity invariants hold at
//! the boundary where inbox / OMS / portfolio / reconcile meet.

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder, OrderState};
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side, MICROS_SCALE};
use mqk_reconcile::{reconcile_tick, BrokerSnapshot, DriftAction, LocalSnapshot};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_portfolio() -> PortfolioState {
    PortfolioState::new(0)
}

fn local_with_pos(symbol: &str, qty: i64) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn broker_with_pos(symbol: &str, qty: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

/// Simulated inbox dedupe: returns true on first insert, false on duplicates.
/// Mirrors inbox_insert_deduped's contract without a live DB.
fn inbox_insert_sim(seen: &mut HashSet<String>, msg_id: &str) -> bool {
    seen.insert(msg_id.to_string())
}

// ---------------------------------------------------------------------------
// DI-01: replay_after_restart_does_not_duplicate_durable_effects
//
// A realistic event sequence is applied once, then replayed 10 times
// (simulating 10 restart cycles).  OMS state and portfolio effects must
// equal the single-pass result after every replay cycle.
// ---------------------------------------------------------------------------

#[test]
fn replay_after_restart_does_not_duplicate_durable_effects() {
    // Fixed event sequence: partial fill x2, then final fill.
    let events: &[(&str, OmsEvent, Fill)] = &[
        (
            "fill-pf-1",
            OmsEvent::PartialFill { delta_qty: 30 },
            Fill::new("SPY", Side::Buy, 30, 500 * MICROS_SCALE, 0),
        ),
        (
            "fill-pf-2",
            OmsEvent::PartialFill { delta_qty: 40 },
            Fill::new("SPY", Side::Buy, 40, 501 * MICROS_SCALE, 0),
        ),
        (
            "fill-final",
            OmsEvent::Fill { delta_qty: 30 },
            Fill::new("SPY", Side::Buy, 30, 502 * MICROS_SCALE, 0),
        ),
    ];

    let mut order = OmsOrder::new("ord-di01", "SPY", 100);
    let mut portfolio = empty_portfolio();
    // Inbox dedupe state persists across restarts — mirrors durable DB rows.
    let mut seen: HashSet<String> = HashSet::new();

    // Single-pass: apply each event exactly once.
    for (msg_id, oms_ev, fill) in events {
        if inbox_insert_sim(&mut seen, msg_id) {
            order.apply(oms_ev, Some(*msg_id)).unwrap();
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill.clone()));
        }
    }

    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, 100);
    let spy_qty = portfolio
        .positions
        .get("SPY")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(spy_qty, 100, "single-pass: SPY must be 100");

    // 10 restart cycles: replay all events with the same seen set.
    // All inserts must return false (already in inbox) → zero new OMS/portfolio effects.
    for cycle in 1..=10 {
        for (msg_id, oms_ev, fill) in events {
            let inserted = inbox_insert_sim(&mut seen, msg_id);
            assert!(
                !inserted,
                "cycle {cycle}: msg_id {msg_id} must dedupe on restart replay"
            );
            if inserted {
                // Must never execute — branch is here to make the guard explicit.
                order.apply(oms_ev, Some(*msg_id)).unwrap();
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill.clone()));
            }
        }

        assert_eq!(
            order.state,
            OrderState::Filled,
            "cycle {cycle}: OMS state must remain Filled"
        );
        assert_eq!(
            order.filled_qty, 100,
            "cycle {cycle}: filled_qty must remain 100 after restart replay"
        );
        let qty = portfolio
            .positions
            .get("SPY")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(
            qty, 100,
            "cycle {cycle}: portfolio SPY qty must remain 100 after restart replay"
        );
    }
}

// ---------------------------------------------------------------------------
// DI-02: duplicate_and_late_event_sequences_preserve_single_truth
//
// Duplicate fills, late fills, and reordered partial-fill sequences all
// converge to exactly one correct OMS and portfolio truth.
// ---------------------------------------------------------------------------

#[test]
fn duplicate_and_late_event_sequences_preserve_single_truth() {
    let total_qty = 100i64;
    let mut order = OmsOrder::new("ord-di02", "QQQ", total_qty);
    let mut portfolio = empty_portfolio();
    let mut seen: HashSet<String> = HashSet::new();

    // ---- Normal forward sequence ----

    if inbox_insert_sim(&mut seen, "pf-1") {
        order
            .apply(&OmsEvent::PartialFill { delta_qty: 30 }, Some("pf-1"))
            .unwrap();
        apply_entry(
            &mut portfolio,
            LedgerEntry::Fill(Fill::new("QQQ", Side::Buy, 30, 400 * MICROS_SCALE, 0)),
        );
    }

    if inbox_insert_sim(&mut seen, "pf-2") {
        order
            .apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("pf-2"))
            .unwrap();
        apply_entry(
            &mut portfolio,
            LedgerEntry::Fill(Fill::new("QQQ", Side::Buy, 40, 401 * MICROS_SCALE, 0)),
        );
    }

    if inbox_insert_sim(&mut seen, "fill-final") {
        order
            .apply(&OmsEvent::Fill { delta_qty: 30 }, Some("fill-final"))
            .unwrap();
        apply_entry(
            &mut portfolio,
            LedgerEntry::Fill(Fill::new("QQQ", Side::Buy, 30, 402 * MICROS_SCALE, 0)),
        );
    }

    // After normal sequence: fully filled.
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, total_qty);
    let qqq_qty = portfolio
        .positions
        .get("QQQ")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(
        qqq_qty, total_qty,
        "portfolio must hold exactly total_qty after normal sequence"
    );

    // ---- Duplicate deliveries of already-processed events ----

    // Duplicate pf-1: inbox dedupe must block it.
    assert!(
        !inbox_insert_sim(&mut seen, "pf-1"),
        "duplicate pf-1 must be blocked by inbox dedupe"
    );

    // Duplicate fill-final: inbox dedupe must block it.
    assert!(
        !inbox_insert_sim(&mut seen, "fill-final"),
        "duplicate fill-final must be blocked by inbox dedupe"
    );

    // ---- Late fill: new msg_id, but order is already in terminal state ----
    //
    // Inbox would accept a new msg_id, but OMS state machine silently ignores
    // fills on a Filled order.  Portfolio apply is gated on OMS acceptance —
    // the late fill produces no portfolio effect.
    // This is the OMS ↔ portfolio alignment invariant:
    //   OMS terminal-state no-op ⇒ portfolio is not called.
    if inbox_insert_sim(&mut seen, "fill-late") {
        // OMS is already Filled — do_transition silently ignores this (no Err).
        order
            .apply(&OmsEvent::Fill { delta_qty: 30 }, Some("fill-late"))
            .unwrap();
        // State and filled_qty must be unchanged.
        assert_eq!(
            order.state,
            OrderState::Filled,
            "late fill must not change OMS state"
        );
        assert_eq!(
            order.filled_qty, total_qty,
            "late fill must not change filled_qty"
        );
        // In the real orchestrator, the OMS no-op prevents the portfolio apply.
        // Here we model that gating explicitly: do NOT call apply_entry.
    }

    // After all duplicates and late events: state and portfolio are unchanged.
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, total_qty);
    let qqq_qty_final = portfolio
        .positions
        .get("QQQ")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(
        qqq_qty_final, total_qty,
        "portfolio QQQ qty must remain total_qty after all duplicates and late events"
    );
}

// ---------------------------------------------------------------------------
// DI-03: reconcile_detects_long_horizon_divergence_without_silent_acceptance
//
// Long-horizon drift is never silently accepted; any dirty tick prescribes
// HaltAndDisarm regardless of how many clean ticks preceded it.
// ---------------------------------------------------------------------------

#[test]
fn reconcile_detects_long_horizon_divergence_without_silent_acceptance() {
    let local = local_with_pos("SPY", 100);
    let clean_broker = broker_with_pos("SPY", 100);
    let dirty_broker_1 = broker_with_pos("SPY", 50);
    let dirty_broker_2 = broker_with_pos("SPY", 0);

    // Phase 1: 100 clean ticks — all Continue.
    for i in 0..100 {
        assert_eq!(
            reconcile_tick(&local, &clean_broker),
            DriftAction::Continue,
            "clean tick #{i} must return Continue"
        );
    }

    // Drift at tick 100: must immediately prescribe halt.
    assert!(
        reconcile_tick(&local, &dirty_broker_1).requires_halt_and_disarm(),
        "first drift at tick 100 must prescribe HaltAndDisarm \
         (100 clean ticks offer no immunity)"
    );

    // Phase 2: 50 recovery clean ticks — all Continue.
    for i in 0..50 {
        assert_eq!(
            reconcile_tick(&local, &clean_broker),
            DriftAction::Continue,
            "recovery clean tick #{i} must return Continue"
        );
    }

    // Drift at tick 151: must immediately prescribe halt again.
    assert!(
        reconcile_tick(&local, &dirty_broker_2).requires_halt_and_disarm(),
        "second drift at tick 151 must prescribe HaltAndDisarm \
         (50 prior clean ticks offer no immunity)"
    );

    // Off-by-one drift: reconciler has no "close enough" tolerance.
    assert!(
        reconcile_tick(&local, &broker_with_pos("SPY", 99)).requires_halt_and_disarm(),
        "off-by-one drift must prescribe HaltAndDisarm: reconciler has no tolerance band"
    );

    // Multi-symbol: a broker position unknown to local also halts.
    let mut dirty_unknown = BrokerSnapshot::empty();
    dirty_unknown.positions.insert("SPY".to_string(), 100);
    dirty_unknown.positions.insert("QQQ".to_string(), 5);
    assert!(
        reconcile_tick(&local, &dirty_unknown).requires_halt_and_disarm(),
        "broker position unknown to local must prescribe HaltAndDisarm"
    );
}

// ---------------------------------------------------------------------------
// DI-04: durable_inbox_lifecycle_and_portfolio_effects_remain_aligned
//
// Simulates the D2 crash-recovery contract:
//   1. Normal path: insert → apply → mark_applied
//   2. Crash simulation: portfolio reset, unapplied rows replayed
//   3. Portfolio converges to the same state whether it ran live or was
//      recovered from unapplied inbox rows.
//
// Uses a pure in-memory inbox simulation that mirrors the DB contract.
// ---------------------------------------------------------------------------

/// Simulated inbox entry — mirrors the DB oms_inbox row concept.
#[derive(Debug, Clone)]
struct SimInboxEntry {
    msg_id: String,
    fill: Fill,
    applied: bool,
}

/// Simulate the D2 inbox contract.
///
/// `crash_after_apply_idx`: if Some(n), the simulation crashes after event n
/// has been inserted to the inbox.  Events 0..=n are inserted AND applied;
/// events n+1.. are inserted but NOT applied (simulating a crash between
/// inbox_insert and inbox_mark_applied).
///
/// After the crash the portfolio is reset and recovered from the inbox rows
/// (applied + unapplied), exactly as inbox_load_all_applied_for_run +
/// inbox_load_unapplied_for_run would do on restart.
fn run_d2_sim(
    events: &[(&str, Fill)],
    crash_after_apply_idx: Option<usize>,
) -> (PortfolioState, usize, usize) {
    let mut inbox: Vec<SimInboxEntry> = Vec::new();
    let mut portfolio = empty_portfolio();

    // Normal ingest path.
    for (i, (msg_id, fill)) in events.iter().enumerate() {
        // Dedupe.
        if inbox.iter().any(|e| e.msg_id == *msg_id) {
            continue;
        }
        inbox.push(SimInboxEntry {
            msg_id: msg_id.to_string(),
            fill: fill.clone(),
            applied: false,
        });

        // Crash gate: events after crash_after_apply_idx are inserted but not applied.
        if let Some(crash) = crash_after_apply_idx {
            if i > crash {
                // Inserted to inbox, NOT applied, NOT marked.
                continue;
            }
        }

        apply_entry(&mut portfolio, LedgerEntry::Fill(fill.clone()));
        inbox.last_mut().unwrap().applied = true;
    }

    // Crash recovery: reset in-process portfolio, replay from durable inbox.
    if crash_after_apply_idx.is_some() {
        portfolio = empty_portfolio();

        // Replay ALL inbox rows ordered by insertion position (inbox_id asc).
        // This mirrors the startup recovery sequence:
        //   1. inbox_load_all_applied_for_run  (already marked — replay for state)
        //   2. inbox_load_unapplied_for_run     (not yet marked — complete apply)
        // Both sets are applied in one pass here; in production they use the
        // same apply_fill path in canonical (inbox_id asc) order.
        for entry in &mut inbox {
            apply_entry(&mut portfolio, LedgerEntry::Fill(entry.fill.clone()));
            entry.applied = true;
        }
    }

    let applied = inbox.iter().filter(|e| e.applied).count();
    let unapplied = inbox.iter().filter(|e| !e.applied).count();
    (portfolio, applied, unapplied)
}

#[test]
fn durable_inbox_lifecycle_and_portfolio_effects_remain_aligned() {
    let events: &[(&str, Fill)] = &[
        (
            "ev-1",
            Fill::new("AAPL", Side::Buy, 10, 150 * MICROS_SCALE, 0),
        ),
        (
            "ev-2",
            Fill::new("AAPL", Side::Buy, 20, 151 * MICROS_SCALE, 0),
        ),
        (
            "ev-3",
            Fill::new("AAPL", Side::Buy, 30, 152 * MICROS_SCALE, 0),
        ),
        (
            "ev-4",
            Fill::new("MSFT", Side::Buy, 5, 300 * MICROS_SCALE, 0),
        ),
    ];

    // Scenario A: clean run, no crash.
    let (clean_pf, clean_applied, clean_unapplied) = run_d2_sim(events, None);
    let clean_aapl = clean_pf
        .positions
        .get("AAPL")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    let clean_msft = clean_pf
        .positions
        .get("MSFT")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(clean_aapl, 60, "clean run: AAPL must be 60");
    assert_eq!(clean_msft, 5, "clean run: MSFT must be 5");
    assert_eq!(clean_applied, 4, "clean run: all 4 events must be applied");
    assert_eq!(clean_unapplied, 0, "clean run: no unapplied events");

    // Scenario B: crash after ev-2 (index 1).
    // ev-1 and ev-2 applied; ev-3 and ev-4 inserted but not applied.
    let (crash_b_pf, crash_b_applied, crash_b_unapplied) = run_d2_sim(events, Some(1));
    let crash_b_aapl = crash_b_pf
        .positions
        .get("AAPL")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    let crash_b_msft = crash_b_pf
        .positions
        .get("MSFT")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(
        crash_b_aapl, clean_aapl,
        "crash-after-ev2: AAPL must equal clean-run after D2 recovery"
    );
    assert_eq!(
        crash_b_msft, clean_msft,
        "crash-after-ev2: MSFT must equal clean-run after D2 recovery"
    );
    assert_eq!(
        crash_b_applied, 4,
        "crash-after-ev2: all 4 events applied after recovery"
    );
    assert_eq!(
        crash_b_unapplied, 0,
        "crash-after-ev2: no unapplied events after recovery"
    );

    // Scenario C: crash after ev-1 (index 0).
    // ev-1 applied; ev-2, ev-3, ev-4 inserted but not applied.
    let (crash_c_pf, crash_c_applied, crash_c_unapplied) = run_d2_sim(events, Some(0));
    let crash_c_aapl = crash_c_pf
        .positions
        .get("AAPL")
        .map(|p| p.qty_signed())
        .unwrap_or(0);
    assert_eq!(
        crash_c_aapl, clean_aapl,
        "crash-after-ev1: AAPL must equal clean-run after D2 recovery"
    );
    assert_eq!(crash_c_applied, 4);
    assert_eq!(crash_c_unapplied, 0);

    // Alignment invariant: after recovery, portfolio reflects exactly all
    // events in the inbox — no more, no less, regardless of crash timing.
    assert_eq!(
        crash_b_aapl, 60,
        "alignment: AAPL qty must be exactly 60 after crash-B recovery"
    );
    assert_eq!(
        crash_c_aapl, 60,
        "alignment: AAPL qty must be exactly 60 after crash-C recovery"
    );
}
