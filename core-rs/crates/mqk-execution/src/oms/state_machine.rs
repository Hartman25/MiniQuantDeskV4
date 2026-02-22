//! OMS State Machine — Patch L4
//!
//! # Design
//!
//! Explicit state machine for a single live broker order. Every lifecycle
//! event is applied via [`OmsOrder::apply`], which enforces two invariants:
//!
//! 1. **Legal transitions only.** Illegal events return
//!    [`TransitionError`], which callers MUST treat as a halt/alert signal.
//! 2. **Idempotent replay.** If an `event_id` is supplied and has already
//!    been applied, the call is a silent no-op — the order state does not
//!    change and no error is returned.
//!
//! # State diagram (simplified)
//!
//! ```text
//!                ┌──────────────────────────────────────────────────────┐
//!    new()       │           Ack (idempotent)                           │
//!    ──────►  Open ◄──────────────────────────────────────────────────  │
//!                │                                                       │
//!   PartialFill  │  CancelRequest    ReplaceRequest     Reject           │
//!    ──────► PartiallyFilled ───────────────────────► Rejected (term.)  │
//!                │            │            │                             │
//!    Fill        │            ▼            ▼                             │
//!    ──────► Filled (term.) CancelPending ReplacePending ─► ReplaceAck ─┘
//!                            │    │              │
//!                     CancelAck CancelReject  ReplaceReject
//!                            │    │              │
//!                            ▼    └──────────────┘
//!                       Cancelled (term.)   (restores Open|PartiallyFilled)
//! ```
//!
//! Late fills arriving while `CancelPending` or `ReplacePending` are accepted
//! (the broker may fill before processing the cancel/replace).

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// OrderState
// ---------------------------------------------------------------------------

/// All valid states a live OMS order can occupy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrderState {
    /// Order acknowledged by broker; no fills yet.
    Open,
    /// One or more partial fills received; order is not yet fully filled.
    PartiallyFilled,
    /// Order fully filled. **Terminal.**
    Filled,
    /// A cancel request has been sent; awaiting broker acknowledgement.
    CancelPending,
    /// Cancel acknowledged by broker. **Terminal.**
    Cancelled,
    /// A replace (amend) request has been sent; awaiting broker acknowledgement.
    ReplacePending,
    /// Order rejected by broker. **Terminal.**
    Rejected,
}

impl OrderState {
    /// Returns `true` if no further transitions are possible.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

// ---------------------------------------------------------------------------
// OmsEvent
// ---------------------------------------------------------------------------

/// Events that drive state transitions in an [`OmsOrder`].
#[derive(Debug, Clone, PartialEq)]
pub enum OmsEvent {
    /// Broker acknowledged the order (idempotent when already `Open`).
    Ack,
    /// A partial fill arrived. `delta_qty` is the quantity filled in this event.
    PartialFill { delta_qty: i64 },
    /// The final fill arrived, completing the order. `delta_qty` is this event's fill.
    Fill { delta_qty: i64 },
    /// Application requested a cancel (→ `CancelPending`).
    CancelRequest,
    /// Broker acknowledged the cancel (→ `Cancelled`).
    CancelAck,
    /// Broker rejected the cancel request (order reverts to its prior live state).
    CancelReject,
    /// Application requested a replace/amend (→ `ReplacePending`).
    ReplaceRequest,
    /// Broker acknowledged the replace (order reverts to its prior live state).
    ReplaceAck,
    /// Broker rejected the replace request (order reverts to its prior live state).
    ReplaceReject,
    /// Broker rejected the order outright (→ `Rejected`).
    Reject,
}

// ---------------------------------------------------------------------------
// TransitionError
// ---------------------------------------------------------------------------

/// Returned when an event cannot legally be applied in the current state.
///
/// **Callers MUST treat this as a halt/alert condition.** An illegal transition
/// indicates a serious OMS inconsistency (e.g. fill arriving after cancellation
/// was confirmed) that requires immediate operator investigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    /// The state the order was in when the illegal event arrived.
    pub from: OrderState,
    /// Debug string of the event that was rejected.
    pub event: String,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "illegal OMS transition: {:?} + {}",
            self.from, self.event
        )
    }
}

impl std::error::Error for TransitionError {}

// ---------------------------------------------------------------------------
// OmsOrder
// ---------------------------------------------------------------------------

/// A live OMS order tracked through an explicit state machine.
///
/// # Idempotency
///
/// Every call to [`apply`][`OmsOrder::apply`] accepts an optional `event_id`.
/// When supplied, the event ID is stored in an internal set; subsequent calls
/// with the same `event_id` are silently ignored. This guarantees that
/// replaying the same event log (e.g. on restart) converges to the same state.
#[derive(Debug, Clone)]
pub struct OmsOrder {
    /// Caller-assigned order identifier (e.g. the `client_order_id`).
    pub order_id: String,
    /// The traded instrument.
    pub symbol: String,
    /// Total quantity of the original order.
    pub total_qty: i64,
    /// Cumulative filled quantity across all fill events.
    pub filled_qty: i64,
    /// Current lifecycle state.
    pub state: OrderState,
    /// Applied event IDs — used for idempotent replay.
    applied: HashSet<String>,
}

impl OmsOrder {
    /// Create a new order in the `Open` state.
    ///
    /// # Panics (debug only)
    /// Panics if `total_qty` ≤ 0.
    pub fn new(order_id: impl Into<String>, symbol: impl Into<String>, total_qty: i64) -> Self {
        debug_assert!(total_qty > 0, "total_qty must be positive");
        Self {
            order_id: order_id.into(),
            symbol: symbol.into(),
            total_qty,
            filled_qty: 0,
            state: OrderState::Open,
            applied: HashSet::new(),
        }
    }

    /// Apply an event to this order.
    ///
    /// `event_id` — if `Some`, deduplicated against the set of already-applied
    /// event IDs. A duplicate returns `Ok(())` immediately without mutating state.
    ///
    /// # Errors
    /// Returns [`TransitionError`] for illegal transitions. Callers **MUST**
    /// treat this as a halt condition.
    pub fn apply(
        &mut self,
        event: &OmsEvent,
        event_id: Option<&str>,
    ) -> Result<(), TransitionError> {
        // Idempotency: skip events we have already processed.
        if let Some(id) = event_id {
            if self.applied.contains(id) {
                return Ok(());
            }
        }

        self.do_transition(event)?;

        if let Some(id) = event_id {
            self.applied.insert(id.to_string());
        }

        Ok(())
    }

    // Internal: perform the actual state machine transition.
    fn do_transition(&mut self, event: &OmsEvent) -> Result<(), TransitionError> {
        use OmsEvent::*;
        use OrderState::*;

        match (&self.state, event) {
            // ------------------------------------------------------------------
            // Ack: idempotent when already Open or PartiallyFilled.
            // ------------------------------------------------------------------
            (Open | PartiallyFilled, Ack) => {}

            // ------------------------------------------------------------------
            // Partial fills: accepted from any live state (fills may arrive
            // while a cancel or replace is in flight).
            // ------------------------------------------------------------------
            (
                Open | PartiallyFilled | CancelPending | ReplacePending,
                PartialFill { delta_qty },
            ) => {
                self.filled_qty += delta_qty;
                self.state = PartiallyFilled;
            }

            // ------------------------------------------------------------------
            // Final fill: accepted from any live state for the same reason.
            // ------------------------------------------------------------------
            (Open | PartiallyFilled | CancelPending | ReplacePending, Fill { delta_qty }) => {
                self.filled_qty += delta_qty;
                self.state = Filled;
            }

            // Late-duplicate fill on an already-Filled order: silently ignored.
            (Filled, Fill { .. } | PartialFill { .. }) => {}

            // ------------------------------------------------------------------
            // Cancel flow
            // ------------------------------------------------------------------
            (Open | PartiallyFilled, CancelRequest) => self.state = CancelPending,

            (CancelPending, CancelAck) => self.state = Cancelled,

            // Cancel rejected → order is still alive; restore the prior live state.
            (CancelPending, CancelReject) => {
                self.state = if self.filled_qty > 0 {
                    PartiallyFilled
                } else {
                    Open
                };
            }

            // ------------------------------------------------------------------
            // Replace flow
            // ------------------------------------------------------------------
            (Open | PartiallyFilled, ReplaceRequest) => self.state = ReplacePending,

            // Replace confirmed → order is live again.
            (ReplacePending, ReplaceAck) => {
                self.state = if self.filled_qty > 0 {
                    PartiallyFilled
                } else {
                    Open
                };
            }

            // Replace rejected → order reverts to its prior live state.
            (ReplacePending, ReplaceReject) => {
                self.state = if self.filled_qty > 0 {
                    PartiallyFilled
                } else {
                    Open
                };
            }

            // ------------------------------------------------------------------
            // Broker reject: accepted from any non-terminal live state.
            // ------------------------------------------------------------------
            (Open | PartiallyFilled | CancelPending | ReplacePending, Reject) => {
                self.state = Rejected;
            }

            // ------------------------------------------------------------------
            // Everything else is illegal.
            // ------------------------------------------------------------------
            (state, ev) => {
                return Err(TransitionError {
                    from: state.clone(),
                    event: format!("{ev:?}"),
                });
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn open_order() -> OmsOrder {
        OmsOrder::new("ord-test", "AAPL", 100)
    }

    #[test]
    fn new_order_starts_open() {
        let o = open_order();
        assert_eq!(o.state, OrderState::Open);
        assert_eq!(o.filled_qty, 0);
        assert!(!o.state.is_terminal());
    }

    #[test]
    fn ack_is_idempotent() {
        let mut o = open_order();
        o.apply(&OmsEvent::Ack, Some("a1")).unwrap();
        o.apply(&OmsEvent::Ack, Some("a1")).unwrap();
        assert_eq!(o.state, OrderState::Open);
    }

    #[test]
    fn partial_then_full_fill() {
        let mut o = open_order();
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f1"))
            .unwrap();
        assert_eq!(o.state, OrderState::PartiallyFilled);
        assert_eq!(o.filled_qty, 60);
        o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("f2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);
        assert!(o.state.is_terminal());
    }

    #[test]
    fn cancel_reject_reverts_to_open() {
        let mut o = open_order();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        assert_eq!(o.state, OrderState::CancelPending);
        o.apply(&OmsEvent::CancelReject, Some("c2")).unwrap();
        assert_eq!(o.state, OrderState::Open);
    }

    #[test]
    fn replace_request_then_ack() {
        let mut o = open_order();
        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::ReplacePending);
        o.apply(&OmsEvent::ReplaceAck, Some("r2")).unwrap();
        assert_eq!(o.state, OrderState::Open);
    }

    #[test]
    fn illegal_transition_returns_error() {
        let mut o = open_order();
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
            .unwrap();
        // CancelRequest on a Filled order is illegal.
        let err = o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap_err();
        assert_eq!(err.from, OrderState::Filled);
        // State must not change after the error.
        assert_eq!(o.state, OrderState::Filled);
    }

    #[test]
    fn idempotent_replay_does_not_double_apply() {
        let mut o = open_order();
        o.apply(&OmsEvent::PartialFill { delta_qty: 50 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 50);
        // Same event_id → silently skipped.
        o.apply(&OmsEvent::PartialFill { delta_qty: 50 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 50, "replayed event must not double-apply");
    }

    #[test]
    fn late_fill_on_filled_order_is_noop() {
        let mut o = open_order();
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        // Different event_id but state is Filled → no-op.
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f-late"))
            .unwrap();
        assert_eq!(o.filled_qty, 100);
        assert_eq!(o.state, OrderState::Filled);
    }

    #[test]
    fn fill_during_cancel_pending() {
        let mut o = open_order();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        // Fill arrives before cancel is processed.
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
    }
}
