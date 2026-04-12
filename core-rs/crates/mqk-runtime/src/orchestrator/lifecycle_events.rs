//! EXEC-02: Lifecycle event row construction for cancel/replace events.
//!
//! Pure helper that maps a non-fill `BrokerEvent` to a
//! `NewOrderLifecycleEvent` row.  Fill events return `None`.
//!
//! Four event kinds are captured:
//! - `CancelAck`     → operation = `"cancel_ack"`
//! - `ReplaceAck`    → operation = `"replace_ack"` (carries new_total_qty)
//! - `CancelReject`  → operation = `"cancel_reject"`
//! - `ReplaceReject` → operation = `"replace_reject"`
//!
//! All other event kinds return `None` — no fabrication.
//!
//! # Exports
//!
//! - `build_lifecycle_event_row` — produce a lifecycle row for a cancel/replace event (pure).

use mqk_execution::BrokerEvent;
use sqlx::types::chrono;
use uuid::Uuid;

/// Build a `NewOrderLifecycleEvent` row for a cancel or replace event.
///
/// Returns `None` for Fill, PartialFill, Ack, and Reject events —
/// those are not lifecycle chain events.
pub(super) fn build_lifecycle_event_row(
    run_id: Uuid,
    broker_message_id: &str,
    event: &BrokerEvent,
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Option<mqk_db::NewOrderLifecycleEvent> {
    let (internal_order_id, operation, broker_order_id, new_total_qty) = match event {
        BrokerEvent::CancelAck {
            internal_order_id,
            broker_order_id,
            ..
        } => (
            internal_order_id.as_str(),
            "cancel_ack",
            broker_order_id.as_deref(),
            None,
        ),
        BrokerEvent::ReplaceAck {
            internal_order_id,
            broker_order_id,
            new_total_qty,
            ..
        } => (
            internal_order_id.as_str(),
            "replace_ack",
            broker_order_id.as_deref(),
            Some(*new_total_qty),
        ),
        BrokerEvent::CancelReject {
            internal_order_id,
            broker_order_id,
            ..
        } => (
            internal_order_id.as_str(),
            "cancel_reject",
            broker_order_id.as_deref(),
            None,
        ),
        BrokerEvent::ReplaceReject {
            internal_order_id,
            broker_order_id,
            ..
        } => (
            internal_order_id.as_str(),
            "replace_reject",
            broker_order_id.as_deref(),
            None,
        ),
        // Fill, PartialFill, Ack, Reject — not lifecycle chain events.
        _ => return None,
    };

    Some(mqk_db::NewOrderLifecycleEvent {
        event_id: broker_message_id.to_string(),
        run_id,
        internal_order_id: internal_order_id.to_string(),
        operation: operation.to_string(),
        broker_order_id: broker_order_id.map(|s| s.to_string()),
        new_total_qty,
        recorded_at_utc: now_utc,
    })
}

#[cfg(test)]
mod tests {
    use super::build_lifecycle_event_row;
    use mqk_execution::BrokerEvent;
    use uuid::Uuid;

    fn fixed_ts() -> chrono::DateTime<chrono::Utc> {
        use sqlx::types::chrono;
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    // L01: CancelAck → operation="cancel_ack", new_total_qty=None
    #[test]
    fn l01_cancel_ack_maps_to_cancel_ack_op() {
        let ev = BrokerEvent::CancelAck {
            broker_message_id: "msg-cancel-1".to_string(),
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("brk-abc".to_string()),
        };
        let row = build_lifecycle_event_row(Uuid::nil(), "msg-cancel-1", &ev, fixed_ts())
            .expect("CancelAck must produce a row");
        assert_eq!(row.operation, "cancel_ack");
        assert_eq!(row.event_id, "msg-cancel-1");
        assert_eq!(row.internal_order_id, "ord-1");
        assert_eq!(row.broker_order_id.as_deref(), Some("brk-abc"));
        assert!(row.new_total_qty.is_none(), "cancel_ack must not carry new_total_qty");
    }

    // L02: ReplaceAck → operation="replace_ack", new_total_qty populated
    #[test]
    fn l02_replace_ack_maps_to_replace_ack_op_with_qty() {
        let ev = BrokerEvent::ReplaceAck {
            broker_message_id: "msg-replace-1".to_string(),
            internal_order_id: "ord-2".to_string(),
            broker_order_id: None,
            new_total_qty: 75,
        };
        let row = build_lifecycle_event_row(Uuid::nil(), "msg-replace-1", &ev, fixed_ts())
            .expect("ReplaceAck must produce a row");
        assert_eq!(row.operation, "replace_ack");
        assert_eq!(row.new_total_qty, Some(75));
        assert!(row.broker_order_id.is_none());
    }

    // L03: CancelReject → operation="cancel_reject"
    #[test]
    fn l03_cancel_reject_maps_correctly() {
        let ev = BrokerEvent::CancelReject {
            broker_message_id: "msg-creject-1".to_string(),
            internal_order_id: "ord-3".to_string(),
            broker_order_id: None,
        };
        let row = build_lifecycle_event_row(Uuid::nil(), "msg-creject-1", &ev, fixed_ts())
            .expect("CancelReject must produce a row");
        assert_eq!(row.operation, "cancel_reject");
    }

    // L04: ReplaceReject → operation="replace_reject"
    #[test]
    fn l04_replace_reject_maps_correctly() {
        let ev = BrokerEvent::ReplaceReject {
            broker_message_id: "msg-rreject-1".to_string(),
            internal_order_id: "ord-4".to_string(),
            broker_order_id: None,
        };
        let row = build_lifecycle_event_row(Uuid::nil(), "msg-rreject-1", &ev, fixed_ts())
            .expect("ReplaceReject must produce a row");
        assert_eq!(row.operation, "replace_reject");
    }

    // L05: Fill, PartialFill, Ack, Reject all return None
    #[test]
    fn l05_non_lifecycle_events_return_none() {
        let cases: &[BrokerEvent] = &[
            BrokerEvent::Ack {
                broker_message_id: "m1".to_string(),
                internal_order_id: "o1".to_string(),
                broker_order_id: None,
            },
            BrokerEvent::Fill {
                broker_message_id: "m2".to_string(),
                broker_fill_id: None,
                internal_order_id: "o1".to_string(),
                broker_order_id: None,
                symbol: "SPY".to_string(),
                side: mqk_execution::types::Side::Buy,
                delta_qty: 10,
                price_micros: 100_000_000,
                fee_micros: 0,
            },
            BrokerEvent::PartialFill {
                broker_message_id: "m3".to_string(),
                broker_fill_id: None,
                internal_order_id: "o1".to_string(),
                broker_order_id: None,
                symbol: "SPY".to_string(),
                side: mqk_execution::types::Side::Buy,
                delta_qty: 5,
                price_micros: 100_000_000,
                fee_micros: 0,
            },
            BrokerEvent::Reject {
                broker_message_id: "m4".to_string(),
                internal_order_id: "o1".to_string(),
                broker_order_id: None,
            },
        ];
        for ev in cases {
            assert!(
                build_lifecycle_event_row(Uuid::nil(), "m", ev, fixed_ts()).is_none(),
                "non-lifecycle event must return None: {:?}",
                ev.broker_message_id()
            );
        }
    }
}
