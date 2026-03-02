//! Scenario: Duplicate / Out-of-Order Event Harness — I9-2
//!
//! # Invariant under test
//!
//! Perturbed event sequences (duplicate ACK, duplicate FILL, FILL before ACK,
//! stale CancelAck after full fill) produce final OMS + portfolio state that is
//! identical to the canonical ordering.
//!
//! All scenarios are pure in-memory: no DB, no IO, no wall-clock time,
//! no randomness.
//!
//! ## S1 — Duplicate ACK: same final OMS state
//!
//! Canonical:  [Ack(e1), Fill(e2)]
//! Perturbed:  [Ack(e1), Ack(e1-dup), Fill(e2)]
//! Assert: OrderState::Filled, filled_qty == 100 on both.
//!
//! ## S2 — Duplicate FILL: same final OMS + portfolio state
//!
//! OMS event_id dedup silently skips the second Fill(f1) at the OMS layer.
//! Ledger seq_no guard rejects the duplicate append and leaves portfolio state
//! completely unchanged.
//! Assert: canonical snapshot == perturbed snapshot.
//!
//! ## S3 — FILL before ACK: same final OMS state
//!
//! Canonical:  [Ack(a1), Fill(f1)]
//! Perturbed:  [Fill(f1)]  (no prior Ack)
//! Fill is accepted from the Open state; both paths terminate in Filled.
//! Assert: OrderState::Filled, filled_qty == 100 on both.
//!
//! ## S4 — Stale CancelAck after full fill: state unchanged
//!
//! Sequence:   [CancelRequest, Fill(f1)]  →  Filled (terminal)
//! Inject:     CancelAck (stale)          →  TransitionError, state unchanged
//! Assert:     perturbed final state == canonical final state.

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder, OrderState};
use mqk_portfolio::{Fill, Ledger, Side, MICROS_SCALE};

// ── Constants ────────────────────────────────────────────────────────────────

const INITIAL_CASH: i64 = 100_000 * MICROS_SCALE; // $100,000
const SPY_PRICE: i64 = 100 * MICROS_SCALE; // $100.00

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Assert that two OMS orders have identical observable state.
///
/// Compares `state` (lifecycle) and `filled_qty` (fill accounting) only;
/// the internal `applied` set is intentionally excluded — it may differ
/// between canonical and perturbed paths due to extra duplicate event_ids.
fn assert_oms_state_eq(canonical: &OmsOrder, perturbed: &OmsOrder, label: &str) {
    assert_eq!(
        canonical.state, perturbed.state,
        "{label}: OMS state differs — canonical={:?} perturbed={:?}",
        canonical.state, perturbed.state
    );
    assert_eq!(
        canonical.filled_qty, perturbed.filled_qty,
        "{label}: filled_qty differs — canonical={} perturbed={}",
        canonical.filled_qty, perturbed.filled_qty
    );
}

// ── S1: Duplicate ACK ────────────────────────────────────────────────────────

/// Canonical [Ack, Fill] and perturbed [Ack, Ack(dup), Fill] must produce
/// identical final OMS state.
#[test]
fn duplicate_ack_produces_same_final_oms_state() {
    // Canonical: Ack → Fill
    let mut canonical = OmsOrder::new("ord-1", "SPY", 100);
    canonical.apply(&OmsEvent::Ack, Some("e1")).unwrap();
    canonical
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("e2"))
        .unwrap();

    assert_eq!(
        canonical.state,
        OrderState::Filled,
        "S1: canonical must be Filled"
    );
    assert_eq!(
        canonical.filled_qty, 100,
        "S1: canonical filled_qty must be 100"
    );

    // Perturbed: Ack → Ack (duplicate, same event_id) → Fill
    let mut perturbed = OmsOrder::new("ord-2", "SPY", 100);
    perturbed.apply(&OmsEvent::Ack, Some("e1")).unwrap();
    perturbed.apply(&OmsEvent::Ack, Some("e1")).unwrap(); // duplicate — idempotent skip
    perturbed
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("e2"))
        .unwrap();

    assert_oms_state_eq(&canonical, &perturbed, "S1: duplicate Ack");
}

// ── S2: Duplicate FILL ───────────────────────────────────────────────────────

/// Duplicate fill deduplication at both the OMS layer (event_id) and the
/// portfolio layer (seq_no guard).  Both canonical and perturbed paths must
/// converge to the same OMS observable state and the same LedgerSnapshot.
#[test]
fn duplicate_fill_produces_same_oms_and_portfolio_state() {
    let fill = Fill::new("SPY", Side::Buy, 50, SPY_PRICE, 0);

    // ── OMS layer ────────────────────────────────────────────────────────────

    // Canonical OMS: single Fill(f1)
    let mut oms_canonical = OmsOrder::new("ord-1", "SPY", 50);
    oms_canonical
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("f1"))
        .unwrap();

    // Perturbed OMS: Fill(f1) then Fill(f1) again — second call is a silent skip
    let mut oms_perturbed = OmsOrder::new("ord-2", "SPY", 50);
    oms_perturbed
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("f1"))
        .unwrap();
    oms_perturbed
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("f1")) // duplicate event_id — skipped
        .unwrap();

    assert_oms_state_eq(
        &oms_canonical,
        &oms_perturbed,
        "S2: OMS layer duplicate fill",
    );

    // ── Portfolio (Ledger) layer ──────────────────────────────────────────────

    // Canonical ledger: one fill appended at seq_no=1
    let mut ledger_canonical = Ledger::new(INITIAL_CASH);
    ledger_canonical
        .append_fill_seq(fill.clone(), 1)
        .expect("S2: canonical seq_no=1 must succeed");
    let snap_canonical = ledger_canonical.snapshot();

    // Perturbed ledger: fill at seq_no=1, then duplicate seq_no=1 rejected
    let mut ledger_perturbed = Ledger::new(INITIAL_CASH);
    ledger_perturbed
        .append_fill_seq(fill.clone(), 1)
        .expect("S2: perturbed seq_no=1 must succeed");
    let dup_result = ledger_perturbed.append_fill_seq(fill.clone(), 1);
    assert!(
        dup_result.is_err(),
        "S2: duplicate seq_no=1 must be rejected, got Ok"
    );
    let snap_perturbed = ledger_perturbed.snapshot();

    assert_eq!(
        snap_canonical, snap_perturbed,
        "S2: LedgerSnapshot must be identical after rejected duplicate fill"
    );
}

// ── S3: FILL before ACK ──────────────────────────────────────────────────────

/// A Fill arriving before an Ack must be accepted (Open → Filled) and produce
/// the same final OMS state as the canonical [Ack, Fill] sequence.
#[test]
fn fill_before_ack_produces_same_final_oms_state() {
    // Canonical: Ack → Fill
    let mut canonical = OmsOrder::new("ord-1", "SPY", 100);
    canonical.apply(&OmsEvent::Ack, Some("a1")).unwrap();
    canonical
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
        .unwrap();

    // Perturbed: Fill only (no prior Ack — accepted from Open state)
    let mut perturbed = OmsOrder::new("ord-2", "SPY", 100);
    perturbed
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
        .unwrap();

    assert_oms_state_eq(&canonical, &perturbed, "S3: fill before ack");
}

// ── S4: Stale CancelAck after full fill ──────────────────────────────────────

/// After [CancelRequest, Fill] the order is Filled (terminal).  A stale
/// CancelAck must return TransitionError and leave state completely unchanged.
/// The perturbed order must equal the canonical order that never received the
/// stale event.
#[test]
fn stale_cancel_ack_after_full_fill_leaves_state_unchanged() {
    // Canonical: CancelRequest → Fill → Filled (terminal)
    let mut canonical = OmsOrder::new("ord-1", "SPY", 100);
    canonical
        .apply(&OmsEvent::CancelRequest, Some("c1"))
        .unwrap();
    canonical
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
        .unwrap();

    assert_eq!(
        canonical.state,
        OrderState::Filled,
        "S4: canonical must be Filled"
    );
    assert_eq!(
        canonical.filled_qty, 100,
        "S4: canonical filled_qty must be 100"
    );

    // Perturbed: same canonical sequence ...
    let mut perturbed = OmsOrder::new("ord-2", "SPY", 100);
    perturbed
        .apply(&OmsEvent::CancelRequest, Some("c1"))
        .unwrap();
    perturbed
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
        .unwrap();

    // ... then stale CancelAck arrives — must fail, state must not change.
    let state_before = perturbed.state.clone();
    let qty_before = perturbed.filled_qty;

    let stale_result = perturbed.apply(&OmsEvent::CancelAck, Some("c-ack-stale"));
    assert!(
        stale_result.is_err(),
        "S4: stale CancelAck on Filled order must return TransitionError, got Ok"
    );

    assert_eq!(
        perturbed.state, state_before,
        "S4: state must be unchanged after rejected stale CancelAck"
    );
    assert_eq!(
        perturbed.filled_qty, qty_before,
        "S4: filled_qty must be unchanged after rejected stale CancelAck"
    );

    assert_oms_state_eq(
        &canonical,
        &perturbed,
        "S4: stale CancelAck after full fill",
    );
}
