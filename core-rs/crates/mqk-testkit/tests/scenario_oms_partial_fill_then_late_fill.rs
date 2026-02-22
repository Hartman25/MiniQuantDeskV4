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

// ---------------------------------------------------------------------------
// 1. Partial fills then final fill
// ---------------------------------------------------------------------------

#[test]
fn three_partial_fills_then_final_fill_completes_order() {
    let mut order = OmsOrder::new("ord-1", "SPY", 100);

    order
        .apply(&OmsEvent::PartialFill { delta_qty: 30 }, Some("f1"))
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);
    assert_eq!(order.filled_qty, 30);

    order
        .apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f2"))
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);
    assert_eq!(order.filled_qty, 70);

    // Final fill for the remaining 30 lots.
    order
        .apply(&OmsEvent::Fill { delta_qty: 30 }, Some("f3"))
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, 100);
    assert!(order.state.is_terminal());
}

// ---------------------------------------------------------------------------
// 2. Late/duplicate fill on already-Filled order: idempotent by state
// ---------------------------------------------------------------------------

#[test]
fn late_fill_on_filled_order_does_not_double_apply() {
    let mut order = OmsOrder::new("ord-2", "AAPL", 50);

    order
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("fill-1"))
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, 50);

    // Same event_id → idempotent by event_id dedup.
    order
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("fill-1"))
        .unwrap();
    assert_eq!(
        order.filled_qty, 50,
        "duplicate event_id must not re-apply the fill"
    );

    // Different event_id but state is Filled → idempotent by state (late fill no-op).
    order
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("fill-late"))
        .unwrap();
    assert_eq!(
        order.filled_qty, 50,
        "late fill on already-Filled order must be a no-op"
    );
    assert_eq!(order.state, OrderState::Filled);
}

// ---------------------------------------------------------------------------
// 3. Idempotent replay: same event_id applied twice → no double effect
// ---------------------------------------------------------------------------

#[test]
fn idempotent_replay_does_not_double_apply_partial_fill() {
    let mut order = OmsOrder::new("ord-3", "QQQ", 100);

    order
        .apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("ev-1"))
        .unwrap();
    assert_eq!(order.filled_qty, 40);
    assert_eq!(order.state, OrderState::PartiallyFilled);

    // Replayed event with the SAME event_id — must be a silent no-op.
    order
        .apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("ev-1"))
        .unwrap();
    assert_eq!(
        order.filled_qty, 40,
        "replayed event must not re-accumulate filled_qty"
    );
    assert_eq!(order.state, OrderState::PartiallyFilled);
}

#[test]
fn idempotent_replay_across_multiple_events() {
    let mut order = OmsOrder::new("ord-replay", "TSLA", 200);

    let events = vec![
        (OmsEvent::PartialFill { delta_qty: 50 }, "e1"),
        (OmsEvent::PartialFill { delta_qty: 50 }, "e2"),
        (OmsEvent::Fill { delta_qty: 100 }, "e3"),
    ];

    // Apply once.
    for (ev, id) in &events {
        order.apply(ev, Some(id)).unwrap();
    }
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, 200);

    // Replay all events — state and qty must be unchanged.
    for (ev, id) in &events {
        order.apply(ev, Some(id)).unwrap();
    }
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(
        order.filled_qty, 200,
        "full replay must produce the same final state"
    );
}

// ---------------------------------------------------------------------------
// 4. Replace semantics: request vs. broker ack
// ---------------------------------------------------------------------------

#[test]
fn replace_request_puts_order_in_replace_pending_not_confirmed() {
    let mut order = OmsOrder::new("ord-4", "TSLA", 10);

    // Application sends a replace request.
    order.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
    assert_eq!(
        order.state,
        OrderState::ReplacePending,
        "replace request must move order to ReplacePending (not yet confirmed)"
    );

    // Broker acknowledges the replace — order is live again.
    order.apply(&OmsEvent::ReplaceAck, Some("r2")).unwrap();
    assert_eq!(
        order.state,
        OrderState::Open,
        "replace ack must restore order to Open"
    );
}

#[test]
fn replace_reject_restores_prior_live_state() {
    let mut order = OmsOrder::new("ord-5", "NVDA", 20);

    // Partial fill before the replace attempt.
    order
        .apply(&OmsEvent::PartialFill { delta_qty: 5 }, Some("f1"))
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
        order.filled_qty, 5,
        "filled_qty must be unchanged after replace reject"
    );
}

// ---------------------------------------------------------------------------
// 5. Fill during ReplacePending still completes the order
// ---------------------------------------------------------------------------

#[test]
fn fill_during_replace_pending_completes_order() {
    let mut order = OmsOrder::new("ord-6", "GLD", 50);

    order.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
    assert_eq!(order.state, OrderState::ReplacePending);

    // Fill arrives before replace is processed.
    order
        .apply(&OmsEvent::Fill { delta_qty: 50 }, Some("f1"))
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::Filled,
        "order must be Filled even when replace was pending"
    );
    assert_eq!(order.filled_qty, 50);
}
