//! Alpaca inbound broker lane — BRK-00R / BRK-01R / BRK-02R / BRK-07R.
//!
//! # Contract summary
//!
//! ## BRK-00R — InboundBatch
//!
//! `InboundBatch` packages broker events + the candidate cursor together.
//! The cursor field is private; it can only be retrieved via
//! `into_cursor_for_persist` or `encode_cursor_for_persist`, both of which
//! consume the batch.  This naming enforces the invariant at the call site:
//!
//! ```text
//! 1. receive InboundBatch from build_inbound_batch_from_ws_update
//! 2. durably ingest all batch.events into the inbox
//! 3. ONLY THEN call batch.encode_cursor_for_persist() and persist cursor
//! ```
//!
//! The cursor MUST NOT be persisted before step 2 completes.  Persisting an
//! advanced cursor before the events are durable creates a continuity gap:
//! if the process crashes between persist-cursor and inbox-ingest, the events
//! are permanently lost and the gap cannot be detected.
//!
//! ## BRK-01R — WS message parsing
//!
//! `parse_ws_message` accepts raw websocket bytes and returns a typed list of
//! `AlpacaWsMessage` variants.  Trade-update payloads are further processed
//! by `build_inbound_batch_from_ws_update` to produce a normalized
//! `InboundBatch`.
//!
//! ## BRK-02R — Cursor-after-ingest ordering
//!
//! The private cursor field on `InboundBatch` is the only BRK-02R enforcement
//! mechanism this layer provides.  The runtime is responsible for calling
//! `encode_cursor_for_persist` only after `inbox_insert_deduped_with_identity`
//! (or equivalent) has committed for every event in the batch.
//!
//! ## BRK-07R — Fail closed on continuity uncertainty
//!
//! `mark_gap_detected` demotes any cursor to `GapDetected`.  The adapter's
//! `fetch_events` already fails closed when it sees `ColdStartUnproven` or
//! `GapDetected` (see lib.rs `continuity_fail_closed`).  Callers that detect
//! a reconnect with missed messages MUST call `mark_gap_detected` and persist
//! the resulting cursor before resuming normal operation.
use crate::normalize::{normalize_trade_update, trade_update_message_id};
use crate::types::{AlpacaFetchCursor, AlpacaTradeUpdate, AlpacaTradeUpdatesResume};
use mqk_execution::{BrokerError, BrokerEvent};
// ---------------------------------------------------------------------------
// InboundBatch — BRK-00R
// ---------------------------------------------------------------------------
/// A batch of normalized broker events paired with their candidate cursor.
///
/// # Invariant (BRK-02R)
///
/// The cursor MUST NOT be extracted (via `into_cursor_for_persist` or
/// `encode_cursor_for_persist`) until every event in `events` has been
/// durably committed to the inbox.  Violating this ordering creates a
/// continuity gap that the system cannot detect or recover from automatically.
///
/// The cursor field is intentionally private; the only exit paths are the
/// consuming methods named `*_for_persist`, making the intended ordering
/// visible at every call site.
pub struct InboundBatch {
    /// Normalized broker events to ingest into the inbox.
    pub events: Vec<BrokerEvent>,
    /// Candidate cursor — private until events are durably ingested.
    cursor: AlpacaFetchCursor,
}
impl InboundBatch {
    /// Construct a new batch.
    ///
    /// `pub(crate)` so only this module and lib.rs can build batches; external
    /// code must go through the public factory functions.
    pub(crate) fn new(events: Vec<BrokerEvent>, cursor: AlpacaFetchCursor) -> Self {
        Self { events, cursor }
    }
    /// Consume the batch and return the raw cursor for persistence.
    ///
    /// **Call this ONLY after all `events` have been durably ingested.**
    pub fn into_cursor_for_persist(self) -> AlpacaFetchCursor {
        self.cursor
    }
    /// Consume the batch and encode the cursor as a JSON string for persistence.
    ///
    /// **Call this ONLY after all `events` have been durably ingested.**
    ///
    /// Returns `Err(BrokerError::Transient)` if cursor serialization fails.
    pub fn encode_cursor_for_persist(self) -> Result<String, BrokerError> {
        serde_json::to_string(&self.cursor).map_err(|e| BrokerError::Transient {
            detail: format!("inbound: failed to serialize cursor: {e}"),
        })
    }
    /// Borrow the cursor without consuming the batch.
    ///
    /// Intended for testing and logging only.  The naming deliberately does not
    /// include `persist` to avoid confusion with the consume paths.
    pub fn peek_cursor(&self) -> &AlpacaFetchCursor {
        &self.cursor
    }
}
// ---------------------------------------------------------------------------
// AlpacaWsMessage — BRK-01R
// ---------------------------------------------------------------------------
/// A decoded message from the Alpaca websocket trade-update stream.
///
/// Alpaca sends JSON arrays; each element has a `"T"` discriminant field.
///
/// ```text
/// [{"T":"trade_updates","data":{...order lifecycle event...}}]
/// [{"T":"authorization","status":"authorized","action":"authenticate"}]
/// [{"T":"listening","streams":["trade_updates"]}]
/// [{"T":"error","code":400,"msg":"invalid syntax"}]
/// ```
// The TradeUpdate variant is intentionally large: boxing every inbound WS message
// in the single-threaded parse path adds allocation overhead for no safety gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AlpacaWsMessage {
    /// An order lifecycle event from the trade-update stream.
    TradeUpdate(AlpacaTradeUpdate),
    /// Broker authorization response.
    Authorization { status: String },
    /// Broker listening-confirmation response.
    Listening { streams: Vec<String> },
    /// Broker-reported protocol error.
    Error { code: i64, msg: String },
    /// Websocket-level ping frame; no `T` field present.
    Ping,
    /// Unknown `T` type; carries the type string for diagnostics.
    Unknown { msg_type: String },
}
/// Error returned when raw websocket bytes cannot be parsed as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsParseError {
    /// The raw bytes were not valid JSON or were not a JSON array.
    JsonError(String),
}
impl std::fmt::Display for WsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsParseError::JsonError(e) => write!(f, "ws parse error: {e}"),
        }
    }
}
impl std::error::Error for WsParseError {}
/// Parse raw websocket bytes into a list of typed `AlpacaWsMessage` values.
///
/// Handles both wire formats sent by Alpaca WebSocket endpoints:
///
/// - **v2 array format** (market-data and newer endpoints): JSON arrays where
///   each element carries a `"T"` discriminant field.
///   ```text
///   [{"T":"authorization","status":"authorized","action":"authenticate"}]
///   [{"T":"listening","streams":["trade_updates"]}]
///   [{"T":"trade_updates","data":{...}}]
///   ```
///
/// - **v1 object format** (paper/live trading stream at `/stream`): a single
///   JSON object with a `"stream"` discriminant and a nested `"data"` field.
///   ```text
///   {"stream":"authorization","data":{"action":"authenticate","status":"authorized"}}
///   {"stream":"listening","data":{"streams":["trade_updates"]}}
///   {"stream":"trade_updates","data":{...}}
///   ```
///
/// v1 objects are normalized to the same internal element shape as v2 array
/// elements before per-element decoding, so the rest of the pipeline is
/// format-agnostic.
///
/// An element whose `T` field is not recognized becomes `Unknown`.  An
/// element whose `T` is `"trade_updates"` but whose `data` field fails to
/// deserialize as `AlpacaTradeUpdate` is also returned as `Unknown` (the
/// failure detail is encoded in `msg_type`) rather than aborting the whole
/// message parse.
///
/// An element with no `T` field at all is treated as `Ping` (protocol ping
/// frame).
///
/// # Errors
///
/// Returns `WsParseError::JsonError` only if the top-level bytes are not
/// valid JSON or are neither an array nor an object.  Per-element decode
/// errors are represented as `Unknown` variants, not as errors.
pub fn parse_ws_message(raw: &[u8]) -> Result<Vec<AlpacaWsMessage>, WsParseError> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| WsParseError::JsonError(e.to_string()))?;
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr,
        obj @ serde_json::Value::Object(_) => {
            // v1 trading stream: single JSON object with "stream" discriminant.
            // Normalize to the element shape that per-item decode expects.
            vec![v1_trading_obj_to_element(obj)]
        }
        _ => {
            return Err(WsParseError::JsonError(
                "expected JSON array or object at top level".to_string(),
            ));
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let msg_type_opt = item.get("T").and_then(|v| v.as_str());
        let msg = match msg_type_opt {
            None => AlpacaWsMessage::Ping,
            Some("trade_updates") => match item.get("data") {
                Some(data) => match serde_json::from_value::<AlpacaTradeUpdate>(data.clone()) {
                    Ok(tu) => AlpacaWsMessage::TradeUpdate(tu),
                    Err(e) => AlpacaWsMessage::Unknown {
                        msg_type: format!("trade_updates:data_parse_err:{e}"),
                    },
                },
                None => AlpacaWsMessage::Unknown {
                    msg_type: "trade_updates:missing_data".to_string(),
                },
            },
            Some("authorization") => {
                let status = item
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                AlpacaWsMessage::Authorization { status }
            }
            Some("listening") => {
                let streams = item
                    .get("streams")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                AlpacaWsMessage::Listening { streams }
            }
            Some("error") => {
                let code = item.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
                let msg = item
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                AlpacaWsMessage::Error { code, msg }
            }
            Some(other) => AlpacaWsMessage::Unknown {
                msg_type: other.to_string(),
            },
        };
        out.push(msg);
    }
    Ok(out)
}
/// Normalize one Alpaca v1 trading-stream object to the per-element shape that
/// the `parse_ws_message` decode logic expects (`"T"` discriminant, flat fields).
///
/// v1 format:  `{"stream": "<type>", "data": { ...fields... }}`
/// normalized: `{"T": "<type>", ...data fields hoisted to top level or kept
///               under "data" depending on the variant...}`
///
/// This is a pure structural conversion — no semantic changes.
fn v1_trading_obj_to_element(obj: serde_json::Value) -> serde_json::Value {
    let stream = obj.get("stream").and_then(|v| v.as_str());
    let data = obj.get("data");
    match stream {
        None => {
            // No "stream" field → treat as protocol ping (same as no "T" in v2).
            serde_json::json!({})
        }
        Some("authorization") => {
            let status = data
                .and_then(|d| d.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let action = data
                .and_then(|d| d.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({"T": "authorization", "status": status, "action": action})
        }
        Some("listening") => {
            let streams = data
                .and_then(|d| d.get("streams"))
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            serde_json::json!({"T": "listening", "streams": streams})
        }
        Some("trade_updates") => {
            let data_val = data.cloned().unwrap_or(serde_json::Value::Null);
            serde_json::json!({"T": "trade_updates", "data": data_val})
        }
        Some("error") => {
            let code = data.and_then(|d| d.get("code")).and_then(|v| v.as_i64()).unwrap_or(0);
            let msg = data
                .and_then(|d| d.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            serde_json::json!({"T": "error", "code": code, "msg": msg})
        }
        Some(other) => serde_json::json!({"T": other}),
    }
}

// ---------------------------------------------------------------------------
// build_inbound_batch_from_ws_update — BRK-01R
// ---------------------------------------------------------------------------
/// Normalize one Alpaca websocket trade-update and produce an `InboundBatch`.
///
/// # Cursor advancement (BRK-02R)
///
/// The resulting batch carries a `Live` cursor with:
/// - `last_message_id` = `trade_update_message_id(&update)` (deterministic,
///   derived from the event payload, never from the wall clock).
/// - `last_event_at` = `update.timestamp`.
/// - `rest_activity_after` preserved from `prev_cursor` unchanged.
///
/// The batch cursor MUST NOT be persisted until all events in
/// `InboundBatch::events` have been durably committed to the inbox.
///
/// # Errors
///
/// Returns `BrokerError::Transient` if normalization fails (unknown event
/// type, unparseable field, etc.).  The cursor is NOT advanced on error;
/// callers that need to signal a gap should call `mark_gap_detected` after
/// receiving a normalization error on a known-live event sequence.
pub fn build_inbound_batch_from_ws_update(
    prev_cursor: &AlpacaFetchCursor,
    update: AlpacaTradeUpdate,
) -> Result<InboundBatch, BrokerError> {
    let message_id = trade_update_message_id(&update);
    let event_at = update.timestamp.clone();
    let event = normalize_trade_update(&update).map_err(|e| BrokerError::Transient {
        detail: format!("inbound: normalize error: {e}"),
    })?;
    let new_cursor = AlpacaFetchCursor::live(
        prev_cursor.rest_activity_after.clone(),
        message_id,
        event_at,
    );
    Ok(InboundBatch::new(vec![event], new_cursor))
}
// ---------------------------------------------------------------------------
// mark_gap_detected — BRK-07R
// ---------------------------------------------------------------------------
/// Demote the current cursor to `GapDetected`, preserving the last known
/// position from a `Live` cursor.
///
/// # When to call this (BRK-07R)
///
/// Call `mark_gap_detected` whenever the inbound lane detects that lifecycle
/// continuity can no longer be proven:
///
/// - Websocket disconnect followed by reconnect with no replay of missed messages.
/// - Sequence gap detected in received message IDs.
/// - Any condition where the set of events since the last persisted cursor is
///   unknown or potentially incomplete.
///
/// After `mark_gap_detected`, the returned cursor carries `GapDetected`.
/// The adapter's `fetch_events` (and the overall inbound lane) fail closed
/// when they see this state, ensuring the OMS halts rather than proceeding
/// with potentially incomplete lifecycle information.
///
/// # Cursor state transitions
///
/// | Previous state        | After `mark_gap_detected`        |
/// |-----------------------|----------------------------------|
/// | `ColdStartUnproven`   | `GapDetected { last_* = None }`  |
/// | `Live { ... }`        | `GapDetected { last_* = Some }`  |
/// | `GapDetected { ... }` | `GapDetected` (preserves last_*, replaces detail) |
///
/// The `rest_activity_after` component of the cursor is always preserved
/// unchanged.
pub fn mark_gap_detected(
    prev_cursor: &AlpacaFetchCursor,
    detail: impl Into<String>,
) -> AlpacaFetchCursor {
    let (last_message_id, last_event_at) = match &prev_cursor.trade_updates {
        AlpacaTradeUpdatesResume::Live {
            last_message_id,
            last_event_at,
        } => (Some(last_message_id.clone()), Some(last_event_at.clone())),
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            ..
        } => (last_message_id.clone(), last_event_at.clone()),
        AlpacaTradeUpdatesResume::ColdStartUnproven => (None, None),
    };
    AlpacaFetchCursor::gap_detected(
        prev_cursor.rest_activity_after.clone(),
        last_message_id,
        last_event_at,
        detail,
    )
}
// ---------------------------------------------------------------------------
// Unit tests (no network, no DB, no wall-clock reads)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AlpacaOrder;
    fn make_order(id: &str, client_id: &str) -> AlpacaOrder {
        AlpacaOrder {
            id: id.to_string(),
            client_order_id: client_id.to_string(),
            symbol: "AAPL".to_string(),
            side: "buy".to_string(),
            qty: "100".to_string(),
            filled_qty: "0".to_string(),
        }
    }
    fn make_trade_update(
        event: &str,
        order_id: &str,
        client_id: &str,
        ts: &str,
    ) -> AlpacaTradeUpdate {
        AlpacaTradeUpdate {
            event: event.to_string(),
            timestamp: ts.to_string(),
            order: make_order(order_id, client_id),
            price: None,
            qty: None,
            broker_fill_id: None,
        }
    }
    // -------------------------------------------------------------------
    // InboundBatch / cursor contract
    // -------------------------------------------------------------------
    #[test]
    fn batch_from_cold_start_advances_cursor_to_live() {
        let prev = AlpacaFetchCursor::cold_start_unproven(None);
        let update = make_trade_update("new", "uuid-1", "coid-1", "2024-01-01T00:00:00Z");
        let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
        assert_eq!(batch.events.len(), 1);
        assert!(matches!(
            batch.peek_cursor().trade_updates,
            AlpacaTradeUpdatesResume::Live { .. }
        ));
    }
    #[test]
    fn batch_cursor_carries_message_id_and_event_at() {
        let prev = AlpacaFetchCursor::cold_start_unproven(None);
        let ts = "2024-06-15T09:30:00.000000Z";
        let update = make_trade_update("new", "alpaca-uuid-001", "internal-001", ts);
        let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
        let cursor = batch.into_cursor_for_persist();
        match &cursor.trade_updates {
            AlpacaTradeUpdatesResume::Live {
                last_message_id,
                last_event_at,
            } => {
                assert_eq!(
                    last_message_id,
                    "alpaca:alpaca-uuid-001:new:2024-06-15T09:30:00.000000Z"
                );
                assert_eq!(last_event_at, ts);
            }
            other => panic!("expected Live, got {other:?}"),
        }
    }
    #[test]
    fn batch_preserves_rest_activity_after() {
        let prev = AlpacaFetchCursor::live(
            Some("rest-cursor-abc".to_string()),
            "prev-msg-id",
            "2024-01-01T00:00:00Z",
        );
        let update = make_trade_update("new", "uuid-1", "coid-1", "2024-01-02T00:00:00Z");
        let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
        let cursor = batch.into_cursor_for_persist();
        assert_eq!(
            cursor.rest_activity_after.as_deref(),
            Some("rest-cursor-abc")
        );
    }
    #[test]
    fn encode_cursor_for_persist_produces_valid_json() {
        let prev = AlpacaFetchCursor::cold_start_unproven(None);
        let update = make_trade_update("new", "uuid-1", "coid-1", "2024-01-01T00:00:00Z");
        let batch = build_inbound_batch_from_ws_update(&prev, update).unwrap();
        let json = batch.encode_cursor_for_persist().unwrap();
        // Must round-trip through serde.
        let decoded: AlpacaFetchCursor = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded.trade_updates,
            AlpacaTradeUpdatesResume::Live { .. }
        ));
    }
    // -------------------------------------------------------------------
    // mark_gap_detected transitions (BRK-07R)
    // -------------------------------------------------------------------
    #[test]
    fn gap_from_live_preserves_last_position() {
        let live = AlpacaFetchCursor::live(
            Some("rest-123".to_string()),
            "last-msg-id",
            "2024-01-01T12:00:00Z",
        );
        let gap = mark_gap_detected(&live, "disconnect");
        match &gap.trade_updates {
            AlpacaTradeUpdatesResume::GapDetected {
                last_message_id,
                last_event_at,
                detail,
            } => {
                assert_eq!(last_message_id.as_deref(), Some("last-msg-id"));
                assert_eq!(last_event_at.as_deref(), Some("2024-01-01T12:00:00Z"));
                assert_eq!(detail, "disconnect");
            }
            other => panic!("expected GapDetected, got {other:?}"),
        }
        assert_eq!(gap.rest_activity_after.as_deref(), Some("rest-123"));
    }
    #[test]
    fn gap_from_cold_start_has_no_last_position() {
        let cold = AlpacaFetchCursor::cold_start_unproven(None);
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
    #[test]
    fn gap_from_existing_gap_preserves_last_position_replaces_detail() {
        let prev_gap = AlpacaFetchCursor::gap_detected(
            None,
            Some("old-msg-id".to_string()),
            Some("2024-01-01T00:00:00Z".to_string()),
            "original detail",
        );
        let new_gap = mark_gap_detected(&prev_gap, "new reconnect detail");
        match &new_gap.trade_updates {
            AlpacaTradeUpdatesResume::GapDetected {
                last_message_id,
                last_event_at,
                detail,
            } => {
                assert_eq!(last_message_id.as_deref(), Some("old-msg-id"));
                assert_eq!(last_event_at.as_deref(), Some("2024-01-01T00:00:00Z"));
                assert_eq!(detail, "new reconnect detail");
            }
            other => panic!("expected GapDetected, got {other:?}"),
        }
    }
    // -------------------------------------------------------------------
    // parse_ws_message (BRK-01R)
    // -------------------------------------------------------------------
    #[test]
    fn parse_trade_update_message() {
        let raw = br#"[{"T":"trade_updates","data":{"event":"new","timestamp":"2024-01-01T00:00:00Z","order":{"id":"uuid-1","client_order_id":"coid-1","symbol":"AAPL","side":"buy","qty":"100","filled_qty":"0"}}}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], AlpacaWsMessage::TradeUpdate(_)));
    }
    #[test]
    fn parse_authorization_message() {
        let raw = br#"[{"T":"authorization","status":"authorized","action":"authenticate"}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            AlpacaWsMessage::Authorization { status } => assert_eq!(status, "authorized"),
            other => panic!("unexpected: {other:?}"),
        }
    }
    #[test]
    fn parse_listening_message() {
        let raw = br#"[{"T":"listening","streams":["trade_updates"]}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            AlpacaWsMessage::Listening { streams } => {
                assert_eq!(streams, &["trade_updates"]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    #[test]
    fn parse_error_message() {
        let raw = br#"[{"T":"error","code":400,"msg":"invalid syntax"}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            AlpacaWsMessage::Error { code, msg } => {
                assert_eq!(*code, 400);
                assert_eq!(msg, "invalid syntax");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    #[test]
    fn parse_unknown_t_field() {
        let raw = br#"[{"T":"subscription_confirmed","streams":["trade_updates"]}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            AlpacaWsMessage::Unknown { msg_type } => {
                assert_eq!(msg_type, "subscription_confirmed");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    #[test]
    fn parse_ping_no_t_field() {
        let raw = br#"[{}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], AlpacaWsMessage::Ping));
    }
    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_ws_message(b"not json at all");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WsParseError::JsonError(_)));
    }
    #[test]
    fn parse_multi_element_array() {
        let raw = br#"[{"T":"authorization","status":"authorized"},{"T":"listening","streams":["trade_updates"]}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(matches!(msgs[0], AlpacaWsMessage::Authorization { .. }));
        assert!(matches!(msgs[1], AlpacaWsMessage::Listening { .. }));
    }
    #[test]
    fn parse_trade_update_bad_data_field_returns_unknown() {
        // "data" is present but not a valid AlpacaTradeUpdate shape.
        let raw = br#"[{"T":"trade_updates","data":{"broken":true}}]"#;
        let msgs = parse_ws_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            AlpacaWsMessage::Unknown { msg_type } => {
                assert!(msg_type.starts_with("trade_updates:data_parse_err:"));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }
    #[test]
    fn parse_trade_update_missing_data_field_returns_unknown() {
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
}
