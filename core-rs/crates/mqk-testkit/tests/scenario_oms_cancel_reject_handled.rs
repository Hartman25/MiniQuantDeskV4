//! Scenario: OMS Cancel Reject Handled — Patch L4
//!
//! # Invariant under test
//! A cancel request is NOT a terminal state.  If the broker rejects the
//! cancel, the order returns to its prior live state (`Open` or
//! `PartiallyFilled`) and remains fully active — it can still receive fills.
//!
//! Key principle: "cancel is a request, not a terminal state."
//!
//! All tests are pure in-process; no DB or network required.

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder, OrderState, TransitionError};
use mqk_execution::QtyMicros;

// ---------------------------------------------------------------------------
// Cancel-reject restores Open state
// ---------------------------------------------------------------------------

#[test]
fn cancel_reject_on_open_order_restores_open() {
    let mut order = OmsOrder::new("ord-1", "SPY", QtyMicros::from_whole_units(100).unwrap());
    assert_eq!(order.state, OrderState::Open);

    // Application sends a cancel request.
    order
        .apply(&OmsEvent::CancelRequest, Some("ev-cancel-req"))
        .unwrap();
    assert_eq!(order.state, OrderState::CancelPending);

    // Broker rejects the cancel — order must revert to Open.
    order
        .apply(&OmsEvent::CancelReject, Some("ev-cancel-rej"))
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::Open,
        "cancel-reject must restore Open state when no fills have occurred"
    );

    // Order is still fully alive and can be filled.
    order
        .apply(
            &OmsEvent::Fill {
                delta_qty: QtyMicros::from_whole_units(100).unwrap(),
            },
            Some("ev-fill"),
        )
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(100).unwrap());
}

// ---------------------------------------------------------------------------
// Cancel-reject after a partial fill restores PartiallyFilled
// ---------------------------------------------------------------------------

#[test]
fn cancel_reject_after_partial_fill_restores_partially_filled() {
    let mut order = OmsOrder::new("ord-2", "AAPL", QtyMicros::from_whole_units(100).unwrap());

    // Partial fill first.
    order
        .apply(
            &OmsEvent::PartialFill {
                delta_qty: QtyMicros::from_whole_units(30).unwrap(),
            },
            Some("ev-pf"),
        )
        .unwrap();
    assert_eq!(order.state, OrderState::PartiallyFilled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(30).unwrap());

    // Application sends a cancel request.
    order
        .apply(&OmsEvent::CancelRequest, Some("ev-cancel-req"))
        .unwrap();
    assert_eq!(order.state, OrderState::CancelPending);

    // Broker rejects cancel → order reverts to PartiallyFilled (not Open).
    order
        .apply(&OmsEvent::CancelReject, Some("ev-cancel-rej"))
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::PartiallyFilled,
        "cancel-reject must restore PartiallyFilled when partial fills exist"
    );
    assert_eq!(
        order.filled_qty,
        QtyMicros::from_whole_units(30).unwrap(),
        "filled_qty must be unchanged after cancel-reject"
    );

    // Remaining fill completes the order.
    order
        .apply(
            &OmsEvent::Fill {
                delta_qty: QtyMicros::from_whole_units(70).unwrap(),
            },
            Some("ev-fill"),
        )
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(100).unwrap());
}

// ---------------------------------------------------------------------------
// Successful cancel terminates the order
// ---------------------------------------------------------------------------

#[test]
fn cancel_ack_terminates_order() {
    let mut order = OmsOrder::new("ord-3", "QQQ", QtyMicros::from_whole_units(50).unwrap());

    order
        .apply(&OmsEvent::CancelRequest, Some("ev-req"))
        .unwrap();
    order.apply(&OmsEvent::CancelAck, Some("ev-ack")).unwrap();

    assert_eq!(order.state, OrderState::Cancelled);
    assert!(
        order.state.is_terminal(),
        "Cancelled must be a terminal state"
    );
}

// ---------------------------------------------------------------------------
// Attempting to cancel an already-Filled order is an illegal transition
// ---------------------------------------------------------------------------

#[test]
fn cancel_on_filled_order_is_illegal_transition() {
    let mut order = OmsOrder::new("ord-4", "MSFT", QtyMicros::from_whole_units(50).unwrap());

    order
        .apply(
            &OmsEvent::Fill {
                delta_qty: QtyMicros::from_whole_units(50).unwrap(),
            },
            Some("ev-fill"),
        )
        .unwrap();
    assert_eq!(order.state, OrderState::Filled);

    // CancelRequest on a Filled order must be rejected.
    let result: Result<(), TransitionError> =
        order.apply(&OmsEvent::CancelRequest, Some("ev-cancel"));
    assert!(
        result.is_err(),
        "CancelRequest on Filled order must return TransitionError"
    );

    // State must remain Filled after the illegal transition attempt.
    assert_eq!(
        order.state,
        OrderState::Filled,
        "state must not change after illegal transition"
    );
}

// ---------------------------------------------------------------------------
// Fill arriving while cancel is in flight completes the order
// ---------------------------------------------------------------------------

#[test]
fn fill_during_cancel_pending_still_fills_order() {
    // In real markets, a fill can arrive before the exchange processes a cancel.
    let mut order = OmsOrder::new("ord-5", "GLD", QtyMicros::from_whole_units(30).unwrap());

    order
        .apply(&OmsEvent::CancelRequest, Some("ev-cancel"))
        .unwrap();
    assert_eq!(order.state, OrderState::CancelPending);

    // Fill arrives before cancel is processed.
    order
        .apply(
            &OmsEvent::Fill {
                delta_qty: QtyMicros::from_whole_units(30).unwrap(),
            },
            Some("ev-fill"),
        )
        .unwrap();
    assert_eq!(
        order.state,
        OrderState::Filled,
        "order must be Filled even when cancel was pending"
    );
    assert_eq!(order.filled_qty, QtyMicros::from_whole_units(30).unwrap());
}
