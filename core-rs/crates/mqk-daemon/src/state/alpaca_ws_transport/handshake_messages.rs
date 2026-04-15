//! Pure WS handshake message builders — testable without network.
//!
//! These functions produce the JSON wire frames that the Alpaca trading stream
//! expects during session establishment.  They are pure (no I/O, no async) and
//! have no dependency on `AppState` or runtime cursor state.

/// Build the Alpaca WS authentication JSON message.
///
/// Wire format: `{"action":"auth","key":"<key>","secret":"<secret>"}`
pub fn build_ws_auth_message(key: &str, secret: &str) -> String {
    serde_json::json!({ "action": "auth", "key": key, "secret": secret }).to_string()
}

/// Build the Alpaca WS subscribe message for the `trade_updates` stream.
///
/// Wire format: `{"action":"listen","data":{"streams":["trade_updates"]}}`
pub fn build_ws_subscribe_message() -> String {
    serde_json::json!({
        "action": "listen",
        "data": { "streams": ["trade_updates"] }
    })
    .to_string()
}

/// Derive the Alpaca paper WS URL from the REST base URL.
///
/// Replaces `https://` with `wss://` and appends `/stream`.
///
/// ```text
/// "https://paper-api.alpaca.markets"   → "wss://paper-api.alpaca.markets/stream"
/// "https://paper-api.alpaca.markets/"  → "wss://paper-api.alpaca.markets/stream"
/// ```
pub fn ws_url_from_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let ws_base = if let Some(host) = trimmed.strip_prefix("https://") {
        format!("wss://{host}")
    } else {
        trimmed.to_string()
    };
    format!("{ws_base}/stream")
}
