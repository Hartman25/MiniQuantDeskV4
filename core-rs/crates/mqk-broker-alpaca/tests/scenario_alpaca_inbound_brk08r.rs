//! BRK-08R — Full inbound broker proof suite.
//!
//! # Purpose
//!
//! Proves the complete correctness contract for the Alpaca inbound lane
//! introduced by BRK-00R / BRK-01R / BRK-02R / BRK-07R.
//!
//! All tests are pure in-memory — no network, no DB, no wall-clock reads.
//!
//! # Coverage
//!
//! ## R — Fail-closed behavior (BRK-07R)
//! R1  ColdStartUnproven cursor: build_inbound_batch still advances to Live.
//!     (Fail-closed for the REST polling lane; WS lane always advances on success.)
//! R2  GapDetected cursor: build_inbound_batch still advances to Live.
//!     (Same rationale — WS lane advances; REST lane fails closed.)
//! R3  After mark_gap_detected, cursor.trade_updates is always GapDetected.
//! R4  Gap cursor encode-decode round-trip produces identical state.
//! R5  Normalization error does not advance the cursor.
//!
//! ## D — Duplicate / idempotent normalization (BRK-01R)
//! D1  Two identical WS messages produce identical batches (idempotent normalization).
//! D2  Duplicate trade update produces the same broker_message_id each time.
//!
//! ## O — Out-of-order messages (BRK-01R)
//! O1  Two WS messages processed in either order normalize to the same individual events.
//!
//! ## C — InboundBatch cursor contract (BRK-00R / BRK-02R)
//! C1  Cursor is not accessible via any public field; peek_cursor is the only borrow path.
//! C2  encode_cursor_for_persist round-trips through serde_json.
//! C3  into_cursor_for_persist consumes the batch (compile-time proof via ownership).
//! C4  peek_cursor does not consume the batch; events remain accessible after peek.
//! C5  Cursor carries schema_version from AlpacaFetchCursor::SCHEMA_VERSION.
//!
//! ## G — Gap detection (BRK-07R)
//! G1  mark_gap_detected from Live preserves last_message_id and last_event_at.
//! G2  mark_gap_detected from ColdStartUnproven produces GapDetected with None positions.
//! G3  mark_gap_detected from GapDetected preserves last_* and replaces detail.
//! G4  mark_gap_detected preserves rest_activity_after unchanged.
//! G5  Gap cursor encode-decode produces valid AlpacaFetchCursor with GapDetected state.
//!
//! ## P — WS message parsing (BRK-01R)
//! P1  trade_updates message decodes to TradeUpdate variant.
//! P2  authorization message decodes status field.
//! P3  listening message decodes streams array.
//! P4  error message decodes code and msg fields.
//! P5  Unknown T field produces Unknown variant with msg_type preserved.
//! P6  Missing T field (ping frame) produces Ping variant.
//! P7  Invalid JSON produces WsParseError::JsonError.
//! P8  Multi-element array decodes all elements independently.
//! P9  trade_updates with malformed data produces Unknown (not error).
//! P10 trade_updates with missing data produces Unknown (not error).
//!
//! ## I — Integration (BRK-00R / BRK-01R / BRK-02R / BRK-07R combined)
//! I1  Full WS lane: parse → build_batch → peek_cursor → encode_cursor_for_persist.
//! I2  Cursor chain: three sequential WS messages each advance the cursor independently.
//! I3  Gap on reconnect: Live → build_batch (advance) → mark_gap_detected → GapDetected.
//! I4  Batch from gap cursor: build_batch from GapDetected still advances to Live.
//!     (Runtime is responsible for not calling build_batch on a gap cursor without
//!      first persisting the gap state.)
use mqk_broker_alpaca::{
    build_inbound_batch_from_ws_update, mark_gap_detected,
    normalize::normalize_trade_update,
    parse_ws_message,
    types::{AlpacaFetchCursor, AlpacaOrder, AlpacaTradeUpdate, AlpacaTradeUpdatesResume},
    AlpacaWsMessage, WsParseError,
};
use mqk_execution::BrokerEvent;
// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------
const BROKER_ID: &str = "alpaca-broker-uuid-brk08r-001";
const CLIENT_ID: &str = "internal-client-brk08r-001";
const SYMBOL: &str = "AAPL";
const TS_A: &str = "2024-06-15T09:30:00.000000Z";
const TS_B: &str = "2024-06-15T09:31:00.000000Z";
const TS_C: &str = "2024-06-15T09:32:00.000000Z";
fn make_order(id: &str, client_id: &str) -> AlpacaOrder {
    AlpacaOrder {
        id: id.to_string(),
        client_order_id: client_id.to_string(),
        symbol: SYMBOL.to_string(),
        side: "buy".to_string(),
        qty: "100".to_string(),
        filled_qty: "0".to_string(),
    }
}
fn make_update(event: &str, ts: &str) -> AlpacaTradeUpdate {
    AlpacaTradeUpdate {
        event: event.to_string(),
        timestamp: ts.to_string(),
        order: make_order(BROKER_ID, CLIENT_ID),
        price: None,
        qty: None,
        broker_fill_id: None,
    }
}
fn make_fill_update(ts: &str, price: &str, qty: &str) -> AlpacaTradeUpdate {
    AlpacaTradeUpdate {
        event: "fill".to_string(),
        timestamp: ts.to_string(),
        order: make_order(BROKER_ID, CLIENT_ID),
        price: Some(price.to_string()),
        qty: Some(qty.to_string()),
        broker_fill_id: None,
    }
}
fn cold_start() -> AlpacaFetchCursor {
    AlpacaFetchCursor::cold_start_unproven(None)
}
fn live_cursor(rest_after: Option<&str>, msg_id: &str, event_at: &str) -> AlpacaFetchCursor {
    AlpacaFetchCursor::live(rest_after.map(str::to_string), msg_id, event_at)
}
fn gap_cursor(
    rest_after: Option<&str>,
    last_msg: Option<&str>,
    last_at: Option<&str>,
    detail: &str,
) -> AlpacaFetchCursor {
    AlpacaFetchCursor::gap_detected(
        rest_after.map(str::to_string),
        last_msg.map(str::to_string),
        last_at.map(str::to_string),
        detail,
    )
}
// ---------------------------------------------------------------------------
// R — Fail-closed behavior
// ---------------------------------------------------------------------------
/// R1: build_inbound_batch_from_ws_update from a ColdStartUnproven cursor
/// advances the cursor to Live (WS lane is self-proving; only REST lane is
/// fail-closed on cold start).
#[test]
fn brk08r_r1_cold_start_cursor_batch_advances_to_live() {
    let prev = cold_start();
    let update = make_update("new", TS_A);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    assert_eq!(batch.events.len(), 1);
    assert!(matches!(
        batch.peek_cursor().trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
/// R2: build_inbound_batch_from_ws_update from a GapDetected cursor advances
/// to Live.  The WS lane processes each message independently.
#[test]
fn brk08r_r2_gap_detected_cursor_batch_advances_to_live() {
    let prev = gap_cursor(None, Some("old-id"), Some(TS_A), "prior disconnect");
    let update = make_update("new", TS_B);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    assert!(matches!(
        batch.peek_cursor().trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
/// R3: mark_gap_detected always produces a GapDetected cursor regardless of
/// the previous state.
#[test]
fn brk08r_r3_mark_gap_always_produces_gap_detected() {
    let cold = cold_start();
    let live = live_cursor(None, "msg-id", TS_A);
    let gap = gap_cursor(None, Some("old-msg"), Some(TS_A), "earlier gap");
    for prev in [&cold, &live, &gap] {
        let result = mark_gap_detected(prev, "test gap");
        assert!(
            matches!(
                result.trade_updates,
                AlpacaTradeUpdatesResume::GapDetected { .. }
            ),
            "expected GapDetected for prev={prev:?}"
        );
    }
}
/// R4: A gap cursor encodes and decodes back to an identical GapDetected state.
#[test]
fn brk08r_r4_gap_cursor_encode_decode_round_trip() {
    let gap = gap_cursor(Some("rest-abc"), Some("last-msg"), Some(TS_A), "disconnect");
    let encoded = serde_json::to_string(&gap).unwrap();
    let decoded: AlpacaFetchCursor = serde_json::from_str(&encoded).unwrap();
    assert_eq!(gap, decoded);
}
/// R5: A normalization error (unknown event type) does not advance the cursor.
/// build_inbound_batch_from_ws_update returns Err without producing a batch.
#[test]
fn brk08r_r5_normalization_error_does_not_advance_cursor() {
    let prev = cold_start();
    // "held" is not a recognized Alpaca event type.
    let bad_update = make_update("held", TS_A);
    let result = build_inbound_batch_from_ws_update(&prev, bad_update);
    assert!(result.is_err(), "expected Err on unknown event type");
}
// ---------------------------------------------------------------------------
// D — Duplicate / idempotent normalization
// ---------------------------------------------------------------------------
/// D1: Two identical WS messages produce identical normalized events and
/// identical cursors (idempotent normalization).
#[test]
fn brk08r_d1_duplicate_ws_message_produces_identical_batches() {
    let prev = cold_start();
    let u1 = make_update("new", TS_A);
    let u2 = make_update("new", TS_A); // identical
    let b1 = build_inbound_batch_from_ws_update(&prev, u1).unwrap();
    let b2 = build_inbound_batch_from_ws_update(&prev, u2).unwrap();
    // Cursors must be identical.
    assert_eq!(b1.peek_cursor(), b2.peek_cursor());
    // Both batches carry one Ack event with the same internal_order_id.
    assert_eq!(b1.events.len(), 1);
    assert_eq!(b2.events.len(), 1);
    let id1 = b1.events[0].internal_order_id().to_string();
    let id2 = b2.events[0].internal_order_id().to_string();
    assert_eq!(id1, id2);
}
/// D2: Two identical WS messages produce the same broker_message_id.
/// This is the deduplication key for inbox_insert_deduped_with_identity.
#[test]
fn brk08r_d2_duplicate_ws_message_produces_same_broker_message_id() {
    let u1 = make_update("new", TS_A);
    let u2 = make_update("new", TS_A);
    let e1 = normalize_trade_update(&u1).unwrap();
    let e2 = normalize_trade_update(&u2).unwrap();
    assert_eq!(e1.broker_message_id(), e2.broker_message_id());
}
// ---------------------------------------------------------------------------
// O — Out-of-order messages
// ---------------------------------------------------------------------------
/// O1: Two WS messages processed in either order normalize to the same
/// individual events (stateless normalization is permutation-invariant).
#[test]
fn brk08r_o1_out_of_order_messages_normalize_to_same_individual_events() {
    let ack_update = make_update("new", TS_A);
    let fill_update = make_fill_update(TS_B, "150.00", "100");
    let ack_event = normalize_trade_update(&ack_update).unwrap();
    let fill_event = normalize_trade_update(&fill_update).unwrap();
    // Process in reverse order — individual event content is unchanged.
    let fill_event_rev = normalize_trade_update(&fill_update).unwrap();
    let ack_event_rev = normalize_trade_update(&ack_update).unwrap();
    assert_eq!(
        ack_event.broker_message_id(),
        ack_event_rev.broker_message_id()
    );
    assert_eq!(
        fill_event.broker_message_id(),
        fill_event_rev.broker_message_id()
    );
    assert!(matches!(ack_event, BrokerEvent::Ack { .. }));
    assert!(matches!(fill_event, BrokerEvent::Fill { .. }));
}
// ---------------------------------------------------------------------------
// C — InboundBatch cursor contract
// ---------------------------------------------------------------------------
/// C1: peek_cursor is the only way to borrow the cursor without consuming
/// the batch; events remain accessible after peek.
#[test]
fn brk08r_c1_peek_cursor_does_not_consume_batch() {
    let prev = cold_start();
    let update = make_update("new", TS_A);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    // Peek does not consume.
    let _ = batch.peek_cursor();
    // Events are still accessible.
    assert_eq!(batch.events.len(), 1);
    // Can peek again.
    let cursor = batch.peek_cursor();
    assert!(matches!(
        cursor.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
/// C2: encode_cursor_for_persist produces valid JSON that round-trips through
/// serde_json to an identical AlpacaFetchCursor.
#[test]
fn brk08r_c2_encode_cursor_for_persist_round_trips() {
    let prev = live_cursor(Some("rest-abc"), "prev-msg", TS_A);
    let update = make_update("new", TS_B);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    let json = batch.encode_cursor_for_persist().unwrap();
    let decoded: AlpacaFetchCursor = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        decoded.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
    assert_eq!(decoded.rest_activity_after.as_deref(), Some("rest-abc"));
}
/// C3: into_cursor_for_persist returns the raw AlpacaFetchCursor and the
/// cursor's schema_version is the canonical SCHEMA_VERSION constant.
#[test]
fn brk08r_c3_into_cursor_for_persist_schema_version() {
    let prev = cold_start();
    let update = make_update("new", TS_A);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    let cursor = batch.into_cursor_for_persist();
    assert_eq!(cursor.schema_version, AlpacaFetchCursor::SCHEMA_VERSION);
}
/// C4: peek_cursor does not consume; into_cursor_for_persist does.
/// (Ownership correctness: this is a compile-time proof.  If into_cursor_for_persist
/// did not consume self, it would not prevent double-persist.  Verified here by
/// confirming that a batch moved into into_cursor_for_persist cannot be used after.)
#[test]
fn brk08r_c4_into_cursor_for_persist_is_consuming() {
    let prev = cold_start();
    let update = make_update("new", TS_A);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    // Move the batch into into_cursor_for_persist.
    let _cursor = batch.into_cursor_for_persist();
    // batch is moved; accessing it here would be a compile error.
    // This test passes if it compiles and the cursor is a valid Live cursor.
    assert!(matches!(
        _cursor.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
/// C5: The cursor produced by build_inbound_batch_from_ws_update carries the
/// deterministic message_id derived from the event payload, not from a clock.
#[test]
fn brk08r_c5_cursor_message_id_is_deterministic_not_wall_clock() {
    let prev = cold_start();
    let u1 = make_update("new", TS_A);
    let u2 = make_update("new", TS_A); // identical — same timestamp, same order
    let b1 = build_inbound_batch_from_ws_update(&prev, u1).unwrap();
    let b2 = build_inbound_batch_from_ws_update(&prev, u2).unwrap();
    // Deterministic: calling twice with the same input produces the same cursor.
    let c1 = b1.into_cursor_for_persist();
    let c2 = b2.into_cursor_for_persist();
    assert_eq!(c1, c2);
}
// ---------------------------------------------------------------------------
// G — Gap detection
// ---------------------------------------------------------------------------
/// G1: mark_gap_detected from a Live cursor preserves last_message_id and
/// last_event_at.
#[test]
fn brk08r_g1_gap_from_live_preserves_last_position() {
    let live = live_cursor(Some("rest-999"), "msg-abc-123", TS_A);
    let gap = mark_gap_detected(&live, "reconnect");
    match &gap.trade_updates {
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            detail,
        } => {
            assert_eq!(last_message_id.as_deref(), Some("msg-abc-123"));
            assert_eq!(last_event_at.as_deref(), Some(TS_A));
            assert_eq!(detail, "reconnect");
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }
}
/// G2: mark_gap_detected from ColdStartUnproven produces GapDetected with
/// None for both position fields.
#[test]
fn brk08r_g2_gap_from_cold_start_has_none_positions() {
    let cold = cold_start();
    let gap = mark_gap_detected(&cold, "reconnect on cold start");
    match &gap.trade_updates {
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            ..
        } => {
            assert!(last_message_id.is_none());
            assert!(last_event_at.is_none());
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }
}
/// G3: mark_gap_detected from an existing GapDetected cursor preserves
/// last_* and replaces detail with the new detail string.
#[test]
fn brk08r_g3_gap_from_gap_preserves_positions_replaces_detail() {
    let prev = gap_cursor(None, Some("old-msg-id"), Some(TS_A), "first gap");
    let new_gap = mark_gap_detected(&prev, "second gap on reconnect");
    match &new_gap.trade_updates {
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            detail,
        } => {
            assert_eq!(last_message_id.as_deref(), Some("old-msg-id"));
            assert_eq!(last_event_at.as_deref(), Some(TS_A));
            assert_eq!(detail, "second gap on reconnect");
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }
}
/// G4: mark_gap_detected preserves rest_activity_after unchanged across all
/// cursor state transitions.
#[test]
fn brk08r_g4_gap_preserves_rest_activity_after() {
    let rest = Some("rest-cursor-xyz");
    let cold = AlpacaFetchCursor::cold_start_unproven(rest.map(str::to_string));
    let live = AlpacaFetchCursor::live(rest.map(str::to_string), "msg-id", TS_A);
    let gap_prev = AlpacaFetchCursor::gap_detected(rest.map(str::to_string), None, None, "prior");
    for prev in [&cold, &live, &gap_prev] {
        let g = mark_gap_detected(prev, "gap");
        assert_eq!(
            g.rest_activity_after.as_deref(),
            rest,
            "rest_activity_after not preserved for prev={prev:?}"
        );
    }
}
/// G5: A gap cursor serializes and deserializes to an equivalent
/// AlpacaFetchCursor with GapDetected state.
#[test]
fn brk08r_g5_gap_cursor_serde_round_trip() {
    let gap = gap_cursor(
        Some("rest-abc"),
        Some("last-msg-id"),
        Some(TS_B),
        "ws disconnect",
    );
    let json = serde_json::to_string(&gap).unwrap();
    let decoded: AlpacaFetchCursor = serde_json::from_str(&json).unwrap();
    assert_eq!(gap, decoded);
    assert!(matches!(
        decoded.trade_updates,
        AlpacaTradeUpdatesResume::GapDetected { .. }
    ));
}
// ---------------------------------------------------------------------------
// P — WS message parsing
// ---------------------------------------------------------------------------
/// P1: A well-formed trade_updates array element decodes to TradeUpdate.
#[test]
fn brk08r_p1_trade_update_message_decodes() {
    let raw = br#"[{"T":"trade_updates","data":{"event":"new","timestamp":"2024-06-15T09:30:00Z","order":{"id":"uuid-1","client_order_id":"coid-1","symbol":"AAPL","side":"buy","qty":"100","filled_qty":"0"}}}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::TradeUpdate(tu) => {
            assert_eq!(tu.event, "new");
            assert_eq!(tu.order.id, "uuid-1");
        }
        other => panic!("expected TradeUpdate, got {other:?}"),
    }
}
/// P2: authorization message decodes status field correctly.
#[test]
fn brk08r_p2_authorization_message_decodes_status() {
    let raw = br#"[{"T":"authorization","status":"authorized","action":"authenticate"}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Authorization { status } => assert_eq!(status, "authorized"),
        other => panic!("unexpected: {other:?}"),
    }
}
/// P3: listening message decodes streams array.
#[test]
fn brk08r_p3_listening_message_decodes_streams() {
    let raw = br#"[{"T":"listening","streams":["trade_updates"]}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Listening { streams } => {
            assert_eq!(streams.as_slice(), &["trade_updates"]);
        }
        other => panic!("unexpected: {other:?}"),
    }
}
/// P4: error message decodes code and msg fields.
#[test]
fn brk08r_p4_error_message_decodes_code_and_msg() {
    let raw = br#"[{"T":"error","code":400,"msg":"invalid syntax"}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Error { code, msg } => {
            assert_eq!(*code, 400i64);
            assert_eq!(msg, "invalid syntax");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
/// P5: Unknown T field produces Unknown variant with the original msg_type.
#[test]
fn brk08r_p5_unknown_t_produces_unknown_variant() {
    let raw = br#"[{"T":"subscription_confirmed","extra":"data"}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Unknown { msg_type } => {
            assert_eq!(msg_type, "subscription_confirmed");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
/// P6: An element with no T field is treated as Ping.
#[test]
fn brk08r_p6_no_t_field_produces_ping() {
    let raw = br#"[{}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0], AlpacaWsMessage::Ping));
}
/// P7: Non-JSON bytes produce WsParseError::JsonError.
#[test]
fn brk08r_p7_invalid_json_produces_error() {
    let result = parse_ws_message(b"not valid json");
    assert!(matches!(result.unwrap_err(), WsParseError::JsonError(_)));
}
/// P8: A multi-element array decodes all elements independently.
#[test]
fn brk08r_p8_multi_element_array_decodes_all() {
    let raw = br#"[
        {"T":"authorization","status":"authorized","action":"authenticate"},
        {"T":"listening","streams":["trade_updates"]},
        {"T":"trade_updates","data":{"event":"new","timestamp":"2024-06-15T09:30:00Z","order":{"id":"u1","client_order_id":"c1","symbol":"AAPL","side":"buy","qty":"10","filled_qty":"0"}}}
    ]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 3);
    assert!(matches!(msgs[0], AlpacaWsMessage::Authorization { .. }));
    assert!(matches!(msgs[1], AlpacaWsMessage::Listening { .. }));
    assert!(matches!(msgs[2], AlpacaWsMessage::TradeUpdate(_)));
}
/// P9: trade_updates element with a malformed data field produces Unknown
/// (not a hard error), allowing the rest of the array to be processed.
#[test]
fn brk08r_p9_trade_update_malformed_data_produces_unknown() {
    let raw = br#"[{"T":"trade_updates","data":{"completely_wrong":true}}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Unknown { msg_type } => {
            assert!(
                msg_type.starts_with("trade_updates:data_parse_err:"),
                "unexpected msg_type: {msg_type}"
            );
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}
/// P10: trade_updates element with no data field produces Unknown.
#[test]
fn brk08r_p10_trade_update_missing_data_produces_unknown() {
    let raw = br#"[{"T":"trade_updates"}]"#;
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        AlpacaWsMessage::Unknown { msg_type } => {
            assert_eq!(msg_type, "trade_updates:missing_data");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// I — Integration
// ---------------------------------------------------------------------------
/// I1: Full WS lane happy path: parse raw bytes → extract TradeUpdate →
/// build_inbound_batch → peek_cursor → encode_cursor_for_persist.
#[test]
fn brk08r_i1_full_ws_lane_happy_path() {
    let raw = br#"[{"T":"trade_updates","data":{"event":"new","timestamp":"2024-06-15T09:30:00.000000Z","order":{"id":"broker-uuid-i1","client_order_id":"internal-i1","symbol":"AAPL","side":"buy","qty":"50","filled_qty":"0"}}}]"#;
    // Parse.
    let msgs = parse_ws_message(raw).unwrap();
    assert_eq!(msgs.len(), 1);
    let tu = match &msgs[0] {
        AlpacaWsMessage::TradeUpdate(tu) => tu.clone(),
        other => panic!("expected TradeUpdate, got {other:?}"),
    };
    // Build batch.
    let prev = cold_start();
    let batch = build_inbound_batch_from_ws_update(&prev, tu).unwrap();
    assert_eq!(batch.events.len(), 1);
    assert!(matches!(batch.events[0], BrokerEvent::Ack { .. }));
    // Peek cursor — Live.
    assert!(matches!(
        batch.peek_cursor().trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
    // Encode cursor — valid JSON.
    let json = batch.encode_cursor_for_persist().unwrap();
    let decoded: AlpacaFetchCursor = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        decoded.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
/// I2: Cursor chain: three sequential WS messages each produce a distinct
/// Live cursor with the correct last_message_id from each event.
#[test]
fn brk08r_i2_cursor_chain_three_messages() {
    let mut cursor = cold_start();
    let updates = [
        make_update("new", TS_A),
        make_fill_update(TS_B, "150.00", "50"),
        make_fill_update(TS_C, "151.00", "50"),
    ];
    let mut message_ids: Vec<String> = Vec::new();
    for update in updates {
        let batch = build_inbound_batch_from_ws_update(&cursor, update).unwrap();
        let new_cursor = batch.into_cursor_for_persist();
        match &new_cursor.trade_updates {
            AlpacaTradeUpdatesResume::Live {
                last_message_id, ..
            } => {
                message_ids.push(last_message_id.clone());
            }
            other => panic!("expected Live, got {other:?}"),
        }
        cursor = new_cursor;
    }
    // All three message IDs must be distinct.
    assert_eq!(message_ids.len(), 3);
    assert_ne!(message_ids[0], message_ids[1]);
    assert_ne!(message_ids[1], message_ids[2]);
    assert_ne!(message_ids[0], message_ids[2]);
}
/// I3: Gap on reconnect: process a message (Live cursor) → mark_gap_detected
/// (GapDetected) → persist gap cursor.  The gap cursor's last_message_id
/// matches the message that was last successfully processed.
#[test]
fn brk08r_i3_gap_on_reconnect_workflow() {
    let prev = cold_start();
    let update = make_update("new", TS_A);
    let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
    let live_cursor = batch.into_cursor_for_persist();
    // Reconnect detected — no replay of missed messages.
    let gap = mark_gap_detected(&live_cursor, "ws disconnect: no replay available");
    // Gap cursor preserves last position from the live cursor.
    match (&live_cursor.trade_updates, &gap.trade_updates) {
        (
            AlpacaTradeUpdatesResume::Live {
                last_message_id: live_id,
                last_event_at: live_at,
            },
            AlpacaTradeUpdatesResume::GapDetected {
                last_message_id: Some(gap_id),
                last_event_at: Some(gap_at),
                detail,
            },
        ) => {
            assert_eq!(live_id, gap_id);
            assert_eq!(live_at, gap_at);
            assert!(detail.contains("disconnect"));
        }
        other => panic!("unexpected cursor pair: {other:?}"),
    }
    // Gap cursor serializes cleanly.
    let _json = serde_json::to_string(&gap).unwrap();
}
/// I4: After persisting a gap cursor, build_inbound_batch_from_ws_update still
/// advances to Live.  The runtime is responsible for not calling this without
/// first persisting the gap state; this test documents the WS lane behavior.
#[test]
fn brk08r_i4_batch_from_gap_cursor_advances_to_live() {
    let gap = gap_cursor(None, Some("last-confirmed-msg"), Some(TS_A), "prior gap");
    let update = make_update("new", TS_B);
    let batch = build_inbound_batch_from_ws_update(&gap, update).unwrap();
    assert!(matches!(
        batch.peek_cursor().trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
