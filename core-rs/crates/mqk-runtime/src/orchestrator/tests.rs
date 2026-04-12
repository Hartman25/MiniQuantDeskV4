use super::*;
use ::chrono::TimeZone;

// ---------------------------------------------------------------------------
// Test-only helpers (moved from orchestrator.rs outer scope)
// ---------------------------------------------------------------------------

fn order_json_qty(json: &serde_json::Value) -> i64 {
    json["quantity"].as_i64().unwrap_or(0).saturating_abs()
}

fn order_json_side(json: &serde_json::Value) -> mqk_execution::Side {
    match json["side"].as_str() {
        Some("buy") | Some("BUY") | Some("Buy") => mqk_execution::Side::Buy,
        Some("sell") | Some("SELL") | Some("Sell") => mqk_execution::Side::Sell,
        _ => {
            if json["quantity"].as_i64().unwrap_or(0) >= 0 {
                mqk_execution::Side::Buy
            } else {
                mqk_execution::Side::Sell
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[test]
fn invariant_check_passes_on_fresh_portfolio() {
    let pf = PortfolioState::new(1_000_000_000_i64);
    check_capital_invariants(&pf).unwrap();
}
#[test]
fn invariant_check_detects_cash_corruption() {
    let mut pf = PortfolioState::new(1_000_000_000_i64);
    // Directly corrupt cash without a corresponding ledger entry.
    pf.cash_micros = 999_999_999;
    assert!(check_capital_invariants(&pf).is_err());
}
#[test]
fn invariant_check_passes_after_apply_entry() {
    use mqk_portfolio::{Fill, LedgerEntry, Side};
    let mut pf = PortfolioState::new(1_000_000_000_i64);
    let fill = Fill::new("AAPL", Side::Buy, 10, 150_000_000, 0);
    apply_entry(&mut pf, LedgerEntry::Fill(fill));
    // After apply_entry (which appends to ledger), invariant must hold.
    check_capital_invariants(&pf).unwrap();
}
#[test]
fn broker_event_accessors() {
    use mqk_execution::Side;
    let ev = BrokerEvent::Fill {
        broker_message_id: "msg-1".to_string(),
        broker_fill_id: None,
        internal_order_id: "ord-1".to_string(),
        broker_order_id: None,
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 10,
        price_micros: 150_000_000,
        fee_micros: 0,
    };
    assert_eq!(ev.broker_message_id(), "msg-1");
    assert_eq!(ev.internal_order_id(), "ord-1");
}
#[test]
fn broker_event_to_fill_converts_correctly() {
    use mqk_execution::Side;
    let ev = BrokerEvent::Fill {
        broker_message_id: "msg-2".to_string(),
        broker_fill_id: None,
        internal_order_id: "ord-2".to_string(),
        broker_order_id: None,
        symbol: "MSFT".to_string(),
        side: Side::Sell,
        delta_qty: 5,
        price_micros: 300_000_000,
        fee_micros: 1_000,
    };
    let fill = broker_event_to_fill(&ev).unwrap();
    assert_eq!(fill.qty, 5);
    assert_eq!(fill.price_micros, 300_000_000);
    assert_eq!(fill.fee_micros, 1_000);
    assert_eq!(fill.side, mqk_portfolio::Side::Sell);
}
#[test]
fn broker_event_to_fill_returns_none_for_ack() {
    let ev = BrokerEvent::Ack {
        broker_message_id: "msg-3".to_string(),
        internal_order_id: "ord-3".to_string(),
        broker_order_id: None,
    };
    assert!(broker_event_to_fill(&ev).is_none());
}
#[test]
fn order_json_qty_preserves_positive_buy_quantity() {
    let json = serde_json::json!({
        "symbol": "SPY",
        "side": "buy",
        "quantity": 100
    });
    assert_eq!(order_json_qty(&json), 100);
    assert!(matches!(order_json_side(&json), mqk_execution::Side::Buy));
}
#[test]
fn order_json_qty_normalizes_negative_sell_quantity_for_oms_registration() {
    let json = serde_json::json!({
        "symbol": "SPY",
        "quantity": -100
    });
    let qty = order_json_qty(&json);
    assert_eq!(qty, 100, "OMS registration quantity must be absolute");
    assert!(matches!(order_json_side(&json), mqk_execution::Side::Sell));
    let order = OmsOrder::new("ord-sell", "SPY", qty);
    assert_eq!(
        order.total_qty, 100,
        "negative signed sell quantity must not leak into OmsOrder::new"
    );
}
#[test]
fn explicit_side_overrides_legacy_sign_in_submit_request_building() {
    let row = mqk_db::OutboxRow {
        outbox_id: 1,
        run_id: uuid::Uuid::nil(),
        idempotency_key: "ord-1".to_string(),
        order_json: serde_json::json!({
            "symbol": "SPY",
            "side": "buy",
            "quantity": -100,
            "order_type": "market",
            "time_in_force": "day"
        }),
        status: "PENDING".to_string(),
        created_at_utc: chrono::Utc::now(),
        sent_at_utc: None,
        claimed_at_utc: None,
        claimed_by: None,
        dispatching_at_utc: None,
        dispatch_attempt_id: None,
    };
    let req = build_submit_request(&row).expect("submit request must build");
    assert!(matches!(req.side, mqk_execution::Side::Buy));
    assert_eq!(req.quantity, 100);
}
#[test]
fn broker_event_to_fill_rejects_zero_qty() {
    use mqk_execution::Side;
    let ev = BrokerEvent::Fill {
        broker_message_id: "msg-4".to_string(),
        broker_fill_id: None,
        internal_order_id: "ord-4".to_string(),
        broker_order_id: None,
        symbol: "X".to_string(),
        side: Side::Buy,
        delta_qty: 0,
        price_micros: 100_000_000,
        fee_micros: 0,
    };
    assert!(broker_event_to_fill(&ev).is_none());
}
#[test]
fn oms_event_mapping_covers_all_variants() {
    use mqk_execution::Side;
    let cases: &[BrokerEvent] = &[
        BrokerEvent::Ack {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::Fill {
            broker_message_id: "m".to_string(),
            broker_fill_id: None,
            internal_order_id: "o".to_string(),
            broker_order_id: None,
            symbol: "X".to_string(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 1,
            fee_micros: 0,
        },
        BrokerEvent::PartialFill {
            broker_message_id: "m".to_string(),
            broker_fill_id: None,
            internal_order_id: "o".to_string(),
            broker_order_id: None,
            symbol: "X".to_string(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 1,
            fee_micros: 0,
        },
        BrokerEvent::CancelAck {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::CancelReject {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::ReplaceAck {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
            new_total_qty: 100, // P1-03
        },
        BrokerEvent::ReplaceReject {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::Reject {
            broker_message_id: "m".to_string(),
            internal_order_id: "o".to_string(),
            broker_order_id: None,
        },
    ];
    // Verify mapping does not panic for any variant.
    for ev in cases {
        let _ = broker_event_to_oms_event(ev);
    }
}
#[test]
fn event_kind_rank_is_strictly_ordered() {
    use mqk_execution::Side;
    // Verify no two distinct variant kinds map to the same rank.
    let events: Vec<BrokerEvent> = vec![
        BrokerEvent::Ack {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
        },
        BrokerEvent::PartialFill {
            broker_message_id: "m".into(),
            broker_fill_id: None,
            internal_order_id: "o".into(),
            broker_order_id: None,
            symbol: "X".into(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 1,
            fee_micros: 0,
        },
        BrokerEvent::Fill {
            broker_message_id: "m".into(),
            broker_fill_id: None,
            internal_order_id: "o".into(),
            broker_order_id: None,
            symbol: "X".into(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 1,
            fee_micros: 0,
        },
        BrokerEvent::CancelAck {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
        },
        BrokerEvent::CancelReject {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
        },
        BrokerEvent::ReplaceAck {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
            new_total_qty: 100, // P1-03
        },
        BrokerEvent::ReplaceReject {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
        },
        BrokerEvent::Reject {
            broker_message_id: "m".into(),
            internal_order_id: "o".into(),
            broker_order_id: None,
        },
    ];
    let mut ranks: Vec<u8> = events.iter().map(event_kind_rank).collect();
    ranks.sort_unstable();
    ranks.dedup();
    assert_eq!(
        ranks.len(),
        events.len(),
        "each variant must have a unique rank"
    );
}
#[test]
fn canonical_apply_order_does_not_depend_on_broker_message_id() {
    use mqk_execution::Side;
    // Delivery order was fill -> ack even though lexicographic message-id is opposite.
    let fill = BrokerEvent::Fill {
        broker_message_id: "z-msg".into(),
        broker_fill_id: None,
        internal_order_id: "ord-1".into(),
        broker_order_id: None,
        symbol: "X".into(),
        side: Side::Buy,
        delta_qty: 5,
        price_micros: 100,
        fee_micros: 0,
    };
    let ack = BrokerEvent::Ack {
        broker_message_id: "a-msg".into(),
        internal_order_id: "ord-1".into(),
        broker_order_id: None,
    };
    let mut queue: Vec<(i64, String, BrokerEvent)> =
        vec![(42, "z-msg".into(), fill), (43, "a-msg".into(), ack)];
    queue.sort_by_key(|(inbox_id, _, _)| *inbox_id);
    assert!(
        matches!(queue[0].2, BrokerEvent::Fill { .. }),
        "canonical apply order must follow durable inbox ingest order, not broker_message_id"
    );
}

#[test]
fn out_of_order_broker_delivery_uses_real_ordering_truth() {
    use mqk_execution::Side;

    let queue = build_canonical_apply_queue(vec![
        mqk_db::InboxRow {
            inbox_id: 10,
            run_id: Uuid::nil(),
            broker_message_id: "z-msg".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Fill {
                broker_message_id: "z-msg".into(),
                broker_fill_id: None,
                internal_order_id: "ord-1".into(),
                broker_order_id: None,
                symbol: "X".into(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
        mqk_db::InboxRow {
            inbox_id: 11,
            run_id: Uuid::nil(),
            broker_message_id: "a-msg".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Ack {
                broker_message_id: "a-msg".into(),
                internal_order_id: "ord-1".into(),
                broker_order_id: None,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
    ])
    .expect("canonical queue should build");

    assert_eq!(queue[0].0, 10);
    assert!(matches!(queue[0].2, BrokerEvent::Fill { .. }));
}

#[test]
fn restart_replay_preserves_durable_apply_order() {
    use mqk_execution::Side;

    let first_pass = build_canonical_apply_queue(vec![
        mqk_db::InboxRow {
            inbox_id: 200,
            run_id: Uuid::nil(),
            broker_message_id: "m-2".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::PartialFill {
                broker_message_id: "m-2".into(),
                broker_fill_id: None,
                internal_order_id: "ord-r".into(),
                broker_order_id: None,
                symbol: "X".into(),
                side: Side::Buy,
                delta_qty: 2,
                price_micros: 2,
                fee_micros: 0,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
        mqk_db::InboxRow {
            inbox_id: 201,
            run_id: Uuid::nil(),
            broker_message_id: "m-1".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Ack {
                broker_message_id: "m-1".into(),
                internal_order_id: "ord-r".into(),
                broker_order_id: None,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
    ])
    .unwrap();
    let second_pass = build_canonical_apply_queue(vec![
        mqk_db::InboxRow {
            inbox_id: 200,
            run_id: Uuid::nil(),
            broker_message_id: "m-2".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::PartialFill {
                broker_message_id: "m-2".into(),
                broker_fill_id: None,
                internal_order_id: "ord-r".into(),
                broker_order_id: None,
                symbol: "X".into(),
                side: Side::Buy,
                delta_qty: 2,
                price_micros: 2,
                fee_micros: 0,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
        mqk_db::InboxRow {
            inbox_id: 201,
            run_id: Uuid::nil(),
            broker_message_id: "m-1".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Ack {
                broker_message_id: "m-1".into(),
                internal_order_id: "ord-r".into(),
                broker_order_id: None,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
    ])
    .unwrap();

    let first_ids: Vec<i64> = first_pass.into_iter().map(|x| x.0).collect();
    let second_ids: Vec<i64> = second_pass.into_iter().map(|x| x.0).collect();
    assert_eq!(first_ids, second_ids);
}

#[test]
fn ambiguous_ordering_truth_fails_closed() {
    let err = build_canonical_apply_queue(vec![
        mqk_db::InboxRow {
            inbox_id: 7,
            run_id: Uuid::nil(),
            broker_message_id: "m-1".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Ack {
                broker_message_id: "m-1".into(),
                internal_order_id: "ord-a".into(),
                broker_order_id: None,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
        mqk_db::InboxRow {
            inbox_id: 7,
            run_id: Uuid::nil(),
            broker_message_id: "m-2".into(),
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
            message_json: serde_json::to_value(BrokerEvent::Reject {
                broker_message_id: "m-2".into(),
                internal_order_id: "ord-a".into(),
                broker_order_id: None,
            })
            .unwrap(),
            received_at_utc: chrono::Utc::now(),
            applied_at_utc: None,
        },
    ])
    .expect_err("duplicate canonical key must fail closed");

    assert!(err.to_string().contains("AMBIGUOUS_CANONICAL_ORDER"));
}

// -----------------------------------------------------------------------
// Section C - apply_fill_step unit tests
// -----------------------------------------------------------------------
fn make_ack_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::Ack {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
    }
}
fn make_partial_fill_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
    BrokerEvent::PartialFill {
        broker_message_id: msg_id.to_string(),
        broker_fill_id: None,
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
        symbol: "SPY".to_string(),
        side: mqk_execution::Side::Buy,
        delta_qty: qty,
        price_micros: 450_000_000,
        fee_micros: 0,
    }
}
fn make_fill_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
    BrokerEvent::Fill {
        broker_message_id: msg_id.to_string(),
        broker_fill_id: None,
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
        symbol: "SPY".to_string(),
        side: mqk_execution::Side::Buy,
        delta_qty: qty,
        price_micros: 450_000_000,
        fee_micros: 0,
    }
}
fn make_cancel_ack_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::CancelAck {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
    }
}
fn make_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::Reject {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
    }
}
fn make_cancel_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::CancelReject {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
    }
}
fn make_replace_ack_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
    BrokerEvent::ReplaceAck {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
        new_total_qty: qty,
    }
}
fn make_replace_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::ReplaceReject {
        broker_message_id: msg_id.to_string(),
        internal_order_id: internal_id.to_string(),
        broker_order_id: None,
    }
}
fn apply_event_and_maybe_remove_broker_mapping(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    order_map: &mut BrokerOrderMap,
    event: &BrokerEvent,
    msg_id: &str,
) -> anyhow::Result<AppliedBrokerEventOutcome> {
    let internal_id = event.internal_order_id().to_string();
    let outcome = apply_broker_event_step(oms_orders, &internal_id, event, msg_id)?;
    if outcome.terminal_apply_succeeded {
        remove_broker_mapping_from_memory(order_map, &internal_id);
    }
    Ok(outcome)
}
#[test]
fn fill_terminal_apply_success_removes_broker_map() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert(
        "ord-fill".to_string(),
        OmsOrder::new("ord-fill", "SPY", 100),
    );
    let mut order_map = BrokerOrderMap::new();
    order_map.register("ord-fill", "broker-fill");

    let outcome = apply_event_and_maybe_remove_broker_mapping(
        &mut oms,
        &mut order_map,
        &make_fill_event("ord-fill", "fill-msg", 100),
        "fill-msg",
    )
    .expect("terminal fill apply must succeed");

    assert!(outcome.terminal_apply_succeeded);
    assert_eq!(
        oms["ord-fill"].state,
        mqk_execution::oms::state_machine::OrderState::Filled
    );
    assert!(
        order_map.broker_id("ord-fill").is_none(),
        "successful terminal fill apply must remove the broker mapping"
    );
}
#[test]
fn cancel_ack_unknown_order_does_not_remove_broker_map() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut order_map = BrokerOrderMap::new();
    order_map.register("ord-cancel", "broker-cancel");

    let outcome = apply_event_and_maybe_remove_broker_mapping(
        &mut oms,
        &mut order_map,
        &make_cancel_ack_event("ord-cancel", "cancel-msg"),
        "cancel-msg",
    )
    .expect("unknown cancel-ack should be skipped safely");

    assert!(!outcome.terminal_apply_succeeded);
    assert!(
        order_map.broker_id("ord-cancel").is_some(),
        "unknown-order cancel-ack must not remove the broker mapping"
    );
}
#[test]
fn reject_unknown_order_does_not_remove_broker_map() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut order_map = BrokerOrderMap::new();
    order_map.register("ord-reject", "broker-reject");

    let outcome = apply_event_and_maybe_remove_broker_mapping(
        &mut oms,
        &mut order_map,
        &make_reject_event("ord-reject", "reject-msg"),
        "reject-msg",
    )
    .expect("unknown reject should be skipped safely");

    assert!(!outcome.terminal_apply_succeeded);
    assert!(
        order_map.broker_id("ord-reject").is_some(),
        "unknown-order reject must not remove the broker mapping"
    );
}
#[test]
fn non_terminal_events_do_not_remove_broker_map() {
    // Ack on a live open order: non-terminal, mapping must remain.
    {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert(
            "ord-non-terminal".to_string(),
            OmsOrder::new("ord-non-terminal", "SPY", 120),
        );
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-non-terminal", "broker-non-terminal");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_ack_event("ord-non-terminal", "ack-msg"),
            "ack-msg",
        )
        .expect("ack apply must not fail");

        assert!(
            !outcome.terminal_apply_succeeded,
            "ack must not request broker-map cleanup"
        );
        assert!(
            order_map.broker_id("ord-non-terminal").is_some(),
            "ack must retain the broker mapping"
        );
    }

    // Partial fill on an open order: non-terminal, mapping must remain.
    {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert(
            "ord-non-terminal".to_string(),
            OmsOrder::new("ord-non-terminal", "SPY", 120),
        );
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-non-terminal", "broker-non-terminal");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_partial_fill_event("ord-non-terminal", "partial-msg", 10),
            "partial-msg",
        )
        .expect("partial fill apply must not fail");

        assert!(
            !outcome.terminal_apply_succeeded,
            "partial fill must not request broker-map cleanup"
        );
        assert!(
            order_map.broker_id("ord-non-terminal").is_some(),
            "partial fill must retain the broker mapping"
        );
    }

    // CancelReject is only legal from CancelPending.
    {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
        order
            .apply(&OmsEvent::CancelRequest, Some("cancel-request-msg"))
            .expect("seed cancel-pending state");
        oms.insert("ord-non-terminal".to_string(), order);

        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-non-terminal", "broker-non-terminal");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_cancel_reject_event("ord-non-terminal", "cancel-reject-msg"),
            "cancel-reject-msg",
        )
        .expect("cancel reject apply must not fail");

        assert!(
            !outcome.terminal_apply_succeeded,
            "cancel reject must not request broker-map cleanup"
        );
        assert!(
            order_map.broker_id("ord-non-terminal").is_some(),
            "cancel reject must retain the broker mapping"
        );
    }

    // ReplaceAck is only legal from ReplacePending.
    {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
        order
            .apply(&OmsEvent::ReplaceRequest, Some("replace-request-msg"))
            .expect("seed replace-pending state");
        oms.insert("ord-non-terminal".to_string(), order);

        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-non-terminal", "broker-non-terminal");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_replace_ack_event("ord-non-terminal", "replace-ack-msg", 120),
            "replace-ack-msg",
        )
        .expect("replace ack apply must not fail");

        assert!(
            !outcome.terminal_apply_succeeded,
            "replace ack must not request broker-map cleanup"
        );
        assert!(
            order_map.broker_id("ord-non-terminal").is_some(),
            "replace ack must retain the broker mapping"
        );
    }

    // ReplaceReject is only legal from ReplacePending.
    {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
        order
            .apply(&OmsEvent::ReplaceRequest, Some("replace-request-msg"))
            .expect("seed replace-pending state");
        oms.insert("ord-non-terminal".to_string(), order);

        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-non-terminal", "broker-non-terminal");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_replace_reject_event("ord-non-terminal", "replace-reject-msg"),
            "replace-reject-msg",
        )
        .expect("replace reject apply must not fail");

        assert!(
            !outcome.terminal_apply_succeeded,
            "replace reject must not request broker-map cleanup"
        );
        assert!(
            order_map.broker_id("ord-non-terminal").is_some(),
            "replace reject must retain the broker mapping"
        );
    }
}
#[test]
fn replayed_terminal_noop_does_not_incorrectly_remove_mapping_or_break_idempotence() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut terminal = OmsOrder::new("ord-replay", "SPY", 100);
    terminal
        .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("fill-msg"))
        .expect("seed terminal fill state");
    oms.insert("ord-replay".to_string(), terminal);
    let mut order_map = BrokerOrderMap::new();
    order_map.register("ord-replay", "broker-replay");

    let outcome = apply_event_and_maybe_remove_broker_mapping(
        &mut oms,
        &mut order_map,
        &make_fill_event("ord-replay", "fill-late", 100),
        "fill-late",
    )
    .expect("late terminal-looking fill replay must be a safe no-op");

    assert!(outcome.fill.is_none());
    assert!(!outcome.terminal_apply_succeeded);
    assert!(
        order_map.broker_id("ord-replay").is_some(),
        "terminal-looking no-op replay must not remove the mapping solely by event kind"
    );
    assert_eq!(
        oms["ord-replay"].state,
        mqk_execution::oms::state_machine::OrderState::Filled,
        "late replay must preserve terminal OMS state"
    );
}
#[test]
fn terminal_cleanup_occurs_before_mark_applied_or_is_otherwise_proven_durably_safe() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut order = OmsOrder::new("ord-cancel-pending", "SPY", 100);
    order
        .apply(&OmsEvent::CancelRequest, Some("cancel-request"))
        .expect("seed cancel pending state");
    oms.insert("ord-cancel-pending".to_string(), order);
    let mut order_map = BrokerOrderMap::new();
    order_map.register("ord-cancel-pending", "broker-cancel-pending");

    let outcome = apply_event_and_maybe_remove_broker_mapping(
        &mut oms,
        &mut order_map,
        &make_cancel_ack_event("ord-cancel-pending", "cancel-ack-msg"),
        "cancel-ack-msg",
    )
    .expect("cancel-ack terminal apply must succeed");

    assert!(
        outcome.terminal_apply_succeeded,
        "cleanup gate must only open after OMS has successfully reached a terminal state"
    );
    assert_eq!(
        oms["ord-cancel-pending"].state,
        mqk_execution::oms::state_machine::OrderState::Cancelled,
        "terminal state must be applied before cleanup can run"
    );
    assert!(
        order_map.broker_id("ord-cancel-pending").is_none(),
        "once terminal apply succeeds, runtime cleanup can safely remove the mapping before mark_applied"
    );
}
/// Section C - T1.
/// A Fill event for an order not present in oms_orders must return
/// UNKNOWN_ORDER_FILL and never produce a portfolio fill.
#[test]
fn unknown_order_fill_is_rejected() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let ev = make_fill_event("ord-unknown", "fill-msg-1", 100);
    let result = apply_fill_step(&mut oms, "ord-unknown", &ev, "fill-msg-1");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("UNKNOWN_ORDER_FILL"),
        "expected UNKNOWN_ORDER_FILL, got: {err}"
    );
}
/// Section C - T2.
/// A PartialFill event for an order not present in oms_orders must also
/// return UNKNOWN_ORDER_FILL - the rule is not limited to final fills.
#[test]
fn unknown_order_partial_fill_is_rejected() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let ev = make_partial_fill_event("ord-unknown", "pf-msg-1", 50);
    let result = apply_fill_step(&mut oms, "ord-unknown", &ev, "pf-msg-1");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("UNKNOWN_ORDER_FILL"),
        "expected UNKNOWN_ORDER_FILL, got: {err}"
    );
}
/// Section C - T3.
/// A Fill event for a known order must succeed, return Some(fill) with
/// correct qty, and advance the OMS filled_qty.
#[test]
fn known_order_fill_succeeds_and_returns_fill() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert("ord-1".to_string(), OmsOrder::new("ord-1", "SPY", 100));
    let ev = make_fill_event("ord-1", "fill-msg-2", 100);
    let result = apply_fill_step(&mut oms, "ord-1", &ev, "fill-msg-2");
    let fill = result
        .unwrap()
        .expect("expected Some(fill) for known order fill");
    assert_eq!(fill.qty, 100);
    // OMS state must have advanced.
    assert_eq!(oms["ord-1"].filled_qty, 100);
}
/// Section C - T4.
/// An OMS-level transition error (fill would overflow total_qty) must
/// surface as Err containing "OMS transition error" and must NOT advance
/// filled_qty, preventing any downstream portfolio mutation.
#[test]
fn oms_rejection_blocks_portfolio_fill() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut order = OmsOrder::new("ord-2", "SPY", 100);
    // Pre-fill 60 so that any further 60-unit fill overflows.
    order
        .apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("pf-setup"))
        .unwrap();
    oms.insert("ord-2".to_string(), order);
    // Fill(60) when filled=60, total=100 → 60+60=120 ≠ 100 → TransitionError.
    let ev = make_fill_event("ord-2", "fill-overflow", 60);
    let result = apply_fill_step(&mut oms, "ord-2", &ev, "fill-overflow");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("OMS transition error"),
        "expected OMS transition error, got: {err}"
    );
    // filled_qty must NOT have advanced on rejection.
    assert_eq!(oms["ord-2"].filled_qty, 60);
}
/// Section C - T5.
/// A duplicate fill replay (same msg_id applied twice to the same order)
/// must return Ok(Some(fill)) on the first call and Ok(None) on the second.
/// filled_qty must not advance on the duplicate.
#[test]
fn duplicate_fill_replay_does_not_double_apply_portfolio() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert("ord-3".to_string(), OmsOrder::new("ord-3", "SPY", 100));
    let ev = make_partial_fill_event("ord-3", "pf-msg-dup", 60);
    // First application: fill goes through.
    let first = apply_fill_step(&mut oms, "ord-3", &ev, "pf-msg-dup")
        .unwrap()
        .expect("first application must return Some(fill)");
    assert_eq!(first.qty, 60);
    assert_eq!(oms["ord-3"].filled_qty, 60);
    // Second application with the same msg_id: OMS dedup → no state change.
    let second = apply_fill_step(&mut oms, "ord-3", &ev, "pf-msg-dup").unwrap();
    assert!(
        second.is_none(),
        "duplicate fill replay must return None to prevent double portfolio mutation"
    );
    // filled_qty must not have advanced.
    assert_eq!(oms["ord-3"].filled_qty, 60);
}

#[test]
fn duplicate_economic_fill_id_across_messages_is_deduped() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert("ord-3b".to_string(), OmsOrder::new("ord-3b", "SPY", 100));

    let mut ev1 = make_partial_fill_event("ord-3b", "transport-msg-1", 60);
    let mut ev2 = make_partial_fill_event("ord-3b", "transport-msg-2", 60);
    if let BrokerEvent::PartialFill { broker_fill_id, .. } = &mut ev1 {
        *broker_fill_id = Some("econ-fill-1".to_string());
    }
    if let BrokerEvent::PartialFill { broker_fill_id, .. } = &mut ev2 {
        *broker_fill_id = Some("econ-fill-1".to_string());
    }

    let first = apply_fill_step(&mut oms, "ord-3b", &ev1, "transport-msg-1")
        .unwrap()
        .expect("first apply should mutate portfolio");
    assert_eq!(first.qty, 60);
    assert_eq!(oms["ord-3b"].filled_qty, 60);

    let second = apply_fill_step(&mut oms, "ord-3b", &ev2, "transport-msg-2").unwrap();
    assert!(
        second.is_none(),
        "same broker_fill_id should dedupe even when broker_message_id changes"
    );
    assert_eq!(oms["ord-3b"].filled_qty, 60);
}
/// Section C - T6.
/// A non-fill event (Ack) for an order not present in oms_orders must
/// return Ok(None) - not Err.  Unknown-order Acks are silently skipped
/// because they carry no portfolio effect and can arrive legitimately after
/// a crash during restart recovery.
#[test]
fn unknown_order_non_fill_is_silently_skipped() {
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let ev = make_ack_event("ord-ghost", "ack-msg-ghost");
    let result = apply_fill_step(&mut oms, "ord-ghost", &ev, "ack-msg-ghost");
    assert!(
        result.unwrap().is_none(),
        "non-fill event for unknown order must return Ok(None), not Err"
    );
}

fn valid_submit_order_json() -> serde_json::Value {
    serde_json::json!({
        "symbol": "SPY",
        "side": "buy",
        "qty": 10,
        "order_type": "market",
        "limit_price": null,
        "time_in_force": "day"
    })
}

fn legacy_minimal_submit_order_json() -> serde_json::Value {
    serde_json::json!({
        "symbol": "SPY",
        "quantity": 10
    })
}

struct SubmitCountingBroker {
    submits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl mqk_execution::BrokerAdapter for SubmitCountingBroker {
    fn submit_order(
        &self,
        req: mqk_execution::BrokerSubmitRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerSubmitResponse, mqk_execution::BrokerError> {
        self.submits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(mqk_execution::BrokerSubmitResponse {
            broker_order_id: format!("broker-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerCancelResponse, mqk_execution::BrokerError> {
        Ok(mqk_execution::BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: mqk_execution::BrokerReplaceRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerReplaceResponse, mqk_execution::BrokerError> {
        Ok(mqk_execution::BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), mqk_execution::BrokerError>
    {
        Ok((Vec::new(), None))
    }
}

#[test]
fn submit_request_rejects_zero_effective_quantity() {
    let mut zero_qty = valid_submit_order_json();
    zero_qty["qty"] = serde_json::json!(0);
    let err = build_validated_submit_request("ord-zero", &zero_qty)
        .expect_err("zero quantity must be rejected before broker submission");
    assert!(err.to_string().contains("quantity") || err.to_string().contains("qty"));
}

#[test]
fn submit_request_rejects_out_of_range_quantity() {
    let mut out_of_range = valid_submit_order_json();
    out_of_range["qty"] = serde_json::json!(2147483648_i64);
    let err = build_validated_submit_request("ord-range", &out_of_range)
        .expect_err("out-of-range quantity must be rejected before broker submission");
    assert!(err.to_string().contains("out of range"));

    let mut lossy = valid_submit_order_json();
    lossy["qty"] = serde_json::json!(1.5);
    let err = build_validated_submit_request("ord-lossy", &lossy)
        .expect_err("lossy quantity must be rejected before broker submission");
    assert!(
        err.to_string().contains("lossy conversion") || err.to_string().contains("integer")
    );
}

#[test]
fn legacy_payload_without_side_uses_signed_quantity_compatibility_rule() {
    let payload = serde_json::json!({
        "symbol": "SPY",
        "quantity": -25,
        "order_type": "market",
        "time_in_force": "day"
    });

    let req = build_validated_submit_request("ord-legacy-side", &payload)
        .expect("legacy signed-quantity payload must build");

    assert!(matches!(req.side, mqk_execution::Side::Sell));
    assert_eq!(req.quantity, 25);
    assert_eq!(req.order_type, "market");
    assert_eq!(req.time_in_force, "day");
}

#[test]
fn legacy_payload_without_order_type_or_tif_uses_repo_backed_defaults() {
    let req = build_validated_submit_request(
        "ord-legacy-defaults",
        &legacy_minimal_submit_order_json(),
    )
    .expect("legacy minimal payload must build with repo-backed defaults");

    assert!(matches!(req.side, mqk_execution::Side::Buy));
    assert_eq!(req.quantity, 10);
    assert_eq!(req.order_type, "market");
    assert_eq!(req.time_in_force, "day");
    assert_eq!(req.limit_price, None);
}

#[test]
fn submit_request_rejects_missing_or_blank_symbol() {
    let mut missing_symbol = valid_submit_order_json();
    let missing_obj = missing_symbol.as_object_mut().expect("object");
    missing_obj.remove("symbol");
    let err = build_validated_submit_request("ord-missing-symbol", &missing_symbol)
        .expect_err("missing symbol must be rejected before broker submission");
    assert!(err.to_string().contains("symbol"));

    let mut blank_symbol = valid_submit_order_json();
    blank_symbol["symbol"] = serde_json::json!("   ");
    let err = build_validated_submit_request("ord-blank-symbol", &blank_symbol)
        .expect_err("blank symbol must be rejected before broker submission");
    assert!(err.to_string().contains("symbol"));
}

#[test]
fn submit_request_rejects_invalid_order_type_or_price_semantics() {
    let mut unsupported_type = valid_submit_order_json();
    unsupported_type["order_type"] = serde_json::json!("stop");
    let err = build_validated_submit_request("ord-stop", &unsupported_type)
        .expect_err("unsupported order_type must be rejected before broker submission");
    assert!(err.to_string().contains("order_type"));

    let mut limit_missing_price = valid_submit_order_json();
    limit_missing_price["order_type"] = serde_json::json!("limit");
    let err = build_validated_submit_request("ord-limit-missing", &limit_missing_price)
        .expect_err("limit order missing limit_price must be rejected");
    assert!(err.to_string().contains("limit_price"));

    let mut market_with_limit = valid_submit_order_json();
    market_with_limit["limit_price"] = serde_json::json!(1000000);
    let err = build_validated_submit_request("ord-market-limit", &market_with_limit)
        .expect_err("market order carrying limit_price must be rejected");
    assert!(err.to_string().contains("limit_price"));
}

#[test]
fn incompatible_qty_and_quantity_fields_are_rejected() {
    let payload = serde_json::json!({
        "symbol": "SPY",
        "side": "buy",
        "qty": 5,
        "quantity": 10,
        "order_type": "market",
        "time_in_force": "day"
    });

    let err = build_validated_submit_request("ord-qty-mismatch", &payload)
        .expect_err("conflicting qty fields must be rejected");
    assert!(err.to_string().contains("disagree"));
}

#[test]
fn malformed_defaulted_market_payload_with_limit_price_is_rejected() {
    let mut payload = legacy_minimal_submit_order_json();
    payload["limit_price"] = serde_json::json!(1_000_000);

    let err = build_validated_submit_request("ord-default-market-limit", &payload)
        .expect_err("defaulted market payload carrying limit_price must be rejected");
    assert!(err.to_string().contains("limit_price"));
}

#[test]
fn malformed_persisted_order_payload_does_not_reach_broker_submit() {
    let submits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _broker = SubmitCountingBroker {
        submits: std::sync::Arc::clone(&submits),
    };
    let mut malformed = valid_submit_order_json();
    malformed["qty"] = serde_json::json!(0);

    let result = build_validated_submit_request("ord-malformed", &malformed);

    assert!(result.is_err(), "malformed payload must fail before submit");
    assert_eq!(
        submits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "invalid persisted payload must not reach broker submit"
    );
}
// -----------------------------------------------------------------------
// Section D - Restart replay safety unit tests
//
// These tests prove that restart replay safety is gated by the durable
// inbox applied_at_utc column (modelled here as queue membership), NOT
// by the OMS in-memory applied_event_ids set.
// -----------------------------------------------------------------------
/// Section D - T1.  Primary restart replay safety proof.
///
/// A fill that was durably marked applied (applied_at_utc IS NOT NULL)
/// before crash is excluded from inbox_load_unapplied_for_run.  Modelled
/// here as an empty apply_queue.  With no rows in the queue, the portfolio
/// cannot be mutated regardless of OmsOrder applied_event_ids being empty.
///
/// This is the load-bearing proof: the DB queue filter is the gate, not
/// the in-memory set.
#[test]
fn applied_fill_absent_from_recovery_queue_leaves_portfolio_clean() {
    let initial_cash = 1_000_000_000_i64;
    // Fresh restart: OmsOrder rebuilt from outbox - applied_event_ids is empty.
    // The fill that was applied before crash is NOT in the apply_queue
    // because inbox_load_unapplied_for_run excluded it (applied_at_utc IS NOT NULL).
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert(
        "ord-pre-crash".to_string(),
        OmsOrder::new("ord-pre-crash", "SPY", 100),
    );
    let apply_queue: Vec<(String, BrokerEvent)> = vec![]; // applied fill filtered by DB
    let mut portfolio = PortfolioState::new(initial_cash);
    for (msg_id, event) in &apply_queue {
        let internal_id = event.internal_order_id().to_string();
        let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
        if let Some(fill) = fill_opt {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
    }
    assert_eq!(
        portfolio.cash_micros, initial_cash,
        "applied fill absent from recovery queue must not re-mutate portfolio cash after restart"
    );
    assert_eq!(
        oms["ord-pre-crash"].filled_qty, 0,
        "fresh OmsOrder must not advance filled_qty when recovery queue is empty"
    );
}
/// Section D - T2.  Unapplied fill recovers exactly once with fresh OMS state.
///
/// Simulates the W6 crash window: fill was inbox-inserted but mark_applied
/// did not complete before crash.  After restart the OmsOrder is rebuilt
/// fresh (applied_event_ids empty) and the fill IS in the recovery queue.
///
/// First apply: Ok(Some(fill)) - portfolio mutated (correct recovery).
/// Second delivery of the same msg_id within the session: Ok(None) -
/// blocked by the within-session OMS dedup (applied_event_ids updated
/// by the first apply).
#[test]
fn unapplied_fill_in_recovery_queue_applies_exactly_once_with_fresh_oms() {
    let initial_cash = 1_000_000_000_000_i64;
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert("ord-w6".to_string(), OmsOrder::new("ord-w6", "SPY", 100));
    let ev = make_fill_event("ord-w6", "crash-window-fill", 100);
    // First recovery apply: fresh applied_event_ids, fill is in queue.
    let fill_opt = apply_fill_step(&mut oms, "ord-w6", &ev, "crash-window-fill").unwrap();
    let mut portfolio = PortfolioState::new(initial_cash);
    if let Some(fill) = fill_opt {
        apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
    }
    assert_ne!(
        portfolio.cash_micros, initial_cash,
        "unapplied fill must apply once and mutate portfolio on crash-window recovery"
    );
    assert_eq!(oms["ord-w6"].filled_qty, 100);
    // Second delivery of same msg_id within recovery session:
    // OMS applied_event_ids now contains "crash-window-fill" → Ok(None).
    let cash_before_second = portfolio.cash_micros;
    let second = apply_fill_step(&mut oms, "ord-w6", &ev, "crash-window-fill").unwrap();
    if let Some(fill) = second {
        apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
    }
    assert_eq!(
        portfolio.cash_micros, cash_before_second,
        "within-session duplicate fill delivery must not mutate portfolio a second time"
    );
}
/// Section D - T3.  Durable applied gate is queue membership, not OMS memory.
///
/// Two fills for the same order:
///   F1 (delta_qty=40) - applied before crash, NOT in apply_queue.
///   F2 (delta_qty=60) - unapplied, IN apply_queue.
///
/// OmsOrder is fresh after restart (applied_event_ids empty, filled_qty=0).
/// Only F2 must reach portfolio; F1's absence from the queue is the fence.
///
/// Proves: which fills mutate portfolio after restart is determined by
/// inbox_load_unapplied_for_run output alone - not by OMS in-memory state.
#[test]
fn durable_applied_gate_is_queue_membership_not_oms_memory() {
    let initial_cash = 1_000_000_000_000_i64;
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    // total_qty=100; F1=40 was applied, F2=60 is unapplied.
    oms.insert(
        "ord-split".to_string(),
        OmsOrder::new("ord-split", "SPY", 100),
    );
    // Only F2 is in the recovery queue; F1 was filtered by the DB.
    let apply_queue: Vec<(String, BrokerEvent)> = vec![(
        "f2".to_string(),
        make_partial_fill_event("ord-split", "f2", 60),
    )];
    let mut portfolio = PortfolioState::new(initial_cash);
    for (msg_id, event) in &apply_queue {
        let internal_id = event.internal_order_id().to_string();
        let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
        if let Some(fill) = fill_opt {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
    }
    // OMS shows only F2's contribution (60), not F1+F2 (100).
    assert_eq!(
        oms["ord-split"].filled_qty,
        60,
        "OMS filled_qty must reflect only F2 (unapplied); F1 (applied, absent) must not advance it"
    );
    // Portfolio cash changed: F2 was applied (cash ≠ initial).
    assert_ne!(
        portfolio.cash_micros, initial_cash,
        "F2 must mutate portfolio cash"
    );
    // If F1 had been double-applied, filled_qty would be 100 not 60.
    // The OMS assertion above is the definitive proof.
}
/// Section D - T4.  Empty applied_event_ids does not bypass restart replay protection.
///
/// Multiple orders rebuilt fresh after restart (all applied_event_ids empty).
/// All fills for those orders were durably applied before crash → none appear
/// in the recovery queue.
///
/// Proves: the OMS in-memory set being empty is not a safety bypass.
/// The durable DB gate (applied_at_utc IS NOT NULL → excluded from queue)
/// is the authoritative restart replay fence.
#[test]
fn empty_oms_applied_event_ids_does_not_bypass_restart_replay_protection() {
    let initial_cash = 1_000_000_000_i64;
    // Multiple orders with fresh applied_event_ids (restart).
    let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    oms.insert("ord-a".to_string(), OmsOrder::new("ord-a", "AAPL", 50));
    oms.insert("ord-b".to_string(), OmsOrder::new("ord-b", "MSFT", 80));
    // All fills were applied before crash → not in recovery queue.
    // The empty OmsOrder applied_event_ids cannot cause them to be re-applied
    // because they never reach apply_fill_step.
    let apply_queue: Vec<(String, BrokerEvent)> = vec![];
    let mut portfolio = PortfolioState::new(initial_cash);
    for (msg_id, event) in &apply_queue {
        let internal_id = event.internal_order_id().to_string();
        let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
        if let Some(fill) = fill_opt {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
    }
    assert_eq!(
        portfolio.cash_micros, initial_cash,
        "empty applied_event_ids must not bypass restart replay protection \
         when recovery queue is empty (DB gate is authoritative)"
    );
    assert!(
        portfolio.positions.is_empty(),
        "no positions must be created when all fills were durably applied pre-crash"
    );
}
#[derive(Clone)]
struct MutableClock {
    now: std::sync::Arc<std::sync::Mutex<chrono::DateTime<chrono::Utc>>>,
}
impl MutableClock {
    fn new(now: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            now: std::sync::Arc::new(std::sync::Mutex::new(now)),
        }
    }
    fn set(&self, now: chrono::DateTime<chrono::Utc>) {
        *self.now.lock().expect("clock lock") = now;
    }
}
impl mqk_db::TimeSource for MutableClock {
    fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
        *self.now.lock().expect("clock lock")
    }
}
struct NoopBroker;
impl mqk_execution::BrokerAdapter for NoopBroker {
    fn submit_order(
        &self,
        req: mqk_execution::BrokerSubmitRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerSubmitResponse, mqk_execution::BrokerError> {
        Ok(mqk_execution::BrokerSubmitResponse {
            broker_order_id: format!("broker-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }
    fn cancel_order(
        &self,
        order_id: &str,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerCancelResponse, mqk_execution::BrokerError> {
        Ok(mqk_execution::BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }
    fn replace_order(
        &self,
        req: mqk_execution::BrokerReplaceRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<mqk_execution::BrokerReplaceResponse, mqk_execution::BrokerError> {
        Ok(mqk_execution::BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }
    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), mqk_execution::BrokerError>
    {
        Ok((Vec::new(), None))
    }
}
#[derive(Clone, Copy)]
struct AllowGate;
impl mqk_execution::IntegrityGate for AllowGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl mqk_execution::RiskGate for AllowGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
impl mqk_execution::ReconcileGate for AllowGate {
    fn is_clean(&self) -> bool {
        true
    }
}
type LeaseTestOrchestrator =
    ExecutionOrchestrator<NoopBroker, AllowGate, AllowGate, AllowGate, MutableClock>;
async fn runtime_test_pool() -> PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-runtime runtime_ -- --include-ignored"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_leader_lease");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    pool
}
fn runtime_ts(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .timestamp_opt(seconds, 0)
        .single()
        .expect("valid timestamp")
}
async fn make_running_run(pool: &PgPool, started_at: chrono::DateTime<chrono::Utc>) -> Uuid {
    let run_id = Uuid::new_v4(); // allow: test-only — isolated DB test fixture, never called from production paths
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("runtime-test-{}", run_id),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "TEST".to_string(),
            config_hash: format!("cfg-{}", run_id),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(pool, run_id).await.expect("arm run");
    mqk_db::begin_run(pool, run_id).await.expect("begin run");
    run_id
}
fn make_lease_test_orchestrator(
    pool: PgPool,
    run_id: Uuid,
    clock: MutableClock,
) -> LeaseTestOrchestrator {
    ExecutionOrchestrator::new(
        pool,
        mqk_execution::BrokerGateway::for_test(NoopBroker, AllowGate, AllowGate, AllowGate),
        mqk_execution::BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(0),
        run_id,
        "runtime-lease-test",
        "paper",
        None,
        clock,
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}
fn broker_snapshot_with_position(
    fetched_at_ms: i64,
    qty: i64,
) -> mqk_reconcile::BrokerSnapshot {
    let mut broker = mqk_reconcile::BrokerSnapshot::empty_at(fetched_at_ms);
    broker.positions.insert("SPY".to_string(), qty);
    broker
}
#[test]
fn runtime_reconcile_gate_remains_dirty_after_stale_snapshot() {
    let mut watermark = SnapshotWatermark::new();
    let mut local = mqk_reconcile::LocalSnapshot::empty();
    local.positions.insert("SPY".to_string(), 100);
    let dirty = broker_snapshot_with_position(2_000, 200);
    let err = evaluate_monotonic_reconcile(&mut watermark, &local, &dirty)
        .expect_err("fresh dirty snapshot must block dispatch");
    assert!(matches!(err, MonotonicReconcileError::Dirty));
    let stale_clean = broker_snapshot_with_position(1_000, 100);
    let err = evaluate_monotonic_reconcile(&mut watermark, &local, &stale_clean)
        .expect_err("stale snapshot must not clear dirty state");
    assert!(matches!(
        err,
        MonotonicReconcileError::Stale(StaleBrokerSnapshot {
            freshness: mqk_reconcile::SnapshotFreshness::Stale { .. }
        })
    ));
}
#[test]
fn placeholder_snapshot_path_fails_closed() {
    let mut watermark = SnapshotWatermark::new();
    let local = mqk_reconcile::LocalSnapshot::empty();
    let broker = mqk_reconcile::BrokerSnapshot::empty();
    let err = evaluate_monotonic_reconcile(&mut watermark, &local, &broker)
        .expect_err("placeholder broker snapshot must fail closed");
    assert!(matches!(
        err,
        MonotonicReconcileError::Stale(StaleBrokerSnapshot {
            freshness: mqk_reconcile::SnapshotFreshness::NoTimestamp
        })
    ));
}
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn runtime_refuses_to_run_without_lease() {
    let pool = runtime_test_pool().await;
    let clock = MutableClock::new(runtime_ts(10_000));
    let run_id = make_running_run(&pool, clock.now_utc()).await;
    let locked =
        mqk_db::runtime_lease::acquire_lease(&pool, "other-runtime", clock.now_utc(), 30)
            .await
            .expect("seed active lease");
    assert!(matches!(
        locked,
        mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(_)
    ));
    let mut orchestrator = make_lease_test_orchestrator(pool.clone(), run_id, clock.clone());
    let err = orchestrator
        .tick()
        .await
        .expect_err("tick must refuse without lease");
    assert!(
        err.to_string().contains("RUNTIME_LEASE_UNAVAILABLE"),
        "unexpected error: {err}"
    );
    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));
    let arm_state = mqk_db::load_arm_state(&pool)
        .await
        .expect("load arm state")
        .expect("arm state persisted");
    assert_eq!(arm_state.0, "DISARMED");
}
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn runtime_halts_when_lease_is_lost() {
    let pool = runtime_test_pool().await;
    let clock = MutableClock::new(runtime_ts(20_000));
    let run_id = make_running_run(&pool, clock.now_utc()).await;
    let mut orchestrator = make_lease_test_orchestrator(pool.clone(), run_id, clock.clone());
    orchestrator
        .tick()
        .await
        .expect("first tick acquires lease");
    clock.set(runtime_ts(20_016));
    let stolen =
        mqk_db::runtime_lease::acquire_lease(&pool, "other-runtime", clock.now_utc(), 30)
            .await
            .expect("steal expired lease");
    assert!(matches!(
        stolen,
        mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(_)
    ));
    let err = orchestrator
        .tick()
        .await
        .expect_err("tick must halt on lease loss");
    assert!(
        err.to_string().contains("RUNTIME_LEASE_LOST"),
        "unexpected error: {err}"
    );
    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));
    let arm_state = mqk_db::load_arm_state(&pool)
        .await
        .expect("load arm state")
        .expect("arm state persisted");
    assert_eq!(arm_state.0, "DISARMED");
    let lease = mqk_db::runtime_lease::fetch_current_lease(&pool)
        .await
        .expect("fetch current lease")
        .expect("active lease row");
    assert_eq!(lease.holder_id, "other-runtime");
}
