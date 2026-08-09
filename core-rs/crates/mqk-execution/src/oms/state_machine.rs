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
        self.apply_with_watermark(event, event_id, None)
    }

    /// PAPER-SOAK-PARTIAL-FILL-DEDUP-02: like [`apply`][Self::apply], but for
    /// `PartialFill`/`Fill` events additionally accepts the broker-reported
    /// authoritative cumulative-filled-quantity watermark for this order as
    /// of this specific event (`BrokerEvent::cum_qty_after`), when the caller
    /// has one.
    ///
    /// # Why this exists
    ///
    /// `event_id`-based dedup (the pre-existing mechanism, still the sole
    /// guard when `cum_qty_after` is `None`) cannot detect a duplicate that
    /// arrives with a *different* economic event id — which is exactly what
    /// happens when the same physical partial fill is delivered once over
    /// the WS inbound lane and once over the REST gap-recovery lane: each
    /// lane derives a different id for the same execution (see
    /// `mqk_db::inbox` module docs). `cum_qty_after` is the one signal both
    /// lanes can report with the *same* true value for the *same* physical
    /// execution, because it names a point in the order's broker-side fill
    /// history rather than a transport-specific message identity.
    ///
    /// # Semantics
    ///
    /// - `cum_qty_after` is `Some(target)` and `target <= self.filled_qty`:
    ///   this event's economic effect is already reflected in the order's
    ///   state (whether via `event_id` match or a prior delivery on the
    ///   other lane) — a no-op. `event_id`, if supplied, is still recorded
    ///   so a same-lane retry short-circuits on the cheaper id-only path.
    /// - `cum_qty_after` is `Some(target)` and `target > self.filled_qty`:
    ///   this is a genuine advance. The event actually applied uses the
    ///   broker-confirmed delta `target - self.filled_qty` — the corrected,
    ///   authoritative amount — in place of the event's own `delta_qty`,
    ///   which guards against a mismatched/derived delta on the REST lane.
    /// - `cum_qty_after` is `None`: behavior is byte-for-byte identical to
    ///   `apply` today (pure `event_id` dedup). This is the path exercised
    ///   by every existing caller and test; it is not weakened.
    ///
    /// Two genuinely distinct fills — even with identical `delta_qty`,
    /// `price_micros`, and near-simultaneous timestamps — always carry
    /// different `cum_qty_after` values (cumulative filled quantity strictly
    /// increases with each real execution), so this mechanism can never
    /// collapse them: no time window, no quantity/price heuristic is used.
    ///
    /// # Errors
    /// Returns [`TransitionError`] for illegal transitions. Callers **MUST**
    /// treat this as a halt condition.
    pub fn apply_with_watermark(
        &mut self,
        event: &OmsEvent,
        event_id: Option<&str>,
        cum_qty_after: Option<i64>,
    ) -> Result<(), TransitionError> {
        let is_fill_event = matches!(event, OmsEvent::PartialFill { .. } | OmsEvent::Fill { .. });

        // A fill event is only ever legal (either a mutating advance or a
        // late-duplicate no-op) from these states; from Cancelled/Rejected it
        // is a TransitionError regardless of watermark (see `do_transition`'s
        // match arms and the C17 acceptance tests). The watermark fast path
        // below must not bypass that: a late/duplicate fill redelivered
        // AFTER a confirmed cancel/reject is exactly the anomaly C17 requires
        // to halt so an operator is alerted, not a benign no-op to swallow.
        let fill_event_is_legal_here = matches!(
            self.state,
            OrderState::Open
                | OrderState::PartiallyFilled
                | OrderState::CancelPending
                | OrderState::ReplacePending
                | OrderState::Filled
        );

        // Broker-authoritative watermark check: fires before the event_id
        // check so a cross-lane duplicate (different event_id, same
        // already-reflected cumulative quantity) is caught even though its
        // event_id has never been seen before. Restricted to states where a
        // fill is legal at all, so it can never mask an illegal transition.
        if is_fill_event && fill_event_is_legal_here {
            if let Some(target) = cum_qty_after {
                if target <= self.filled_qty {
                    if let Some(id) = event_id {
                        self.applied_event_ids.insert(id.to_string());
                    }
                    return Ok(());
                }
            }
        }

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

        // When the watermark proves a genuine advance, apply the
        // broker-confirmed delta rather than the event's own delta_qty —
        // this is the authoritative amount for this order regardless of
        // what the originating lane derived it as.
        let corrected;
        let effective_event = match (event, cum_qty_after) {
            (OmsEvent::PartialFill { .. }, Some(target)) => {
                corrected = OmsEvent::PartialFill {
                    delta_qty: target - self.filled_qty,
                };
                &corrected
            }
            (OmsEvent::Fill { .. }, Some(target)) => {
                corrected = OmsEvent::Fill {
                    delta_qty: target - self.filled_qty,
                };
                &corrected
            }
            _ => event,
        };

        self.do_transition(effective_event)?;

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

            // Late Ack arriving on a terminal order: silently ignored.
            //
            // B14: Broker WS events can arrive out of order (e.g. Ack after Fill
            // on reconnect/replay). A late Ack on Filled/Cancelled/Rejected is
            // not an OMS inconsistency — the order reached its terminal state
            // legitimately. Treat identically to late fills on Filled.
            (Filled | Cancelled | Rejected, Ack) => {}

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

    // -----------------------------------------------------------------------
    // B14 — Late ACK after terminal fill
    // -----------------------------------------------------------------------

    /// B14-1: Ack arriving on a Filled order with a new event_id must be a
    /// silent noop. Broker WS can replay or deliver Ack out of order after
    /// the order has already filled; this must not trigger a halt.
    #[test]
    fn b14_late_ack_after_filled_is_noop() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::Ack, Some("ack-1")).unwrap();
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("fill-1"))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 100);

        // Late Ack with a fresh event_id arriving after the order is Filled.
        // Must be a silent noop — not a TransitionError/halt.
        o.apply(&OmsEvent::Ack, Some("ack-late")).unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "B14: late Ack on Filled must leave state unchanged"
        );
        assert_eq!(o.filled_qty, 100, "B14: filled_qty must not change");
    }

    /// B14-2: Ack on Cancelled (after partial fill + cancel) must also be noop.
    #[test]
    fn b14_late_ack_after_cancelled_is_noop() {
        let mut o = open_order();
        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        o.apply(&OmsEvent::CancelAck, Some("c2")).unwrap();
        assert_eq!(o.state, OrderState::Cancelled);

        // Late Ack on Cancelled — must be noop.
        o.apply(&OmsEvent::Ack, Some("ack-late")).unwrap();
        assert_eq!(
            o.state,
            OrderState::Cancelled,
            "B14: late Ack on Cancelled must leave state unchanged"
        );
        assert_eq!(
            o.filled_qty, 40,
            "B14: filled_qty must not change on late Ack after Cancelled"
        );
    }

    /// B14-3: Ack on Rejected must also be noop.
    #[test]
    fn b14_late_ack_after_rejected_is_noop() {
        let mut o = open_order();
        o.apply(&OmsEvent::Reject, Some("r1")).unwrap();
        assert_eq!(o.state, OrderState::Rejected);

        o.apply(&OmsEvent::Ack, Some("ack-late")).unwrap();
        assert_eq!(
            o.state,
            OrderState::Rejected,
            "B14: late Ack on Rejected must leave state unchanged"
        );
    }

    // -----------------------------------------------------------------------
    // C17 — Fill after CancelAck
    //
    // Fill after confirmed cancel IS an OMS inconsistency that warrants halt.
    // Unlike late fills on Filled (benign duplicate), a fill after CancelAck
    // means the broker filled an order it had already confirmed cancelling.
    // The state machine must return TransitionError so callers halt and alert.
    // -----------------------------------------------------------------------

    /// C17-1: Fill(new event_id) arriving after CancelAck must be TransitionError.
    /// This is NOT a noop — it is a genuine broker/OMS inconsistency.
    #[test]
    fn c17_fill_after_cancel_ack_is_transition_error() {
        let mut o = open_order(); // total_qty = 100
        o.apply(&OmsEvent::Ack, Some("ack-1")).unwrap();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        o.apply(&OmsEvent::CancelAck, Some("c2")).unwrap();
        assert_eq!(o.state, OrderState::Cancelled);

        // Fill arriving after confirmed cancel — must be TransitionError (halt).
        let err = o
            .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("fill-late"))
            .unwrap_err();
        assert_eq!(
            err.from,
            OrderState::Cancelled,
            "C17: TransitionError.from must be Cancelled"
        );
        // State and accounting must be unchanged after rejection.
        assert_eq!(
            o.state,
            OrderState::Cancelled,
            "C17: state must remain Cancelled — fill after cancel is an error"
        );
        assert_eq!(
            o.filled_qty, 0,
            "C17: filled_qty must not change on fill after CancelAck"
        );
    }

    /// C17-2: PartialFill after CancelAck is also TransitionError.
    #[test]
    fn c17_partial_fill_after_cancel_ack_is_transition_error() {
        let mut o = open_order();
        o.apply(&OmsEvent::PartialFill { delta_qty: 40 }, Some("f1"))
            .unwrap();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        o.apply(&OmsEvent::CancelAck, Some("c2")).unwrap();
        assert_eq!(o.state, OrderState::Cancelled);
        assert_eq!(o.filled_qty, 40);

        // PartialFill arriving after confirmed cancel — must be TransitionError.
        let err = o
            .apply(&OmsEvent::PartialFill { delta_qty: 10 }, Some("pf-late"))
            .unwrap_err();
        assert_eq!(err.from, OrderState::Cancelled);
        assert_eq!(
            o.state,
            OrderState::Cancelled,
            "C17: state must remain Cancelled"
        );
        assert_eq!(
            o.filled_qty, 40,
            "C17: filled_qty must not change on PartialFill after CancelAck"
        );
    }

    /// C17-3: Contrast with CancelPending — PartialFill DURING pending is still accepted.
    /// This confirms the C17 gate fires only after the cancel is CONFIRMED (CancelAck),
    /// not while still pending.
    #[test]
    fn c17_fill_during_cancel_pending_is_accepted() {
        let mut o = open_order();
        o.apply(&OmsEvent::CancelRequest, Some("c1")).unwrap();
        assert_eq!(o.state, OrderState::CancelPending);

        // Fill arrives before cancel is processed — still accepted.
        o.apply(&OmsEvent::Fill { delta_qty: 100 }, Some("f1"))
            .unwrap();
        assert_eq!(
            o.state,
            OrderState::Filled,
            "C17-contrast: fill during CancelPending is accepted (not yet confirmed cancel)"
        );
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

    // -----------------------------------------------------------------------
    // PAPER-SOAK-PARTIAL-FILL-DEDUP-02: apply_with_watermark
    // -----------------------------------------------------------------------

    /// Cross-lane duplicate: the SAME physical fill arrives twice with two
    /// DIFFERENT event ids (simulating WS then REST) but the SAME
    /// cum_qty_after. The second delivery must be a no-op even though its
    /// event_id has never been seen before — this is precisely the case
    /// `event_id`-only dedup (plain `apply`) cannot catch.
    #[test]
    fn watermark_cross_lane_duplicate_with_different_event_ids_is_noop() {
        let mut o = OmsOrder::new("ord-wm", "AAPL", 100);
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("ws-msg-1"),
            Some(10),
        )
        .unwrap();
        assert_eq!(o.filled_qty, 10);

        // REST redelivery: different event_id, same broker-confirmed cum_qty_after.
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("rest-activity-xyz"),
            Some(10),
        )
        .unwrap();
        assert_eq!(
            o.filled_qty, 10,
            "cross-lane duplicate with matching cum_qty_after must not double-apply"
        );
        assert_eq!(o.state, OrderState::PartiallyFilled);
    }

    /// Two legitimate, distinct partial fills with IDENTICAL delta_qty and
    /// price (only distinguishable by cum_qty_after) must BOTH apply. No
    /// time window is involved — this is the mandatory acceptance case the
    /// prior heuristic (PARTIAL_FILL_DEDUPE_WINDOW_MS) could wrongly collapse.
    #[test]
    fn watermark_two_distinct_same_size_fills_both_apply() {
        let mut o = OmsOrder::new("ord-wm2", "AAPL", 100);
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("f1"),
            Some(10),
        )
        .unwrap();
        assert_eq!(o.filled_qty, 10);

        // Second, genuinely distinct fill: same delta_qty as the first, but
        // cum_qty_after proves it is a new execution (10 -> 20, not 0 -> 10).
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("f2"),
            Some(20),
        )
        .unwrap();
        assert_eq!(
            o.filled_qty, 20,
            "two distinct fills with matching qty/price must both apply"
        );
    }

    /// When cum_qty_after proves a genuine advance, the applied delta is the
    /// broker-confirmed correction (target - filled_qty), not the event's own
    /// (possibly stale/mismatched) delta_qty.
    #[test]
    fn watermark_uses_corrected_delta_not_event_delta_qty() {
        let mut o = OmsOrder::new("ord-wm3", "AAPL", 100);
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("f1"),
            Some(10),
        )
        .unwrap();
        assert_eq!(o.filled_qty, 10);

        // Event claims delta_qty=999 (wrong/stale), but cum_qty_after=30 is
        // authoritative -> corrected delta is 30-10=20.
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 999 },
            Some("f2"),
            Some(30),
        )
        .unwrap();
        assert_eq!(
            o.filled_qty, 30,
            "applied quantity must follow cum_qty_after, not the event's own delta_qty"
        );
    }

    /// cum_qty_after == None behaves byte-for-byte like plain `apply` (pure
    /// event_id dedup) — confirms the fallback path is unweakened.
    #[test]
    fn watermark_none_falls_back_to_event_id_dedup_only() {
        let mut o = OmsOrder::new("ord-wm4", "AAPL", 100);
        o.apply_with_watermark(&OmsEvent::PartialFill { delta_qty: 10 }, Some("f1"), None)
            .unwrap();
        assert_eq!(o.filled_qty, 10);

        // Different event_id, no cum_qty_after -> cannot be recognized as a
        // duplicate, so (as with plain `apply` today) it applies again.
        o.apply_with_watermark(&OmsEvent::PartialFill { delta_qty: 10 }, Some("f2"), None)
            .unwrap();
        assert_eq!(o.filled_qty, 20);

        // Same event_id -> id-based dedup still short-circuits.
        o.apply_with_watermark(&OmsEvent::PartialFill { delta_qty: 10 }, Some("f2"), None)
            .unwrap();
        assert_eq!(o.filled_qty, 20, "same event_id retry must remain a no-op");
    }

    /// A watermark-proven duplicate on a terminal Fill is also a no-op, and
    /// does not disturb the Filled state.
    #[test]
    fn watermark_duplicate_terminal_fill_is_noop() {
        let mut o = OmsOrder::new("ord-wm5", "AAPL", 10);
        o.apply_with_watermark(&OmsEvent::Fill { delta_qty: 10 }, Some("f1"), Some(10))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 10);

        o.apply_with_watermark(&OmsEvent::Fill { delta_qty: 10 }, Some("f2"), Some(10))
            .unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert_eq!(o.filled_qty, 10);
    }

    /// Concurrent-arrival order independence: whichever lane's row is
    /// processed FIRST in canonical apply order wins the real application;
    /// the second is always a no-op regardless of which lane it came from
    /// (proves A1 vs A2 symmetry at the OMS layer).
    #[test]
    fn watermark_apply_order_symmetric_rest_then_ws() {
        let mut o = OmsOrder::new("ord-wm6", "AAPL", 100);
        // REST processed first this time.
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("rest-activity-xyz"),
            Some(10),
        )
        .unwrap();
        assert_eq!(o.filled_qty, 10);
        // WS redelivery second.
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("ws-msg-1"),
            Some(10),
        )
        .unwrap();
        assert_eq!(
            o.filled_qty, 10,
            "REST-then-WS duplicate must not double-apply"
        );
    }

    /// Self-review regression: a watermark-carrying duplicate fill arriving
    /// AFTER a confirmed cancel must still halt (TransitionError), exactly
    /// as C17 (`c17_partial_fill_after_cancel_ack_is_transition_error`)
    /// requires for the no-watermark path. The watermark fast path exists to
    /// recognize benign cross-lane duplicates on a still-live order, not to
    /// swallow a late fill redelivered after the order's lifecycle already
    /// concluded — that anomaly must keep reaching an operator, not become a
    /// silent no-op just because its cum_qty_after happens to already be
    /// reflected in filled_qty.
    #[test]
    fn watermark_duplicate_after_cancel_ack_is_still_transition_error() {
        let mut o = OmsOrder::new("ord-wm7", "AAPL", 100);
        o.apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 40 },
            Some("f1"),
            Some(40),
        )
        .unwrap();
        o.apply_with_watermark(&OmsEvent::CancelRequest, Some("c1"), None)
            .unwrap();
        o.apply_with_watermark(&OmsEvent::CancelAck, Some("c2"), None)
            .unwrap();
        assert_eq!(o.state, OrderState::Cancelled);
        assert_eq!(o.filled_qty, 40);

        // Cross-lane duplicate of the SAME already-applied 40-share fill,
        // now arriving after the cancel was confirmed. cum_qty_after=40 is
        // <= filled_qty=40 (the watermark condition that is a benign no-op
        // on a LIVE order) but the order is Cancelled — must still error.
        let err = o
            .apply_with_watermark(
                &OmsEvent::PartialFill { delta_qty: 40 },
                Some("rest-late-dup"),
                Some(40),
            )
            .unwrap_err();
        assert_eq!(err.from, OrderState::Cancelled);
        assert_eq!(
            o.state,
            OrderState::Cancelled,
            "state must remain Cancelled, not silently swallowed"
        );
        assert_eq!(o.filled_qty, 40, "filled_qty must not change");
    }

    /// A7 (MANDATORY): concurrent WS + REST delivery of the SAME physical
    /// partial fill, racing on two real OS threads synchronized with a
    /// `std::sync::Barrier` (not a sleep), must produce exactly one economic
    /// application. This proves `apply_with_watermark` is safe when both
    /// deliveries reach the shared order state at genuinely the same instant
    /// — the harder case than sequential delivery, since without external
    /// serialization both threads could observe `filled_qty` pre-advance
    /// simultaneously.
    ///
    /// The order is wrapped in `Mutex` because `OmsOrder::apply_with_watermark`
    /// takes `&mut self` — this mirrors production, where the canonical apply
    /// queue processes one order's events under a single lock
    /// (`oms_orders: &mut BTreeMap<...>`) rather than truly lock-free
    /// concurrent mutation; the Barrier forces both threads to attempt entry
    /// at the same instant so whichever wins the lock is not deterministic,
    /// proving the watermark — not favorable scheduling — is what prevents
    /// the double-apply.
    #[test]
    fn a7_concurrent_ws_and_rest_duplicate_applies_exactly_once() {
        use std::sync::{Arc, Barrier, Mutex};

        let order = Arc::new(Mutex::new(OmsOrder::new("ord-a7", "AAPL", 100)));
        let barrier = Arc::new(Barrier::new(2));

        let run = |order: Arc<Mutex<OmsOrder>>, barrier: Arc<Barrier>, event_id: &'static str| {
            std::thread::spawn(move || {
                barrier.wait();
                let mut guard = order.lock().unwrap();
                guard
                    .apply_with_watermark(
                        &OmsEvent::PartialFill { delta_qty: 10 },
                        Some(event_id),
                        Some(10),
                    )
                    .unwrap();
            })
        };

        let ws_handle = run(order.clone(), barrier.clone(), "ws-msg-1");
        let rest_handle = run(order.clone(), barrier.clone(), "rest-activity-xyz");

        ws_handle.join().unwrap();
        rest_handle.join().unwrap();

        let final_order = order.lock().unwrap();
        assert_eq!(
            final_order.filled_qty, 10,
            "A7: concurrent WS+REST delivery of the same physical fill must apply exactly once, \
             not 0 (lost) or 20 (double-applied)"
        );
    }
}
