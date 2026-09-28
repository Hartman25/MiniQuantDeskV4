//! M6 Alpaca crypto wire correctness (RC-M6-A).
//!
//! Provider facts these tests encode (Alpaca docs, crypto spot trading):
//! - orders use the slash pair form `BTC/USD`, while the positions list shows
//!   the consolidated asset symbol `BTCUSD` (asset_class `crypto`);
//! - crypto orders support only `gtc` and `ioc` time-in-force;
//! - fills carry exact fractional quantities.
//!
//! | ID   | Invariant                                                              |
//! |------|------------------------------------------------------------------------|
//! | W01  | crypto position `BTCUSD` normalizes to canonical `BTC/USD`             |
//! | W02  | non-crypto / unknown-class / already-canonical symbols pass through    |
//! | W03  | REST fractional crypto partial fill applies with exact `cum_qty_after` |
//! | W04  | fractional `cum_qty` on an Equity order still fails the page closed    |
//! | W05  | malformed `cum_qty` fails before any order lookup                      |
//! | W06  | crypto submit with unsupported time-in-force is refused pre-HTTP       |

use mqk_broker_alpaca::{
    encode_fetch_cursor, normalize_position,
    types::{AlpacaFetchCursor, AlpacaPositionRaw},
    AlpacaBrokerAdapter, AlpacaConfig,
};
use mqk_execution::{
    AssetClass, BrokerAdapter, BrokerError, BrokerInvokeToken, BrokerSubmitRequest, QtyMicros, Side,
};
use std::io::{Read, Write};
use std::net::TcpListener;

fn position(symbol: &str, asset_class: Option<&str>, qty: &str) -> AlpacaPositionRaw {
    let mut wire = serde_json::json!({
        "symbol": symbol,
        "qty": qty,
        "avg_entry_price": "60000",
    });
    if let Some(class) = asset_class {
        wire["asset_class"] = serde_json::json!(class);
    }
    serde_json::from_value(wire).expect("real Alpaca position wire shape must deserialize")
}

// ---------------------------------------------------------------------------
// W01 / W02 — position symbol canonicalization
// ---------------------------------------------------------------------------

#[test]
fn w01_crypto_position_symbol_is_canonicalized_to_the_order_pair_form() {
    for (wire, canonical) in [
        ("BTCUSD", "BTC/USD"),
        ("BTCUSDT", "BTC/USDT"),
        ("BTCUSDC", "BTC/USDC"),
        ("ETHBTC", "ETH/BTC"),
        ("USDTUSD", "USDT/USD"),
    ] {
        let pos = normalize_position(&position(wire, Some("crypto"), "0.5"));
        assert_eq!(pos.symbol, canonical, "wire symbol {wire}");
        assert_eq!(pos.qty, "0.5", "quantity is passed through verbatim");
    }
}

#[test]
fn w02_only_a_proven_crypto_asset_is_rewritten() {
    // Already canonical.
    assert_eq!(
        normalize_position(&position("BTC/USD", Some("crypto"), "1")).symbol,
        "BTC/USD"
    );
    // Equity rows never change, even when the ticker looks pair-shaped.
    assert_eq!(
        normalize_position(&position("AAPL", Some("us_equity"), "10")).symbol,
        "AAPL"
    );
    assert_eq!(
        normalize_position(&position("BTCUSD", Some("us_equity"), "10")).symbol,
        "BTCUSD"
    );
    // No asset_class evidence: never guess.
    assert_eq!(
        normalize_position(&position("BTCUSD", None, "0.5")).symbol,
        "BTCUSD"
    );
    // Crypto with no recognized quote asset, or nothing before the quote.
    assert_eq!(
        normalize_position(&position("FOOEUR", Some("crypto"), "1")).symbol,
        "FOOEUR"
    );
    assert_eq!(
        normalize_position(&position("USD", Some("crypto"), "1")).symbol,
        "USD"
    );
}

// ---------------------------------------------------------------------------
// REST activity lane fixtures
// ---------------------------------------------------------------------------

fn read_request_path(stream: &mut impl Read) -> String {
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).unwrap_or(0);
    let raw = String::from_utf8_lossy(&buf[..n]);
    raw.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string()
}

fn write_json_response(stream: &mut impl Write, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).ok();
}

fn partial_fill_activity(symbol: &str, qty: &str, cum_qty: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "act-w-1",
        "activity_type": "FILL",
        "type": "partial_fill",
        "order_id": "order-w",
        "transaction_time": "2024-01-15T10:30:00Z",
        "price": "60000.00",
        "qty": qty,
        "cum_qty": cum_qty,
        "side": "buy",
        "symbol": symbol,
    })
}

fn order_lookup(symbol: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "order-w",
        "client_order_id": "internal-order-w",
        "symbol": symbol,
        "side": "buy",
        "qty": "1",
        "filled_qty": "unused-not-an-economic-source",
    })
}

fn adapter_for(port: u16) -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
        crypto_capability_enabled: false,
        options_mleg_capability_enabled: false,
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

/// Serve one activities page followed (optionally) by one order lookup.
fn serve(activity: serde_json::Value, order: Option<serde_json::Value>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &serde_json::json!([activity]).to_string());
        }
        if let Some(order) = order {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order.to_string());
        }
    });
    port
}

// ---------------------------------------------------------------------------
// W03 — REST fractional crypto partial fill
// ---------------------------------------------------------------------------

#[test]
fn w03_rest_fractional_crypto_partial_fill_applies_with_exact_cum_qty() {
    let port = serve(
        partial_fill_activity("BTC/USD", "0.25", "0.25"),
        Some(order_lookup("BTC/USD")),
    );
    let token = BrokerInvokeToken::for_test();
    let (events, _) = adapter_for(port)
        .fetch_events(Some(&live_cursor()), &token)
        .expect("a fractional crypto partial fill must not fail the page");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].cum_qty_after(),
        Some("0.25".parse::<QtyMicros>().unwrap())
    );
}

// ---------------------------------------------------------------------------
// W04 / W05 — Equity whole-share guard is preserved
// ---------------------------------------------------------------------------

#[test]
fn w04_fractional_cum_qty_on_an_equity_order_still_fails_closed() {
    let port = serve(
        partial_fill_activity("AAPL", "0.5", "0.5"),
        Some(order_lookup("AAPL")),
    );
    let token = BrokerInvokeToken::for_test();
    match adapter_for(port).fetch_events(Some(&live_cursor()), &token) {
        Err(BrokerError::Transient { detail }) => {
            assert!(detail.contains("cum_qty"), "{detail}");
        }
        other => panic!("expected Transient naming cum_qty, got {other:?}"),
    }
}

#[test]
fn w05_malformed_cum_qty_fails_before_any_order_lookup() {
    // Only the activities page is served; an order lookup would hang/err.
    for bad in ["not-a-number", "", "-0.5", "0.1234567"] {
        let port = serve(partial_fill_activity("BTC/USD", "0.25", bad), None);
        let token = BrokerInvokeToken::for_test();
        match adapter_for(port).fetch_events(Some(&live_cursor()), &token) {
            Err(BrokerError::Transient { detail }) => {
                assert!(detail.contains("cum_qty"), "cum_qty={bad:?}: {detail}");
            }
            other => panic!("cum_qty={bad:?}: expected Transient, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// W06 — crypto time-in-force
// ---------------------------------------------------------------------------

fn crypto_request(time_in_force: &str) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-w06".to_string(),
        symbol: "BTC/USD".to_string(),
        side: Side::Buy,
        quantity: "0.5".parse().unwrap(),
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: time_in_force.to_string(),
        asset_class: AssetClass::Crypto,
    }
}

#[test]
fn w06_crypto_submit_with_unsupported_time_in_force_is_refused_before_http() {
    // Nothing listens on port 0: any attempted HTTP call would surface as a
    // Transport/AmbiguousSubmit error instead of the typed Reject below.
    let adapter = adapter_for(0);
    let token = BrokerInvokeToken::for_test();
    for tif in ["day", "fok", "opg", "cls", "", "GTD"] {
        match adapter.submit_order(crypto_request(tif), &token) {
            Err(BrokerError::Reject { code, .. }) => {
                assert_eq!(code, "crypto_time_in_force_unsupported", "tif={tif:?}");
            }
            other => panic!("tif={tif:?}: expected typed Reject, got {other:?}"),
        }
    }
}
