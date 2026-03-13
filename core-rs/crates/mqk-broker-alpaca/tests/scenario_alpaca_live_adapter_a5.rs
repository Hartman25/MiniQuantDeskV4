//! End-to-end adapter tests - Patch A5 live adapter completion.
//!
//! # Coverage
//!
//! L1  build_submit_body: market-buy produces correct field values.
//! L2  build_submit_body: limit-sell converts price micros to decimal string at boundary.
//! L3  build_submit_body: client_order_id is set from req.order_id (not fabricated).
//! L4  AlpacaSubmitResponse: parses broker order UUID from fixture JSON.
//! L5  build_replace_body: new total qty = filled_qty + new open leaves (Alpaca semantics).
//! L6  build_replace_body: limit price micros → decimal string at wire boundary.
//! L7  activity_to_trade_update + normalize: FILL activity produces BrokerEvent::Fill.
//! L8  activity_to_trade_update + normalize: PARTIAL_FILL activity produces PartialFill.
//! L9  activity_to_trade_update + normalize: non-fill lifecycle activities map canonically.
//! L10 activity_to_trade_update: unknown activity_type returns Err (normalizer enforced).
//! L10 BrokerEvent::Fill from activity carries deterministic broker_message_id format.
//! L11 adapter submit_order fails with Transport/AmbiguousSubmit on unreachable URL.
//! L12 adapter cancel_order fails with Transport on unreachable URL.
//! L13 adapter replace_order fails with Transport on unreachable URL.
//! L14 adapter fetch_events fails with Transport (not silent empty-vec stub) on unreachable URL.
//!
//! All tests are pure in-memory or use an unreachable URL - no live network,
//! no DB, no wall-clock reads.
use mqk_broker_alpaca::{
    activity_to_trade_update, build_replace_body, build_submit_body, decode_fetch_cursor,
    encode_fetch_cursor, micros_to_price_str,
    normalize::normalize_trade_update,
    types::{AlpacaFetchCursor, AlpacaOrderActivity, AlpacaOrderFull, AlpacaSubmitResponse},
    AlpacaBrokerAdapter, AlpacaConfig,
};
use mqk_execution::{
    BrokerAdapter, BrokerError, BrokerEvent, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerSubmitRequest, Side,
};
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
/// Construct an adapter that points to an unreachable address (port 1 on
/// loopback).  Any real network call will fail with a connection error, which
/// the adapter must surface as `BrokerError::Transport` or `BrokerError::AmbiguousSubmit`
/// - never as a silent success.
fn unreachable_adapter() -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key_id: "test-key-id".to_string(),
        api_secret_key: "test-secret-key".to_string(),
    })
}
fn make_submit_req(
    order_id: &str,
    symbol: &str,
    side: Side,
    quantity: i32,
    order_type: &str,
    limit_price: Option<i64>,
) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: order_id.to_string(),
        symbol: symbol.to_string(),
        side,
        quantity,
        order_type: order_type.to_string(),
        limit_price,
        time_in_force: "day".to_string(),
    }
}
#[allow(clippy::too_many_arguments)]
fn make_activity(
    id: &str,
    activity_type: &str,
    order_id: &str,
    ts: &str,
    price: Option<&str>,
    qty: Option<&str>,
    side: &str,
    symbol: &str,
) -> AlpacaOrderActivity {
    AlpacaOrderActivity {
        id: id.to_string(),
        activity_type: activity_type.to_string(),
        order_id: order_id.to_string(),
        transaction_time: ts.to_string(),
        price: price.map(str::to_string),
        qty: qty.map(str::to_string),
        side: side.to_string(),
        symbol: symbol.to_string(),
    }
}
fn make_order_full(
    id: &str,
    client_order_id: &str,
    symbol: &str,
    side: &str,
    qty: &str,
    filled_qty: &str,
) -> AlpacaOrderFull {
    AlpacaOrderFull {
        id: id.to_string(),
        client_order_id: client_order_id.to_string(),
        symbol: symbol.to_string(),
        side: side.to_string(),
        qty: qty.to_string(),
        filled_qty: filled_qty.to_string(),
    }
}
// ---------------------------------------------------------------------------
// L1 - build_submit_body: market-buy field values
// ---------------------------------------------------------------------------
#[test]
fn l1_build_submit_body_market_buy() {
    let req = make_submit_req("ord-l1", "AAPL", Side::Buy, 100, "market", None);
    let body = build_submit_body(&req);
    assert_eq!(body.symbol, "AAPL");
    assert_eq!(body.qty, "100");
    assert_eq!(body.side, "buy");
    assert_eq!(body.order_type, "market");
    assert_eq!(body.time_in_force, "day");
    assert_eq!(
        body.limit_price, None,
        "market order must not include limit_price"
    );
    assert_eq!(body.client_order_id, "ord-l1");
}
// ---------------------------------------------------------------------------
// L2 - build_submit_body: limit-sell price micros converted to decimal string
// ---------------------------------------------------------------------------
#[test]
fn l2_build_submit_body_limit_sell_price_at_wire_boundary() {
    // $250.50 = 250_500_000 micros
    let req = make_submit_req("ord-l2", "TSLA", Side::Sell, 50, "limit", Some(250_500_000));
    let body = build_submit_body(&req);
    assert_eq!(body.side, "sell");
    assert_eq!(
        body.qty, "50",
        "quantity must be positive regardless of side"
    );
    // Price must be a decimal string, never the raw micros integer
    let price_str = body
        .limit_price
        .as_deref()
        .expect("limit order must have limit_price");
    assert_eq!(
        price_str, "250.50",
        "price micros must be converted to decimal at wire boundary"
    );
    // The string must NOT look like a raw micros value
    assert!(
        !price_str.contains("250500"),
        "price string must not contain raw micros: got {price_str}"
    );
}
// ---------------------------------------------------------------------------
// L3 - build_submit_body: client_order_id set from req.order_id
// ---------------------------------------------------------------------------
#[test]
fn l3_build_submit_body_client_order_id_is_order_id() {
    let req = make_submit_req("internal-ord-l3-abc", "MSFT", Side::Buy, 10, "market", None);
    let body = build_submit_body(&req);
    assert_eq!(
        body.client_order_id, "internal-ord-l3-abc",
        "client_order_id must be the canonical order_id, not fabricated"
    );
}
// ---------------------------------------------------------------------------
// L4 - AlpacaSubmitResponse: parse broker order UUID from fixture JSON
// ---------------------------------------------------------------------------
#[test]
fn l4_parse_submit_response_extracts_broker_order_id() {
    let json = r#"{
        "id": "alpaca-broker-uuid-l4-001",
        "client_order_id": "internal-ord-l4",
        "created_at": "2024-06-15T09:30:00Z"
    }"#;
    let resp: AlpacaSubmitResponse = serde_json::from_str(json).expect("fixture must deserialize");
    assert_eq!(resp.id, "alpaca-broker-uuid-l4-001");
    assert_eq!(resp.client_order_id, "internal-ord-l4");
    assert_eq!(resp.created_at.as_deref(), Some("2024-06-15T09:30:00Z"));
}
#[test]
fn l4_parse_submit_response_created_at_is_optional() {
    // Some Alpaca environments (sandbox) omit created_at.
    let json = r#"{"id":"alpaca-uuid-l4b","client_order_id":"ord-l4b"}"#;
    let resp: AlpacaSubmitResponse = serde_json::from_str(json).expect("fixture must deserialize");
    assert_eq!(resp.id, "alpaca-uuid-l4b");
    assert!(resp.created_at.is_none());
}
// ---------------------------------------------------------------------------
// L5 - build_replace_body: total qty = filled + new open leaves
// ---------------------------------------------------------------------------
#[test]
fn l5_build_replace_body_total_qty_semantics() {
    // Alpaca filled 20 shares; operator wants 80 more open leaves → total = 100.
    let body = build_replace_body(80, 20, None, "day");
    assert_eq!(
        body.qty, "100",
        "replace qty must be total (filled + new leaves), got {}",
        body.qty
    );
}
#[test]
fn l5_build_replace_body_zero_filled() {
    // No fills yet; new open leaves = 50 → total = 50.
    let body = build_replace_body(50, 0, None, "day");
    assert_eq!(body.qty, "50");
}
#[test]
fn l5_build_replace_body_large_fill() {
    // Heavily-filled order: 900 filled, want 100 more → total = 1000.
    let body = build_replace_body(100, 900, None, "gtc");
    assert_eq!(body.qty, "1000");
}
// ---------------------------------------------------------------------------
// L6 - build_replace_body: limit price micros → decimal at wire boundary
// ---------------------------------------------------------------------------
#[test]
fn l6_build_replace_body_limit_price_at_wire_boundary() {
    // $150.75 = 150_750_000 micros
    let body = build_replace_body(50, 0, Some(150_750_000), "gtc");
    let price_str = body
        .limit_price
        .as_deref()
        .expect("limit replace must have limit_price");
    assert_eq!(price_str, "150.75");
}
#[test]
fn l6_build_replace_body_no_limit_price_for_market() {
    let body = build_replace_body(100, 0, None, "day");
    assert!(body.limit_price.is_none());
}
// ---------------------------------------------------------------------------
// L7 - activity → trade_update → normalize: FILL → BrokerEvent::Fill
// ---------------------------------------------------------------------------
#[test]
fn l7_fill_activity_through_normalization_pipeline() {
    let activity = make_activity(
        "20240615093000000::activity-l7",
        "FILL",
        "alpaca-broker-uuid-l7",
        "2024-06-15T09:30:00.000000Z",
        Some("150.50"),
        Some("100"),
        "buy",
        "AAPL",
    );
    let order = make_order_full(
        "alpaca-broker-uuid-l7",
        "internal-ord-l7",
        "AAPL",
        "buy",
        "100",
        "100",
    );
    let trade_update = activity_to_trade_update(&activity, &order).expect("mapping must succeed");
    let event = normalize_trade_update(&trade_update).expect("normalize must succeed");
    // Variant
    assert!(
        matches!(event, BrokerEvent::Fill { .. }),
        "expected Fill, got {event:?}"
    );
    // ID mapping
    assert_eq!(event.internal_order_id(), "internal-ord-l7");
    assert_eq!(event.broker_order_id(), Some("alpaca-broker-uuid-l7"));
    // Fill fields
    match event {
        BrokerEvent::Fill {
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            assert_eq!(delta_qty, 100);
            assert_eq!(price_micros, 150_500_000);
            assert_eq!(fee_micros, 0, "Alpaca does not carry per-trade fee data");
        }
        _ => unreachable!(),
    }
}
// ---------------------------------------------------------------------------
// L8 - PARTIAL_FILL activity → BrokerEvent::PartialFill
// ---------------------------------------------------------------------------
#[test]
fn l8_partial_fill_activity_through_normalization_pipeline() {
    let activity = make_activity(
        "20240615093100000::activity-l8",
        "PARTIAL_FILL",
        "alpaca-broker-uuid-l8",
        "2024-06-15T09:31:00.000000Z",
        Some("200.25"),
        Some("40"),
        "sell",
        "TSLA",
    );
    let order = make_order_full(
        "alpaca-broker-uuid-l8",
        "internal-ord-l8",
        "TSLA",
        "sell",
        "100",
        "40",
    );
    let trade_update = activity_to_trade_update(&activity, &order).expect("mapping must succeed");
    let event = normalize_trade_update(&trade_update).expect("normalize must succeed");
    assert!(matches!(event, BrokerEvent::PartialFill { .. }));
    assert_eq!(event.internal_order_id(), "internal-ord-l8");
    assert_eq!(event.broker_order_id(), Some("alpaca-broker-uuid-l8"));
    match event {
        BrokerEvent::PartialFill {
            delta_qty,
            price_micros,
            ..
        } => {
            assert_eq!(delta_qty, 40);
            assert_eq!(price_micros, 200_250_000);
        }
        _ => unreachable!(),
    }
}
// ---------------------------------------------------------------------------
// L9 - non-fill lifecycle activity types map through canonical normalization
// ---------------------------------------------------------------------------
#[test]
fn l9_non_fill_lifecycle_activity_types_map_canonically() {
    let cases = [
        ("NEW", "new", "Ack"),
        ("PENDING_NEW", "new", "Ack"),
        ("ACCEPTED", "new", "Ack"),
        ("CANCELED", "canceled", "CancelAck"),
        ("EXPIRED", "canceled", "CancelAck"),
        ("CANCEL_REJECTED", "cancel_rejected", "CancelReject"),
        ("REPLACED", "replaced", "ReplaceAck"),
        ("REPLACE_REJECTED", "replace_rejected", "ReplaceReject"),
        ("REJECTED", "rejected", "Reject"),
    ];

    for (activity_type, expected_event, expected_variant) in cases {
        let activity = make_activity(
            "20240615093200000::activity-l9",
            activity_type,
            "alpaca-broker-uuid-l9",
            "2024-06-15T09:32:00.000000Z",
            None,
            None,
            "buy",
            "AAPL",
        );
        let order = make_order_full(
            "alpaca-broker-uuid-l9",
            "internal-ord-l9",
            "AAPL",
            "buy",
            "100",
            "0",
        );
        let trade_update = activity_to_trade_update(&activity, &order)
            .expect("known lifecycle activity_type must map to trade update");
        assert_eq!(trade_update.event, expected_event);
        let normalized = normalize_trade_update(&trade_update)
            .expect("mapped lifecycle trade update must normalize");
        let variant_ok = match normalized {
            BrokerEvent::Ack { .. } => expected_variant == "Ack",
            BrokerEvent::CancelAck { .. } => expected_variant == "CancelAck",
            BrokerEvent::CancelReject { .. } => expected_variant == "CancelReject",
            BrokerEvent::ReplaceAck { .. } => expected_variant == "ReplaceAck",
            BrokerEvent::ReplaceReject { .. } => expected_variant == "ReplaceReject",
            BrokerEvent::Reject { .. } => expected_variant == "Reject",
            _ => false,
        };
        assert!(
            variant_ok,
            "activity_type={activity_type} expected variant={expected_variant}, got {normalized:?}"
        );
    }
}
// ---------------------------------------------------------------------------
// L10 - unknown activity_type returns Err (normalizer is NOT bypassed)
// ---------------------------------------------------------------------------
#[test]
fn l10_unknown_activity_type_returns_err_not_empty_event() {
    // "DIV" (dividend) must not pass through the normalizer.
    let activity = make_activity(
        "20240615093200000::activity-l9",
        "DIV",
        "alpaca-broker-uuid-l9",
        "2024-06-15T09:32:00.000000Z",
        None,
        None,
        "buy",
        "AAPL",
    );
    let order = make_order_full(
        "alpaca-broker-uuid-l9",
        "internal-ord-l9",
        "AAPL",
        "buy",
        "100",
        "0",
    );
    let result = activity_to_trade_update(&activity, &order);
    assert!(
        result.is_err(),
        "unknown activity_type must return Err, not produce a silent event"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("DIV"),
        "error must identify the unknown activity_type, got: {msg}"
    );
}
#[test]
fn l10_non_fill_activity_not_silently_normalized() {
    for activity_type in &["ACATC", "ACATS", "CSD", "CSW", "PTC", "REORG", "SSO", "SSP"] {
        let activity = make_activity(
            "act-id",
            activity_type,
            "alpaca-broker-uuid",
            "2024-06-15T09:30:00.000000Z",
            None,
            None,
            "buy",
            "AAPL",
        );
        let order = make_order_full(
            "alpaca-broker-uuid",
            "internal-ord",
            "AAPL",
            "buy",
            "100",
            "0",
        );
        let result = activity_to_trade_update(&activity, &order);
        assert!(
            result.is_err(),
            "activity_type={activity_type} must not produce a canonical event"
        );
    }
}
// ---------------------------------------------------------------------------
// L10 - broker_message_id format: proves normalizer was executed
// ---------------------------------------------------------------------------
#[test]
fn l10_fill_event_broker_message_id_has_deterministic_alpaca_format() {
    let broker_id = "alpaca-broker-uuid-l10";
    let ts = "2024-06-15T09:30:00.000000Z";
    let activity = make_activity(
        "20240615093000000::l10",
        "FILL",
        broker_id,
        ts,
        Some("100.00"),
        Some("50"),
        "buy",
        "AAPL",
    );
    let order = make_order_full(broker_id, "internal-ord-l10", "AAPL", "buy", "50", "50");
    let trade_update = activity_to_trade_update(&activity, &order).unwrap();
    let event = normalize_trade_update(&trade_update).unwrap();
    // The normalizer must have set this; raw events don't have this format.
    let expected_mid = format!("alpaca:{broker_id}:fill:{ts}");
    assert_eq!(
        event.broker_message_id(),
        expected_mid,
        "broker_message_id must be deterministic; normalizer must have been called"
    );
    // Calling normalize twice on the same input must produce identical ids.
    let event2 = normalize_trade_update(&trade_update).unwrap();
    assert_eq!(event.broker_message_id(), event2.broker_message_id());
}
// ---------------------------------------------------------------------------
// L11 - submit_order fails on unreachable URL (not silent ok)
// ---------------------------------------------------------------------------
#[test]
fn l11_adapter_submit_order_fails_with_transport_on_unreachable_url() {
    let adapter = unreachable_adapter();
    let req = make_submit_req("ord-l11", "AAPL", Side::Buy, 10, "market", None);
    let token = BrokerInvokeToken::for_test();
    let result = adapter.submit_order(req, &token);
    assert!(result.is_err(), "submit_order must not silently succeed");
    match result.unwrap_err() {
        BrokerError::Transport { .. } | BrokerError::AmbiguousSubmit { .. } => {}
        other => {
            panic!("submit to unreachable URL must be Transport or AmbiguousSubmit, got {other:?}")
        }
    }
}
// ---------------------------------------------------------------------------
// L12 - cancel_order fails on unreachable URL
// ---------------------------------------------------------------------------
#[test]
fn l12_adapter_cancel_order_fails_with_transport_on_unreachable_url() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let result = adapter.cancel_order("alpaca-broker-uuid-l12", &token);
    assert!(result.is_err(), "cancel_order must not silently succeed");
    match result.unwrap_err() {
        BrokerError::Transport { .. } | BrokerError::Transient { .. } => {}
        other => panic!("cancel to unreachable URL must be Transport or Transient, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// L13 - replace_order fails on unreachable URL (GET for filled_qty fails first)
// ---------------------------------------------------------------------------
#[test]
fn l13_adapter_replace_order_fails_on_unreachable_url() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let req = BrokerReplaceRequest {
        broker_order_id: "alpaca-broker-uuid-l13".to_string(),
        quantity: 100,
        limit_price: Some(150_000_000),
        time_in_force: "day".to_string(),
    };
    let result = adapter.replace_order(req, &token);
    assert!(result.is_err(), "replace_order must not silently succeed");
    match result.unwrap_err() {
        BrokerError::Transport { .. } | BrokerError::Transient { .. } => {}
        other => panic!("replace to unreachable URL must be Transport or Transient, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// L14 - fetch_events cold start fails closed and persists resume state
// ---------------------------------------------------------------------------
#[test]
fn l14_adapter_fetch_events_cold_start_fails_closed_and_persists_resume_state() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let result = adapter.fetch_events(None, &token);
    assert!(
        result.is_err(),
        "fetch_events must fail closed when websocket continuity is unproven"
    );
    match result.unwrap_err() {
        BrokerError::InboundContinuityUnproven {
            persist_cursor: Some(cursor),
            ..
        } => {
            let decoded = decode_fetch_cursor(Some(&cursor)).expect("persisted cursor must decode");
            assert_eq!(decoded, AlpacaFetchCursor::cold_start_unproven(None));
        }
        other => panic!("expected InboundContinuityUnproven, got {other:?}"),
    }
}
#[test]
fn alpaca_websocket_gap_detection_fails_closed() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let cursor = encode_fetch_cursor(&AlpacaFetchCursor::gap_detected(
        Some("20240615093001000::activity".to_string()),
        Some("alpaca:ord-1:new:2024-06-15T09:30:00Z".to_string()),
        Some("2024-06-15T09:30:00Z".to_string()),
        "trade update websocket disconnected without a replay token",
    ))
    .expect("cursor encode");
    let err = adapter
        .fetch_events(Some(&cursor), &token)
        .expect_err("gap state must fail closed");
    match err {
        BrokerError::InboundContinuityUnproven {
            persist_cursor: Some(persisted),
            ..
        } => assert_eq!(persisted, cursor),
        other => panic!("expected InboundContinuityUnproven, got {other:?}"),
    }
}
#[test]
fn alpaca_websocket_reconnect_resume_is_safe() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let cursor = encode_fetch_cursor(&AlpacaFetchCursor::live(
        Some("20240615093001000::activity".to_string()),
        "alpaca:ord-1:new:2024-06-15T09:30:00Z",
        "2024-06-15T09:30:00Z",
    ))
    .expect("cursor encode");
    let err = adapter
        .fetch_events(Some(&cursor), &token)
        .expect_err("live resume state should continue into the real transport path");
    match err {
        BrokerError::Transport { .. } | BrokerError::Transient { .. } => {}
        other => panic!("live resume state must reach the real transport path, got {other:?}"),
    }
}
#[test]
fn broker_cursor_or_resume_state_persists_honestly() {
    let live = AlpacaFetchCursor::live(
        Some("20240615093001000::activity".to_string()),
        "alpaca:ord-1:new:2024-06-15T09:30:00Z",
        "2024-06-15T09:30:00Z",
    );
    let encoded = encode_fetch_cursor(&live).expect("encode live cursor");
    let decoded = decode_fetch_cursor(Some(&encoded)).expect("decode live cursor");
    assert_eq!(decoded, live);
    let legacy = decode_fetch_cursor(Some("20240615093001000::legacy-activity"))
        .expect("legacy activity cursor must up-convert");
    assert_eq!(
        legacy,
        AlpacaFetchCursor::cold_start_unproven(Some(
            "20240615093001000::legacy-activity".to_string()
        ))
    );
}
// ---------------------------------------------------------------------------
// Supplementary: micros_to_price_str round-trip
// ---------------------------------------------------------------------------
#[test]
fn supplementary_micros_to_price_str_examples() {
    assert_eq!(micros_to_price_str(150_000_000), "150.00");
    assert_eq!(micros_to_price_str(150_500_000), "150.50");
    assert_eq!(micros_to_price_str(200_250_000), "200.25");
    assert_eq!(micros_to_price_str(0), "0.00");
    assert_eq!(micros_to_price_str(1_000_000), "1.00");
}
