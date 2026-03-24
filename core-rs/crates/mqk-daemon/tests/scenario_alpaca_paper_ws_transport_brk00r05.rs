//! BRK-00R-05: Alpaca paper WebSocket transport proof.
//!
//! Proves the pure protocol-building functions and spawn-guard behavior
//! for the paper WS transport introduced in BRK-00R-05.
//!
//! # What this proves
//!
//! ```text
//! T01: build_ws_auth_message  → canonical JSON wire format
//! T02: build_ws_subscribe_message → canonical JSON wire format
//! T03: ws_url_from_base_url   → correct wss:// endpoint derivation
//! T04: auth response parsing  → AlpacaWsMessage::Authorization{status:"authorized"}
//! T05: listening response parsing → AlpacaWsMessage::Listening{streams:["trade_updates"]}
//! T06: spawn returns None for paper+paper (wrong broker)
//! T07: spawn returns None for live-shadow+alpaca (wrong mode)
//! T08: spawn returns None when credentials are absent (env guard)
//! ```
//!
//! All tests are pure in-process (no network required).

use std::sync::Arc;

use mqk_broker_alpaca::{parse_ws_message, AlpacaWsMessage};
use mqk_daemon::state;

// ---------------------------------------------------------------------------
// BRK00R05-T01 — build_ws_auth_message produces canonical JSON
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t01_ws_auth_message_is_canonical() {
    let msg = state::build_ws_auth_message("test-key-id", "test-secret");
    let v: serde_json::Value = serde_json::from_str(&msg).expect("T01: must be valid JSON");
    assert_eq!(v["action"], "auth", "T01: action must be 'auth'; got: {v}");
    assert_eq!(
        v["key"], "test-key-id",
        "T01: key must be 'test-key-id'; got: {v}"
    );
    assert_eq!(
        v["secret"], "test-secret",
        "T01: secret must be 'test-secret'; got: {v}"
    );
    // Only the three expected fields — no extras leaking secrets.
    let obj = v.as_object().expect("T01: must be a JSON object");
    assert_eq!(
        obj.len(),
        3,
        "T01: auth message must have exactly 3 fields; got: {v}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R05-T02 — build_ws_subscribe_message produces canonical JSON
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t02_ws_subscribe_message_is_canonical() {
    let msg = state::build_ws_subscribe_message();
    let v: serde_json::Value = serde_json::from_str(&msg).expect("T02: must be valid JSON");
    assert_eq!(
        v["action"], "listen",
        "T02: action must be 'listen'; got: {v}"
    );
    let streams = v["data"]["streams"]
        .as_array()
        .expect("T02: data.streams must be an array");
    assert_eq!(
        streams.len(),
        1,
        "T02: exactly one stream must be requested; got: {v}"
    );
    assert_eq!(
        streams[0], "trade_updates",
        "T02: stream must be 'trade_updates'; got: {v}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R05-T03 — ws_url_from_base_url transforms correctly
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t03_ws_url_from_base_url() {
    assert_eq!(
        state::ws_url_from_base_url("https://paper-api.alpaca.markets"),
        "wss://paper-api.alpaca.markets/stream",
        "T03: standard paper URL must map to /stream endpoint"
    );
    // Trailing slash must be stripped.
    assert_eq!(
        state::ws_url_from_base_url("https://paper-api.alpaca.markets/"),
        "wss://paper-api.alpaca.markets/stream",
        "T03: trailing slash must be stripped before appending /stream"
    );
    // Non-https base URL passes through with /stream appended.
    let custom = state::ws_url_from_base_url("wss://internal-mock:8080");
    assert!(
        custom.ends_with("/stream"),
        "T03: custom URL must end with /stream; got: {custom}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R05-T04 — parse_ws_message decodes Alpaca authorization response
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t04_auth_response_parsed_correctly() {
    // Alpaca authorization response wire format.
    let raw = br#"[{"T":"authorization","status":"authorized","action":"authenticate"}]"#;
    let msgs = parse_ws_message(raw).expect("T04: must parse authorization frame");
    assert_eq!(msgs.len(), 1, "T04: one message in frame; got: {msgs:?}");
    match &msgs[0] {
        AlpacaWsMessage::Authorization { status } => {
            assert_eq!(
                status, "authorized",
                "T04: status must be 'authorized'; got: {status}"
            );
        }
        other => panic!("T04: expected Authorization; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BRK00R05-T05 — parse_ws_message decodes Alpaca listening response
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t05_listening_response_parsed_correctly() {
    // Alpaca listening-confirmation wire format.
    let raw = br#"[{"T":"listening","streams":["trade_updates"]}]"#;
    let msgs = parse_ws_message(raw).expect("T05: must parse listening frame");
    assert_eq!(msgs.len(), 1, "T05: one message in frame; got: {msgs:?}");
    match &msgs[0] {
        AlpacaWsMessage::Listening { streams } => {
            assert!(
                streams.iter().any(|s| s == "trade_updates"),
                "T05: streams must contain 'trade_updates'; got: {streams:?}"
            );
        }
        other => panic!("T05: expected Listening; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BRK00R05-T06 — spawn returns None for paper+paper (wrong broker)
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t06_spawn_none_for_paper_paper() {
    // Default state is paper+paper; broker_kind = Paper.
    let st = Arc::new(state::AppState::new());
    // paper+paper → broker is Paper, not Alpaca → must return None.
    let handle = state::spawn_alpaca_paper_ws_task(Arc::clone(&st));
    assert!(
        handle.is_none(),
        "T06: paper+paper must not spawn WS task (broker is Paper, not Alpaca)"
    );
}

// ---------------------------------------------------------------------------
// BRK00R05-T07 — spawn returns None for live-shadow+alpaca (wrong mode)
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t07_spawn_none_for_live_shadow_alpaca() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    // live-shadow is not Paper → must return None.
    let handle = state::spawn_alpaca_paper_ws_task(Arc::clone(&st));
    assert!(
        handle.is_none(),
        "T07: live-shadow+alpaca must not spawn paper WS task (mode is not Paper)"
    );
}

// ---------------------------------------------------------------------------
// BRK00R05-T08 — spawn returns None when credentials are absent
// ---------------------------------------------------------------------------

#[test]
fn brk00r05_t08_spawn_none_when_credentials_absent() {
    // paper+alpaca config but no env vars set → spawn must return None.
    // This proves the task does not start without explicit credentials.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Remove env vars if set (CI/test environments may not have them).
    // env::remove_var is std — no harm if already absent.
    std::env::remove_var("ALPACA_API_KEY_PAPER");
    std::env::remove_var("ALPACA_API_SECRET_PAPER");

    let handle = state::spawn_alpaca_paper_ws_task(Arc::clone(&st));
    assert!(
        handle.is_none(),
        "T08: paper+alpaca without credentials must not spawn WS task"
    );
}
