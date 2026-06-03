//! BRK-BLOCKING-01 proof tests — async dispatch path.
//!
//! Proves that the Alpaca adapter's sync trait methods dispatch correctly
//! from within a Tokio multi-thread async context, using the
//! `block_in_place → handle.block_on` path in `run_async`.
//!
//! All four tests use `#[tokio::test(flavor = "multi_thread")]` to exercise
//! the path that was previously impossible (reqwest::blocking panicked when
//! its internal runtime dropped inside a Tokio async context).  With async
//! reqwest there is no embedded runtime to drop.
//!
//! # Coverage
//!
//! BK01  submit_order from async multi-thread context: returns Transport or
//!       AmbiguousSubmit on an unreachable URL — does not panic.
//! BK02  cancel_order from async multi-thread context: returns Transport on
//!       an unreachable URL — does not panic.
//! BK03  fetch_events with cold-start cursor: returns InboundContinuityUnproven
//!       without touching the network (continuity gate fires before HTTP call).
//! BK04  fetch_events with live cursor + unreachable URL: returns Transport
//!       from within async multi-thread context — does not panic.

use mqk_broker_alpaca::{
    encode_fetch_cursor, types::AlpacaFetchCursor, AlpacaBrokerAdapter, AlpacaConfig,
};
use mqk_execution::{
    AssetClass, BrokerAdapter, BrokerError, BrokerInvokeToken, BrokerSubmitRequest, Side,
};

fn unreachable_adapter() -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
    })
}

fn live_cursor() -> String {
    encode_fetch_cursor(&AlpacaFetchCursor::live(
        None,
        "prev-msg-id",
        "2024-01-15T10:00:00Z",
    ))
    .unwrap()
}

// ---------------------------------------------------------------------------
// BK01 — submit_order from async multi-thread context
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bk01_submit_order_from_async_multi_thread_returns_transport_not_panic() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let req = BrokerSubmitRequest {
        order_id: "bk01-order".to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: 1,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class: AssetClass::Equity,
    };
    // Calling the sync adapter method directly from an async task body.
    // run_async uses block_in_place → handle.block_on internally.
    // Must not panic; must return Transport or AmbiguousSubmit.
    let result = adapter.submit_order(req, &token);
    assert!(result.is_err(), "BK01: unreachable URL must return error");
    match result.unwrap_err() {
        BrokerError::Transport { .. } | BrokerError::AmbiguousSubmit { .. } => {}
        other => panic!("BK01: expected Transport or AmbiguousSubmit, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BK02 — cancel_order from async multi-thread context
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bk02_cancel_order_from_async_multi_thread_returns_transport_not_panic() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let result = adapter.cancel_order("bk02-broker-order-id", &token);
    assert!(result.is_err(), "BK02: unreachable URL must return error");
    match result.unwrap_err() {
        BrokerError::Transport { .. } => {}
        other => panic!("BK02: expected Transport, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BK03 — fetch_events cold-start cursor: continuity gate fires before network
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bk03_fetch_events_cold_start_cursor_returns_continuity_unproven_no_network() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    // None cursor decodes as ColdStartUnproven — the continuity gate fires
    // before any HTTP call is made, so the unreachable URL is never reached.
    let result = adapter.fetch_events(None, &token);
    assert!(result.is_err(), "BK03: cold-start must fail closed");
    match result.unwrap_err() {
        BrokerError::InboundContinuityUnproven { .. } => {}
        other => panic!("BK03: expected InboundContinuityUnproven, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BK04 — fetch_events live cursor + unreachable URL: Transport from async context
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bk04_fetch_events_live_cursor_unreachable_returns_transport_not_panic() {
    let adapter = unreachable_adapter();
    let token = BrokerInvokeToken::for_test();
    let cursor = live_cursor();
    // Live cursor passes the continuity gate; the adapter attempts the network
    // call.  Must return Transport — not panic due to embedded runtime drop.
    let result = adapter.fetch_events(Some(&cursor), &token);
    assert!(result.is_err(), "BK04: unreachable URL must return error");
    match result.unwrap_err() {
        BrokerError::Transport { .. } => {}
        other => panic!("BK04: expected Transport, got {other:?}"),
    }
}
