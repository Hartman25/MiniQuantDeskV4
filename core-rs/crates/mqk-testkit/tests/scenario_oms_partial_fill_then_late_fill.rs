//! Scenario: OMS Partial Fill then Late Fill — Patch L4
//!
//! # Invariants under test
//!
//! 1. Consecutive `PartialFill` events accumulate `filled_qty` correctly.
//! 2. A final `Fill` event after partial fills transitions to `Filled`.
//! 3. A late/duplicate `Fill` on an already-`Filled` order is **idempotent**
//!    (no double-apply; `filled_qty` does not exceed original `total_qty`).
//! 4. **Replace semantics**: `ReplaceRequest` → `ReplacePending` (NOT yet
//!    confirmed); `ReplaceAck` confirms and restores the order to `Open` or
//!    `PartiallyFilled`; `ReplaceReject` reverts the same way.
//! 5. **Idempotent replay**: applying the same `event_id` a second time is
//!    a silent no-op — `filled_qty` and `state` remain unchanged.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder, OrderState};
use mqk_execution::QtyMicros;

// ---------------------------------------------------------------------------
// 1. Partial fills then final fill
// ---------------------------------------------------------------------------

#[test]
fn three_partial_fills_then_final_fill_completes_order() {
    let mut order = OmsOrder::new("ord-1", "SPY", QtyMicros::from_whole_units(100).unwrap());

    order
        .apply(&OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(30).unwrap() }, Some("f1"))
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(30).unwrap());

    order
        .apply(&OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(40).unwrap() }, Some("f2"))
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(70).unwrap());

    // Final fill for the remaining 30 lots.
    order
        .apply(&OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(30).unwrap() }, Some("f3"))
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(100).unwrap());
    assert!(order.state.is_terminal());
}

// ---------------------------------------------------------------------------
// 2. Late/duplicate fill on already-Filled order: idempotent by state
// ---------------------------------------------------------------------------

#[test]
fn late_fill_on_filled_order_does_not_double_apply() {
    let mut order = OmsOrder::new("ord-2", "AAPL", QtyMicros::from_whole_units(50).unwrap());

    order
        .apply(&OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, Some("fill-1"))
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(50).unwrap());

    // Same event_id → idempotent by event_id dedup.
    order
        .apply(&OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, Some("fill-1"))
        .unwrap();
    assert_eq!(
        order.filled_qty, QtyMicros::from_whole_units(50).unwrap(),
        "duplicate event_id must not re-apply the fill"
    );

    // Different event_id but state is Filled → idempotent by state (late fill no-op).
    order
        .apply(&OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, Some("fill-late"))
        .unwrap();
    assert_eq!(
        order.filled_qty, QtyMicros::from_whole_units(50).unwrap(),
        "late fill on already-Filled order must be a no-op"
    );
    assert_eq!(order.state, OrderState::Filled);
}

// ---------------------------------------------------------------------------
// 3. Idempotent replay: same event_id applied twice → no double effect
// ---------------------------------------------------------------------------

#[test]
fn idempotent_replay_does_not_double_apply_partial_fill() {
    let mut order = OmsOrder::new("ord-3", "QQQ", QtyMicros::from_whole_units(100).unwrap());

    order
        .apply(&OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(40).unwrap() }, Some("ev-1"))
        .unwrap();
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(40).unwrap());
    assert_eq!(order.state, OrderState::PartiallyFilled);

    // Replayed event with the SAME event_id — must be a silent no-op.
    order
        .apply(&OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(40).unwrap() }, Some("ev-1"))
        .unwrap();
    assert_eq!(
        order.filled_qty, QtyMicros::from_whole_units(40).unwrap(),
        "replayed event must not re-accumulate filled_qty"
    );
    assert_eq!(order.state, OrderState::PartiallyFilled);
}

#[test]
fn idempotent_replay_across_multiple_events() {
    let mut order = OmsOrder::new("ord-replay", "TSLA", QtyMicros::from_whole_units(200).unwrap());

    let events = vec![
        (OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, "e1"),
        (OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, "e2"),
        (OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(100).unwrap() }, "e3"),
    ];

    // Apply once.
    for (ev, id) in &events {
        order.apply(ev, Some(id)).unwrap();
    }
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(200).unwrap());

    // Replay all events — state and qty must be unchanged.
    for (ev, id) in &events {
        order.apply(ev, Some(id)).unwrap();
    }
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(
        order.filled_qty, QtyMicros::from_whole_units(200).unwrap(),
        "full replay must produce the same final state"
    );
}

// ---------------------------------------------------------------------------
// 4. Replace semantics: request vs. broker ack
// ---------------------------------------------------------------------------

#[test]
fn replace_request_puts_order_in_replace_pending_not_confirmed() {
    let mut order = OmsOrder::new("ord-4", "TSLA", QtyMicros::from_whole_units(10).unwrap());

    // Application sends a replace request.
    order.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
    assert_eq!(
        order.state,
        OrderState::ReplacePending,
        "replace request must move order to ReplacePending (not yet confirmed)"
    );

    // Broker acknowledges the replace — order is live again.
    // P1-03: ReplaceAck carries new_total_qty. Order has no fills, total=10.
    order
        .apply(&OmsEvent::ReplaceAck { new_total_qty: QtyMicros::from_whole_units(10).unwrap() }, Some("r2"))
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::Open,
        "replace ack must restore order to Open"
    );
}

#[test]
fn replace_reject_restores_prior_live_state() {
    let mut order = OmsOrder::new("ord-5", "NVDA", QtyMicros::from_whole_units(20).unwrap());

    // Partial fill before the replace attempt.
    order
        .apply(&OmsEvent::PartialFill { delta_qty: QtyMicros::from_whole_units(5).unwrap() }, Some("f1"))
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);

    // Replace request sent.
    order.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
    assert_eq!(order.state, OrderState::ReplacePending);

    // Broker rejects the replace → revert to PartiallyFilled.
    order.apply(&OmsEvent::ReplaceReject, Some("r2")).unwrap();
    assert_eq!(
        order.state,
        OrderState::PartiallyFilled,
        "replace reject must restore PartiallyFilled when partial fills exist"
    );
    assert_eq!(
        order.filled_qty, QtyMicros::from_whole_units(5).unwrap(),
        "filled_qty must be unchanged after replace reject"
    );
}

// ---------------------------------------------------------------------------
// 5. Fill during ReplacePending still completes the order
// ---------------------------------------------------------------------------

#[test]
fn fill_during_replace_pending_completes_order() {
    let mut order = OmsOrder::new("ord-6", "GLD", QtyMicros::from_whole_units(50).unwrap());

    order.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
    assert_eq!(order.state, OrderState::ReplacePending);

    // Fill arrives before replace is processed.
    order
        .apply(&OmsEvent::Fill { delta_qty: QtyMicros::from_whole_units(50).unwrap() }, Some("f1"))
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::Filled,
        "order must be Filled even when replace was pending"
    );
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(50).unwrap());
}
