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
    ///
    /// P1-03: `new_total_qty` is the authoritative post-replace total quantity,
    /// equal to `filled_qty_at_replace + new_open_leaves`. The OMS updates
    /// `self.total_qty` to this value so that subsequent fills validate against
    /// the amended order size rather than the original.
    ReplaceAck { new_total_qty: i64 },
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
    /// Indexed by the caller-supplied event identity string.
    /// Only successful transitions are recorded; rejected events are never
    /// inserted, so the same event_id can be retried with a corrected payload.
    applied_event_ids: HashSet<String>,
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
            applied_event_ids: HashSet::new(),
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
        // Section B — Fill Identity & Deduplication
        // Early check: if this event_id has already been applied, return a
        // silent no-op before any state mutation. This must fire before
        // do_transition so that even an overflow-carrying duplicate cannot
        // reach the quantity guards.
        if let Some(id) = event_id {
            if self.applied_event_ids.contains(id) {
                return Ok(());
            }
        }

        self.do_transition(event)?;

        // Record identity only after a successful transition. Rejected events
        // (TransitionError propagated via `?` above) never reach this line,
        // so their event_id remains available for a corrected retry.
        if let Some(id) = event_id {
            self.applied_event_ids.insert(id.to_string());
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
            //
            // Invariants enforced before any mutation:
            //   1. delta_qty must be positive.
            //   2. filled_qty + delta_qty must not exceed total_qty.
            // A violation returns TransitionError; state and filled_qty are
            // unchanged and the event_id is NOT recorded.
            // ------------------------------------------------------------------
            (
                Open | PartiallyFilled | CancelPending | ReplacePending,
                PartialFill { delta_qty },
            ) => {
                if *delta_qty <= 0 {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "PartialFill(delta_qty={}) — delta_qty must be positive",
                            delta_qty
                        ),
                    });
                }
                let proposed = self.filled_qty + delta_qty;
                if proposed > self.total_qty {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "PartialFill(delta_qty={}) — would overflow: \
                             filled={} + delta={} = {} > total={}",
                            delta_qty, self.filled_qty, delta_qty, proposed, self.total_qty
                        ),
                    });
                }
                self.filled_qty = proposed;
                self.state = PartiallyFilled;
            }

            // ------------------------------------------------------------------
            // Final fill from PartiallyFilled: accepted when proposed >= total_qty.
            //
            // Alpaca paper WS sends the terminal `fill` event with `delta_qty`
            // equal to the prior cumulative filled qty (not the remaining
            // incremental qty).  For a 3-share order with a prior partial_fill
            // of 2 shares, Alpaca sends Fill(delta_qty=2) rather than
            // Fill(delta_qty=1), producing proposed=4 > total_qty=3.
            //
            // Invariants enforced before mutation:
            //   1. delta_qty must be positive.
            //   2. proposed must be >= total_qty (underfill on terminal fill is
            //      still an error; the broker must complete the order).
            //   3. filled_qty is capped at total_qty so the order closes exactly.
            //
            // The caller (apply_fill_step) reads `order.filled_qty - pre_qty`
            // as the effective portfolio delta; this is 1 (= total_qty - prior)
            // not 2 (the broker-reported delta_qty), preventing over-crediting.
            // ------------------------------------------------------------------
            (PartiallyFilled, Fill { delta_qty }) => {
                if *delta_qty <= 0 {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "Fill(delta_qty={}) — delta_qty must be positive",
                            delta_qty
                        ),
                    });
                }
                let proposed = self.filled_qty + delta_qty;
                if proposed < self.total_qty {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "Fill(delta_qty={}) — proposed_filled={} < total_qty={} \
                             (filled={}); terminal fill must complete the order",
                            delta_qty, proposed, self.total_qty, self.filled_qty
                        ),
                    });
                }
                // Cap at total_qty: handles Alpaca sending cumulative or
                // overfill qty for the terminal fill event.
                self.filled_qty = self.total_qty;
                self.state = Filled;
            }

            // ------------------------------------------------------------------
            // Final fill from Open/CancelPending/ReplacePending: exact balance.
            //
            // For orders with no prior partial fills, the broker's delta_qty
            // must equal total_qty exactly (no cumulative ambiguity exists).
            //
            // Invariants enforced before any mutation:
            //   1. delta_qty must be positive.
            //   2. filled_qty + delta_qty must equal total_qty exactly.
            //      Under-completion (< total_qty) and overflow (> total_qty)
            //      are both rejected — the caller used Fill instead of
            //      PartialFill for an incomplete fill, which is a logic error.
            // A violation returns TransitionError; state and filled_qty are
            // unchanged and the event_id is NOT recorded.
            // ------------------------------------------------------------------
            (Open | CancelPending | ReplacePending, Fill { delta_qty }) => {
                if *delta_qty <= 0 {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "Fill(delta_qty={}) — delta_qty must be positive",
                            delta_qty
                        ),
                    });
                }
                let proposed = self.filled_qty + delta_qty;
                if proposed != self.total_qty {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "Fill(delta_qty={}) — proposed_filled={} must equal \
                             total_qty={} (filled={}); use PartialFill for incomplete fills",
                            delta_qty, proposed, self.total_qty, self.filled_qty
                        ),
                    });
                }
                self.filled_qty = proposed;
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
            //
            // P1-03: Update total_qty to the authoritative post-replace total.
            // Reject if new_total_qty < filled_qty — that would create an
            // immediately-overfilled order and violate the core invariant.
            (ReplacePending, ReplaceAck { new_total_qty }) => {
                if *new_total_qty < self.filled_qty {
                    return Err(TransitionError {
                        from: self.state.clone(),
                        event: format!(
                            "ReplaceAck(new_total_qty={}) — below already-filled qty={}",
                            new_total_qty, self.filled_qty
                        ),
                    });
                }
                self.total_qty = *new_total_qty;
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
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::ReplacePending);
        // P1-03: ReplaceAck carries new_total_qty. Order has no fills so new total = 100.
        o.apply(&OmsEvent::ReplaceAck { new_total_qty: 100 }, Some("r2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Open);
        assert_eq!(o.total_qty, 100);
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

    // -----------------------------------------------------------------------
    // Section A — OMS Fill Truth: invariant enforcement
    // -----------------------------------------------------------------------

    /// A PartialFill whose cumulative sum would exceed total_qty is rejected.
    /// State and filled_qty must be unchanged.
    #[test]
    fn partial_fill_overflow_is_rejected() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 60);

        // 60 + 60 = 120 > 100 — must be rejected.
        let err = o
            .apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f2"))
            .unwrap_err();
        assert_eq!(
            err.from,
            OrderState::PartiallyFilled,
            "TransitionError.from must reflect the state at rejection"
        );
        // State and accounting must be unchanged after rejection.
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "state must not change on rejected PartialFill"
        );
        assert_eq!(
            o.filled_qty, 60,
            "filled_qty must not be mutated on rejected PartialFill"
        );
    }

    /// A Fill whose delta pushes cumulative filled above total_qty is rejected.
    #[test]
    fn fill_overflow_is_rejected() {
        let mut o = open_order(); // total_qty = 100
                                  // 0 + 101 = 101 != 100 — overflow.
        let err = o
            .apply(&OmsEvent::Fill { delta_qty: 101 }, Some("f1"))
            .unwrap_err();
        assert_eq!(
            err.from,
            OrderState::Open,
            "TransitionError.from must reflect the state at rejection"
        );
        assert_eq!(
            o.state,
            OrderState::Open,
            "state must not change on rejected Fill"
        );
        assert_eq!(
            o.filled_qty, 0,
            "filled_qty must not be mutated on rejected Fill"
        );
    }

    /// A Fill that would leave the order under-complete (proposed < total_qty)
    /// is rejected. The caller must use PartialFill for incomplete fills.
    #[test]
    fn undercomplete_fill_is_rejected() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 60);

        // 60 + 30 = 90 != 100 — under-complete.
        let err = o
            .apply(&OmsEvent::Fill { delta_qty: 30 }, Some("f2"))
            .unwrap_err();
        assert_eq!(err.from, OrderState::PartiallyFilled);
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "state must not change on under-complete Fill"
        );
        assert_eq!(
            o.filled_qty, 60,
            "filled_qty must not be mutated on under-complete Fill"
        );
    }

    /// Valid path: PartialFill(60) + Fill(40) on total=100 still works.
    #[test]
    fn valid_partial_then_exact_fill_completes() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 60);
        assert_eq!(o.state, OrderState::PartiallyFilled);

        o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("f2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);
        assert!(o.state.is_terminal());
    }

    /// A rejected fill must NOT record the event_id as applied.
    /// Proof: apply the same event_id again with a valid fill — it must
    /// succeed (not be silently skipped as a duplicate).
    #[test]
    fn rejected_fill_event_id_is_not_recorded() {
        let mut o = open_order(); // total_qty = 100
                                  // Overflow — rejected.
        let _err = o
            .apply(&OmsEvent::Fill { delta_qty: 999 }, Some("f-probe"))
            .unwrap_err();
        assert_eq!(o.state, OrderState::Open);
        assert_eq!(o.filled_qty, 0);

        // Re-use "f-probe" with a valid fill. If the event_id was recorded on
        // rejection, this call would be silently skipped and the order would
        // stay Open. It must NOT be skipped.
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f-probe"))
            .unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "rejected event_id must not be recorded; re-use with valid fill must apply"
        );
        assert_eq!(o.filled_qty, 100);
    }

    /// A late fill arriving on an already-Filled order is a no-op by state,
    /// regardless of delta_qty. The state-based guard (Filled → noop) fires
    /// before quantity validation, so even an "oversized" late fill is safe.
    #[test]
    fn late_fill_on_filled_order_is_noop_regardless_of_qty() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);

        // Late fill with a new event_id — state guard makes it a no-op.
        o.apply(&OmsEvent::Fill { delta_qty: 999 }, Some("f-late"))
            .unwrap();
        assert_eq!(
            o.filled_qty, 100,
            "late fill on Filled order must be a no-op regardless of qty"
        );
        assert_eq!(o.state, OrderState::Filled);
    }

    // -----------------------------------------------------------------------
    // Section B — Fill Identity & Deduplication
    // -----------------------------------------------------------------------

    /// Duplicate PartialFill carrying the same event_id must be a silent no-op.
    /// filled_qty must not accumulate past the first application.
    /// State must not change.
    #[test]
    fn duplicate_partial_fill_is_noop() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("E1"))
            .unwrap();
        assert_eq!(o.filled_qty, 60);
        assert_eq!(o.state, OrderState::PartiallyFilled);

        // Duplicate — same event_id "E1", same delta.
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("E1"))
            .unwrap();
        assert_eq!(
            o.filled_qty, 60,
            "duplicate PartialFill must not increase filled_qty"
        );
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "state must not change on duplicate PartialFill"
        );
    }

    /// Duplicate final Fill event after order completion must be a silent no-op.
    /// Event-id deduplication must fire before the Filled→noop state guard,
    /// so that the applied_event_ids set remains the single source of truth
    /// for identity dedup on non-terminal-state paths.
    #[test]
    fn duplicate_final_fill_is_noop() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("E1"))
            .unwrap();
        o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("E2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);

        // Duplicate Fill(E2) on now-Filled order — must be a no-op.
        o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("E2"))
            .unwrap();
        assert_eq!(
            o.filled_qty, 100,
            "duplicate Fill must not increase filled_qty"
        );
        assert_eq!(
            o.state,
            OrderState::Filled,
            "duplicate Fill must not alter terminal state"
        );
    }

    /// 50 repeated deliveries of the same PartialFill event (broker storm)
    /// must produce exactly one fill's worth of quantity accumulation.
    #[test]
    fn duplicate_storm_fifty_repeats_accumulates_once() {
        let mut o = open_order(); // total_qty = 100
        for _ in 0..50 {
            o.apply(&OmsEvent::PartialFill { delta_qty: 50 }, Some("E1"))
                .unwrap();
        }
        assert_eq!(
            o.filled_qty, 50,
            "50 duplicate storm events must accumulate exactly once"
        );
        assert_eq!(o.state, OrderState::PartiallyFilled);
    }

    /// After order reaches a terminal state, repeated delivery of the fill
    /// event that triggered the terminal transition must be a pure no-op.
    /// Neither state nor filled_qty may change across any of the repeats.
    #[test]
    fn duplicate_after_terminal_state_is_noop() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("E1"))
            .unwrap();
        o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("E2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);

        for _ in 0..10 {
            o.apply(&OmsEvent::Fill { delta_qty: 40 }, Some("E2"))
                .unwrap();
        }
        assert_eq!(
            o.state,
            OrderState::Filled,
            "terminal state must not change on repeated duplicate fills"
        );
        assert_eq!(
            o.filled_qty, 100,
            "filled_qty must not change on repeated duplicate fills after terminal"
        );
    }

    // -----------------------------------------------------------------------
    // Section C — P1-03: Cancel / Replace parity after partial fills
    // -----------------------------------------------------------------------

    /// S1: new → partial_fill(40) → replace_request → replace_ack(new_total=65) → fill(25) → Filled.
    ///
    /// Acceptance gates:
    /// - total_qty updated to new_total_qty on ReplaceAck.
    /// - Replace does not erase prior fills (filled_qty remains 40 post-ack).
    /// - Partial fill + replace + fill preserves exact cumulative quantity (65).
    /// - No path permits filled_qty > total_qty.
    #[test]
    fn p1_03_partial_fill_then_replace_then_fill() {
        let mut o = OmsOrder::new("ord-p103-1", "AAPL", 100);

        // 40 of 100 filled.
        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 40);
        assert_eq!(o.total_qty, 100);

        // Replace: new open leaves = 25 → new total = 40 + 25 = 65.
        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::ReplacePending);

        o.apply(&OmsEvent::ReplaceAck { new_total_qty: 65 }, Some("r2"))
            .unwrap();
        assert_eq!(o.state, OrderState::PartiallyFilled);
        assert_eq!(
            o.total_qty, 65,
            "total_qty must be updated to new_total_qty"
        );
        assert_eq!(o.filled_qty, 40, "replace must not erase prior fills");

        // Final fill for remaining 25 lots.
        o.apply(&OmsEvent::Fill { delta_qty: 25 }, Some("f2"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 65);

        // Acceptance gate: no path permits filled_qty > total_qty.
        assert!(
            o.filled_qty <= o.total_qty,
            "filled_qty must never exceed total_qty"
        );
    }

    /// S2a: new → partial_fill(40) → cancel_request → cancel_ack → Cancelled.
    ///
    /// Acceptance gate: cancel after partial fill must not erase prior fills.
    #[test]
    fn p1_03_cancel_after_partial_fill_preserves_filled_qty() {
        let mut o = OmsOrder::new("ord-p103-2a", "MSFT", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 40);

        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        assert_eq!(o.state, OrderState::CancelPending);

        // CancelAck: order is cancelled; prior fills must be preserved.
        o.apply(&OmsEvent::CancelAck, Some("c2")).unwrap();
        assert_eq!(o.state, OrderState::Cancelled);
        assert_eq!(o.filled_qty, 40, "cancel must not erase prior fills");
    }

    /// S2b: new → partial_fill(40) → cancel_request → late_partial_fill(30) during pending.
    ///
    /// Acceptance gate: late fills after cancel request must still apply correctly.
    /// Note: a PartialFill accepted from CancelPending transitions state back to
    /// PartiallyFilled (the broker filled before processing the cancel); the test
    /// captures this correct OMS behavior.
    #[test]
    fn p1_03_late_fill_during_cancel_pending_applies_correctly() {
        let mut o = OmsOrder::new("ord-p103-2b", "MSFT", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 40);

        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        assert_eq!(o.state, OrderState::CancelPending);

        // Late fill arrives before the broker processes the cancel — must apply.
        o.apply(&OmsEvent::PartialFill { delta_qty: 30 }, Some("f2"))
            .unwrap();
        assert_eq!(
            o.filled_qty, 70,
            "late fill during CancelPending must accumulate filled_qty"
        );
        // PartialFill from CancelPending transitions to PartiallyFilled; the broker
        // filled before the cancel was acknowledged.
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "PartialFill during CancelPending returns order to PartiallyFilled"
        );
        assert!(
            o.filled_qty <= o.total_qty,
            "no path permits filled_qty > total_qty"
        );
    }

    /// S3: new → partial_fill(40) → replace_request → replace_reject → PartiallyFilled, qty unchanged.
    ///
    /// Acceptance gate: replace reject restores prior state; total_qty and filled_qty unchanged.
    #[test]
    fn p1_03_partial_fill_then_replace_reject() {
        let mut o = OmsOrder::new("ord-p103-3", "TSLA", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 40);

        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::ReplacePending);

        o.apply(&OmsEvent::ReplaceReject, Some("r2")).unwrap();
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "replace reject must restore PartiallyFilled"
        );
        assert_eq!(
            o.total_qty, 100,
            "total_qty must be unchanged on replace reject"
        );
        assert_eq!(
            o.filled_qty, 40,
            "filled_qty must be unchanged on replace reject"
        );
    }

    /// S4: new → partial_fill(40) → cancel_request → cancel_reject → PartiallyFilled.
    ///
    /// Acceptance gate: cancel reject restores prior state; filled_qty unchanged.
    #[test]
    fn p1_03_partial_fill_then_cancel_reject() {
        let mut o = OmsOrder::new("ord-p103-4", "SPY", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 40);

        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        assert_eq!(o.state, OrderState::CancelPending);

        o.apply(&OmsEvent::CancelReject, Some("c2")).unwrap();
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "cancel reject must restore PartiallyFilled"
        );
        assert_eq!(
            o.filled_qty, 40,
            "filled_qty must be unchanged on cancel reject"
        );
    }

    /// Acceptance gate: ReplaceAck with new_total_qty < filled_qty is rejected.
    /// No path permits filled_qty > total_qty.
    #[test]
    fn p1_03_replace_ack_below_filled_qty_is_rejected() {
        let mut o = OmsOrder::new("ord-p103-5", "GLD", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("f1"))
            .unwrap();
        assert_eq!(o.filled_qty, 60);

        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::ReplacePending);

        // new_total_qty=40 < filled_qty=60 — must be rejected.
        let err = o
            .apply(&OmsEvent::ReplaceAck { new_total_qty: 40 }, Some("r2"))
            .unwrap_err();
        assert_eq!(
            err.from,
            OrderState::ReplacePending,
            "TransitionError.from must be ReplacePending"
        );
        // State, total_qty, filled_qty must all be unchanged.
        assert_eq!(
            o.state,
            OrderState::ReplacePending,
            "state must not change on rejected ReplaceAck"
        );
        assert_eq!(
            o.total_qty, 100,
            "total_qty must not change on rejected ReplaceAck"
        );
        assert_eq!(
            o.filled_qty, 60,
            "filled_qty must not change on rejected ReplaceAck"
        );
    }

    /// Acceptance gate: replace_ack preserves filled_qty exactly.
    /// Replace cannot erase prior fills.
    #[test]
    fn p1_03_replace_ack_preserves_filled_qty() {
        let mut o = OmsOrder::new("ord-p103-6", "NVDA", 100);

        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        o.apply(&OmsEvent::ReplaceRequest, Some("r1")).unwrap();
        o.apply(&OmsEvent::ReplaceAck { new_total_qty: 65 }, Some("r2"))
            .unwrap();

        assert_eq!(o.filled_qty, 40, "replace_ack must not erase prior fills");
        assert_eq!(
            o.total_qty, 65,
            "total_qty must be updated to new_total_qty"
        );
        // Ensure the event_id for ReplaceAck was recorded (idempotent replay).
        o.apply(&OmsEvent::ReplaceAck { new_total_qty: 99 }, Some("r2"))
            .unwrap();
        assert_eq!(
            o.total_qty, 65,
            "duplicate ReplaceAck event_id must not re-apply"
        );
    }

    /// A rejected fill must not poison its event_id.
    /// Scenario: order at filled_qty=90, Fill(E1, delta=20) rejected because
    /// A rejected event_id must not be recorded, so it remains usable for a
    /// corrected re-submission.  Uses Open-state overfill (still rejected after
    /// the PartiallyFilled relaxation introduced for Alpaca paper WS fills).
    #[test]
    fn rejected_fill_does_not_poison_event_identity() {
        let mut o = open_order(); // total_qty = 100, state = Open

        // Fill(E1, 101) from Open: 0 + 101 = 101 ≠ 100 — strict exact-balance
        // required in Open state; rejected.
        let err = o
            .apply(&OmsEvent::Fill { delta_qty: 101 }, Some("E1"))
            .unwrap_err();
        assert_eq!(err.from, OrderState::Open);
        assert_eq!(o.filled_qty, 0, "rejected fill must not mutate filled_qty");

        // Fill(E1, 100): same event_id, corrected delta. 0 + 100 == total_qty.
        // If E1 had been poisoned on rejection this call would silently skip.
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("E1"))
            .unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "valid fill with previously-rejected event_id must be accepted"
        );
        assert_eq!(o.filled_qty, 100);
    }

    // -----------------------------------------------------------------------
    // Section C — Alpaca paper WS terminal fill: cumulative-qty regression
    //
    // Alpaca paper WS sends the terminal `fill` event with `delta_qty` equal
    // to the prior cumulative filled qty rather than the remaining incremental
    // qty (observed in supervised paper smoke 2026-06-01).
    //
    // Scenario: order qty=3, partial_fill(2) → fill(2) where the fill's
    // delta_qty=2 matches the prior partial fill, not the remaining 1 share.
    // The OMS must cap filled_qty at total_qty and mark the order Filled.
    // -----------------------------------------------------------------------

    /// PartialFill(2) then Fill(2) on a 3-share order (Alpaca sends prior
    /// cumulative as terminal fill qty).  The OMS must accept and cap at 3.
    #[test]
    fn alpaca_paper_terminal_fill_cumulative_qty_is_accepted() {
        let mut o = OmsOrder::new("ord-alpaca", "AAPL", 3);
        o.apply(&OmsEvent::Ack, Some("ack-1")).unwrap();

        // partial_fill: 2 shares filled, 1 remaining.
        o.apply(&OmsEvent::PartialFill { delta_qty: 2 }, Some("pf-1"))
            .unwrap();
        assert_eq!(o.filled_qty, 2);
        assert_eq!(o.state, OrderState::PartiallyFilled);

        // terminal fill: Alpaca sends delta_qty=2 (prior cumulative, not remaining 1).
        // Must be accepted and capped at total_qty=3.
        o.apply(&OmsEvent::Fill { delta_qty: 2 }, Some("fill-1"))
            .unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "terminal fill with cumulative qty must close the order"
        );
        assert_eq!(
            o.filled_qty, 3,
            "filled_qty must be capped at total_qty, not overflowed"
        );
        assert!(o.state.is_terminal());
    }

    /// Two partial fills followed by an Alpaca-style cumulative terminal fill.
    /// Scenario mirrors N=2 partial fills before the terminal fill event, which
    /// is possible when Alpaca splits a 3-share order into two 1-share partials.
    ///
    /// PartialFill(1) + PartialFill(1) → filled_qty=2, then Fill(delta_qty=2)
    /// where Alpaca sends the prior cumulative as the terminal fill qty.
    /// proposed = 2 + 2 = 4 > total=3 → cap at 3, Filled.
    #[test]
    fn alpaca_paper_two_partials_then_cumulative_terminal_fill_is_accepted() {
        let mut o = OmsOrder::new("ord-multi", "AAPL", 3);
        o.apply(&OmsEvent::PartialFill { delta_qty: 1 }, Some("pf-1"))
            .unwrap();
        assert_eq!(o.filled_qty, 1);
        o.apply(&OmsEvent::PartialFill { delta_qty: 1 }, Some("pf-2"))
            .unwrap();
        assert_eq!(o.filled_qty, 2);
        assert_eq!(o.state, OrderState::PartiallyFilled);

        // Alpaca terminal fill sends delta_qty=2 (prior cumulative), not remaining 1.
        o.apply(&OmsEvent::Fill { delta_qty: 2 }, Some("fill-1"))
            .unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "two partials + cumulative terminal fill must close the order"
        );
        assert_eq!(o.filled_qty, 3, "filled_qty must be capped at total_qty=3");
        assert!(o.state.is_terminal());
    }

    /// Undercomplete terminal fill from PartiallyFilled is still rejected.
    /// (proposed < total_qty remains an error even after the overfill relaxation)
    #[test]
    fn alpaca_paper_undercomplete_terminal_fill_is_still_rejected() {
        let mut o = OmsOrder::new("ord-test", "AAPL", 10);
        o.apply(&OmsEvent::PartialFill { delta_qty: 5 }, Some("pf-1"))
            .unwrap();
        assert_eq!(o.filled_qty, 5);

        // Terminal fill where proposed=5+3=8 < total=10 — undercomplete, rejected.
        let err = o
            .apply(&OmsEvent::Fill { delta_qty: 3 }, Some("fill-bad"))
            .unwrap_err();
        assert_eq!(err.from, OrderState::PartiallyFilled);
        assert_eq!(
            o.state,
            OrderState::PartiallyFilled,
            "state must not change on undercomplete terminal fill"
        );
        assert_eq!(
            o.filled_qty, 5,
            "filled_qty must not be mutated on undercomplete terminal fill"
        );
    }

    /// Fill(delta_qty=total_qty) from PartiallyFilled where Alpaca sends the
    /// order's total qty as the terminal fill qty.  Must be accepted and capped.
    #[test]
    fn alpaca_paper_terminal_fill_total_qty_is_accepted() {
        let mut o = OmsOrder::new("ord-test", "AAPL", 5);
        o.apply(&OmsEvent::PartialFill { delta_qty: 3 }, Some("pf-1"))
            .unwrap();
        assert_eq!(o.filled_qty, 3);

        // Alpaca sends delta_qty=5 (full order qty) for terminal fill.
        // proposed=3+5=8 > total=5 → cap at 5.
        o.apply(&OmsEvent::Fill { delta_qty: 5 }, Some("fill-1"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 5);
    }
}
