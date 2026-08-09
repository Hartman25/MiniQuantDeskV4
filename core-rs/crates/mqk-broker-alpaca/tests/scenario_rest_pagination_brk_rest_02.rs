//! BRK-REST-02 — Paginated Alpaca REST fill recovery.
//!
//! # Coverage
//!
//! P01  Empty first page: no events returned, cursor unchanged, no page_token on request.
//! P02  Partial page (< page_size): cursor advances to last activity ID; first request has no
//!      page_token; assertion proves `activity.id` stored (not `transaction_time`).
//! P03  Exactly page_size activities: second request uses page_token=<last_id_from_page1> and
//!      NOT after=<last_id_from_page1>; terminates on empty second page.
//! P04  Two pages: all events from both pages recovered; second request carries
//!      page_token=<last_id_from_page1> and NOT after=<last_id_from_page1>.
//! P05  No-progress guard: full page whose last id equals prev page_token causes
//!      BrokerError::Transient (fail closed rather than loop).
//! P06  PAPER-SOAK-PARTIAL-FILL-DEDUP-04: single PARTIAL_FILL activity in a page,
//!      cum_qty_after sourced directly from the activity's own broker-native `cum_qty`.
//! P07  PAPER-SOAK-PARTIAL-FILL-DEDUP-04: two PARTIAL_FILL activities for the same
//!      order in one page, each carrying its own distinct broker-native `cum_qty` —
//!      no page-relative computation involved.
//! P08  PAPER-SOAK-PARTIAL-FILL-DEDUP-04: PARTIAL_FILL sharing a page with the order's
//!      terminal FILL — the PARTIAL_FILL's cum_qty_after is its own activity's `cum_qty`,
//!      never contaminated by the FILL's quantity (defect A from -03 is structurally
//!      impossible now: there is no cross-activity arithmetic at all).
//! P09  PAPER-SOAK-PARTIAL-FILL-DEDUP-04: two partials plus a terminal fill on one page.
//! P10  PAPER-SOAK-PARTIAL-FILL-DEDUP-04 (MANDATORY, mission A04-7): a PARTIAL_FILL
//!      activity with a missing/unparseable broker-native `cum_qty` fails the WHOLE
//!      page closed (`BrokerError::Transient`) rather than falling back to an
//!      ambiguous `cum_qty_after=None` that plain event-id dedup cannot safely handle
//!      across lanes.
//! A04-1 (MANDATORY, mission A04-1 / NEG-1): pre-bracket contamination is structurally
//!      impossible now — a new distinct execution landing on the broker AFTER the
//!      activities page is fetched cannot affect an earlier activity's own `cum_qty`,
//!      because no `fetch_order`-derived value is used for `cum_qty_after` at all.
//!      Exercises the real `fetch_events` -> `OmsOrder::apply_with_watermark` production
//!      seam end to end.
//! A04-2 (MANDATORY, mission A04-2 / NEG-2): cross-page contamination is structurally
//!      impossible now — page 1's activities each carry their own `cum_qty`, never
//!      derived from a current-order total that already reflects page 2.
//!
//! All tests use a local std::net::TcpListener mock server — no live network, no DB.
//!
//! # Key contract assertions
//!
//! - `page_token=` is the pagination parameter for second and subsequent requests.
//! - `after=` is a date-time filter and must NOT carry an activity ID.
//! - Cursor (`rest_activity_after`) stores the last activity `id`, not `transaction_time`.
//! - `cum_qty_after` for REST-sourced `PartialFill` events is always the activity's own
//!   broker-native `cum_qty` — never reconstructed, never bracketed, never page-relative.

use mqk_broker_alpaca::{
    decode_fetch_cursor, encode_fetch_cursor,
    types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume},
    AlpacaBrokerAdapter, AlpacaConfig, FILL_ACTIVITIES_PAGE_SIZE,
};
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{BrokerAdapter, BrokerError, BrokerEvent, BrokerInvokeToken};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Mock HTTP server helpers
// ---------------------------------------------------------------------------

/// Read the HTTP request line (e.g. `GET /v2/account/... HTTP/1.1`) and return
/// the path+query component.  The query string is included so tests can assert
/// on parameter names.
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

/// Write a minimal HTTP/1.1 200 JSON response and close the connection.
fn write_json_response(stream: &mut impl Write, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).ok();
}

/// Build a FILL activity JSON object.  `id` and `transaction_time` carry
/// distinct values so tests can verify the cursor uses `id`, not `transaction_time`.
fn activity_json(id: &str, order_id: &str, ts: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activity_type": "FILL",
        "order_id": order_id,
        "transaction_time": ts,
        "price": "150.00",
        "qty": "1",
        "side": "buy",
        "symbol": "AAPL"
    })
}

/// Build a PARTIAL_FILL activity JSON object with an explicit `qty` and its
/// own broker-native `cum_qty` (PAPER-SOAK-PARTIAL-FILL-DEDUP-04: no longer
/// reconstructed — this is the exact value `fetch_events` must pass through).
fn partial_fill_activity_json(
    id: &str,
    order_id: &str,
    ts: &str,
    qty: &str,
    cum_qty: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activity_type": "PARTIAL_FILL",
        "order_id": order_id,
        "transaction_time": ts,
        "price": "150.00",
        "qty": qty,
        "cum_qty": cum_qty,
        "side": "buy",
        "symbol": "AAPL"
    })
}

/// Build a PARTIAL_FILL activity JSON object with NO `cum_qty` field at all —
/// simulates a malformed/incomplete broker response.
fn partial_fill_activity_json_no_cum_qty(
    id: &str,
    order_id: &str,
    ts: &str,
    qty: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activity_type": "PARTIAL_FILL",
        "order_id": order_id,
        "transaction_time": ts,
        "price": "150.00",
        "qty": qty,
        "side": "buy",
        "symbol": "AAPL"
    })
}

/// Build a FILL activity JSON object with an explicit `qty` and `price`.
fn fill_activity_json(
    id: &str,
    order_id: &str,
    ts: &str,
    qty: &str,
    price: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activity_type": "FILL",
        "order_id": order_id,
        "transaction_time": ts,
        "price": price,
        "qty": qty,
        "side": "buy",
        "symbol": "AAPL"
    })
}

/// Build an order JSON object for GET /v2/orders/{id}.
///
/// PAPER-SOAK-PARTIAL-FILL-DEDUP-04: `filled_qty` in this response is no
/// longer used for anything economic (it is deliberately a non-numeric
/// sentinel in most tests below to prove that) — the order lookup now exists
/// only to resolve `client_order_id`/`symbol`/`side`, all immutable fields
/// with no race exposure.
fn order_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "client_order_id": "internal-order-1",
        "symbol": "AAPL",
        "side": "buy",
        "qty": "100",
        "filled_qty": "unused-not-an-economic-source"
    })
}

/// Construct an adapter pointing to a local mock server on `port`.
fn adapter_for(port: u16) -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
    })
}

/// Live cursor with no prior rest_activity_after (first call in session).
fn live_cursor_no_prior() -> String {
    encode_fetch_cursor(&AlpacaFetchCursor::live(
        None,
        "prev-msg-id",
        "2024-01-15T10:00:00Z",
    ))
    .unwrap()
}

// ---------------------------------------------------------------------------
// P01 — Empty first page: no events, cursor unchanged, no page_token on request
// ---------------------------------------------------------------------------

#[test]
fn p01_empty_page_returns_no_events_and_no_cursor() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let path = read_request_path(&mut stream);
        cap.lock().unwrap().push(path);
        write_json_response(&mut stream, "[]");
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P01: empty page must not error");

    assert!(events.is_empty(), "P01: empty page must return no events");
    assert!(
        new_cursor.is_none(),
        "P01: empty page must not advance cursor"
    );

    let paths = captured.lock().unwrap();
    assert_eq!(paths.len(), 1, "P01: exactly one HTTP request expected");
    // First request (no prior cursor): must not carry page_token.
    assert!(
        !paths[0].contains("page_token="),
        "P01: first request with no prior cursor must not have page_token; got: {}",
        paths[0]
    );
}

// ---------------------------------------------------------------------------
// P02 — Partial page: cursor uses activity `id` not `transaction_time`;
//       first request has no page_token
// ---------------------------------------------------------------------------

#[test]
fn p02_partial_page_cursor_uses_last_activity_id_not_timestamp() {
    // 1 activities request (2 items, < page_size) + 2 per-activity order
    // lookups (client_order_id only — PAPER-SOAK-PARTIAL-FILL-DEDUP-04
    // removed the race-detection bracket entirely) = 3 requests.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();

    std::thread::spawn(move || {
        // Request 1: GET activities → 2 items (partial page).
        {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);
            cap.lock().unwrap().push(path);
            // activity id and transaction_time differ intentionally.
            let acts = serde_json::json!([
                activity_json("act-id-0001", "order-aaa", "2024-01-15T10:30:00Z"),
                activity_json("act-id-0002", "order-aaa", "2024-01-15T10:31:00Z"),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // Requests 2-3: GET /v2/orders/order-aaa — 1 per activity.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let _path = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-aaa").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P02: partial page must succeed");

    assert_eq!(events.len(), 2, "P02: must recover both activities");

    let decoded = decode_fetch_cursor(new_cursor.as_deref()).expect("P02: cursor must decode");
    // Cursor must be the last activity `id` ("act-id-0002"), NOT the
    // `transaction_time` ("2024-01-15T10:31:00Z").
    assert_eq!(
        decoded.rest_activity_after.as_deref(),
        Some("act-id-0002"),
        "P02: cursor must use last activity id, not transaction_time"
    );
    assert!(
        matches!(decoded.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "P02: WS continuity must remain Live after REST recovery"
    );

    let paths = captured.lock().unwrap();
    // Single activities request, no prior cursor → no page_token.
    assert!(
        !paths[0].contains("page_token="),
        "P02: first request with no prior cursor must not have page_token; got: {}",
        paths[0]
    );
}

// ---------------------------------------------------------------------------
// P03 — Exactly page_size activities: second request uses page_token=<last_id>,
//       NOT after=<last_id>; terminates on empty second page
// ---------------------------------------------------------------------------

#[test]
fn p03_full_page_second_request_uses_page_token_not_after() {
    // Requests: 1 activities (50 items) + 50 per-activity order lookups
    // + 1 activities (empty) = 52.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let activities_call: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let act_call = activities_call.clone();

    let total = FILL_ACTIVITIES_PAGE_SIZE + 2;
    std::thread::spawn(move || {
        for _ in 0..total {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);

            if path.contains("activities") {
                cap.lock().unwrap().push(path.clone());
                let mut n = act_call.lock().unwrap();
                let page = *n;
                *n += 1;
                drop(n);

                if page == 0 {
                    let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                        .map(|i| {
                            activity_json(
                                &format!("act-p1-{i:04}"),
                                "order-bbb",
                                "2024-01-15T10:30:00Z",
                            )
                        })
                        .collect();
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                } else {
                    write_json_response(&mut stream, "[]");
                }
            } else {
                write_json_response(&mut stream, &order_json("order-bbb").to_string());
            }
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P03: full page + empty second page must succeed");

    assert_eq!(
        events.len(),
        FILL_ACTIVITIES_PAGE_SIZE,
        "P03: exactly page_size events must be returned"
    );

    let decoded = decode_fetch_cursor(new_cursor.as_deref()).expect("P03: cursor must decode");
    let last_id = format!("act-p1-{:04}", FILL_ACTIVITIES_PAGE_SIZE - 1);
    assert_eq!(
        decoded.rest_activity_after.as_deref(),
        Some(last_id.as_str()),
        "P03: cursor must be last activity id from first page"
    );

    let paths = captured.lock().unwrap();
    assert_eq!(
        paths.len(),
        2,
        "P03: must have made exactly 2 activities requests"
    );

    // First request: no prior cursor → no page_token.
    assert!(
        !paths[0].contains("page_token="),
        "P03: first request must not have page_token; got: {}",
        paths[0]
    );

    // Second request: must carry page_token=<last_id_from_page1>.
    assert!(
        paths[1].contains(&format!("page_token={last_id}")),
        "P03: second request must use page_token={}; got: {}",
        last_id,
        paths[1]
    );

    // Regression: second request must NOT use after=<activity_id>.
    assert!(
        !paths[1].contains(&format!("after={last_id}")),
        "P03: second request must NOT use after=<activity_id> (after is a date filter); got: {}",
        paths[1]
    );
}

// ---------------------------------------------------------------------------
// P04 — Two pages: all events from both pages recovered;
//       second request uses page_token, NOT after
// ---------------------------------------------------------------------------

#[test]
fn p04_two_page_recovery_all_events_second_request_uses_page_token_not_after() {
    // Requests: 2 activities + (FILL_ACTIVITIES_PAGE_SIZE + 1) per-activity
    // order lookups (no bracket — PAPER-SOAK-PARTIAL-FILL-DEDUP-04).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let activities_call: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let act_call = activities_call.clone();

    let total = 2 + FILL_ACTIVITIES_PAGE_SIZE + 1;
    std::thread::spawn(move || {
        for _ in 0..total {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);

            if path.contains("activities") {
                cap.lock().unwrap().push(path.clone());
                let mut n = act_call.lock().unwrap();
                let page = *n;
                *n += 1;
                drop(n);

                if page == 0 {
                    // Page 1: exactly FILL_ACTIVITIES_PAGE_SIZE items → triggers page 2.
                    let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                        .map(|i| {
                            activity_json(
                                &format!("act-p1-{i:04}"),
                                "order-ccc",
                                "2024-01-15T10:30:00Z",
                            )
                        })
                        .collect();
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                } else {
                    // Page 2: 1 item (partial page → terminates loop).
                    let acts = serde_json::json!([activity_json(
                        "act-p2-0001",
                        "order-ccc",
                        "2024-01-15T10:31:00Z"
                    )]);
                    write_json_response(&mut stream, &acts.to_string());
                }
            } else {
                write_json_response(&mut stream, &order_json("order-ccc").to_string());
            }
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P04: two-page recovery must succeed");

    assert_eq!(
        events.len(),
        FILL_ACTIVITIES_PAGE_SIZE + 1,
        "P04: all events from both pages must be returned"
    );

    let decoded = decode_fetch_cursor(new_cursor.as_deref()).expect("P04: cursor must decode");
    assert_eq!(
        decoded.rest_activity_after.as_deref(),
        Some("act-p2-0001"),
        "P04: cursor must point to the last activity ID on the final page"
    );
    assert!(
        matches!(decoded.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "P04: WS continuity must remain Live after multi-page REST recovery"
    );

    let paths = captured.lock().unwrap();
    assert_eq!(
        paths.len(),
        2,
        "P04: must have made exactly 2 activities requests"
    );

    // First request: no prior cursor → no page_token.
    assert!(
        !paths[0].contains("page_token="),
        "P04: first request must not have page_token; got: {}",
        paths[0]
    );

    // Second request must carry page_token=<last_id_from_page1>.
    let p1_last = format!("act-p1-{:04}", FILL_ACTIVITIES_PAGE_SIZE - 1);
    assert!(
        paths[1].contains(&format!("page_token={p1_last}")),
        "P04: second request must use page_token={}; got: {}",
        p1_last,
        paths[1]
    );

    // Regression: second request must NOT route the activity ID through `after`.
    assert!(
        !paths[1].contains(&format!("after={p1_last}")),
        "P04: second request must NOT use after=<activity_id>; got: {}",
        paths[1]
    );
}

// ---------------------------------------------------------------------------
// P05 — No-progress guard: stale full page fails closed
// ---------------------------------------------------------------------------

#[test]
fn p05_no_progress_full_page_fails_closed_with_transient() {
    // Stale scenario: page 2 returns FILL_ACTIVITIES_PAGE_SIZE items whose last
    // id is the same as the page_token we sent → no progress → Transient error.
    //
    // Server handles: 1 activities (page 1) + PS per-activity order lookups
    // (no bracket) + 1 activities (stale page 2). The guard fires after
    // receiving stale page 2 but BEFORE any order lookups for it, so the
    // server loop ends at exactly 1 + PS + 1 = PS + 2 requests.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let activities_call: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let act_call = activities_call.clone();

    // Stale last ID — the same value on both pages' last activity.
    const STALE_LAST_ID: &str = "act-stale-last";

    let total = FILL_ACTIVITIES_PAGE_SIZE + 2;
    std::thread::spawn(move || {
        for _ in 0..total {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);

            if path.contains("activities") {
                let mut n = act_call.lock().unwrap();
                let page = *n;
                *n += 1;
                drop(n);

                if page == 0 {
                    // Page 1: FILL_ACTIVITIES_PAGE_SIZE unique activities; last id = STALE_LAST_ID.
                    let mut acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE - 1)
                        .map(|i| {
                            activity_json(
                                &format!("act-p1-{i:04}"),
                                "order-zzz",
                                "2024-01-15T10:30:00Z",
                            )
                        })
                        .collect();
                    acts.push(activity_json(
                        STALE_LAST_ID,
                        "order-zzz",
                        "2024-01-15T10:30:59Z",
                    ));
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                } else {
                    // Stale page 2: returns full page; last id is still STALE_LAST_ID
                    // → no progress guard fires.
                    let mut acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE - 1)
                        .map(|i| {
                            activity_json(
                                &format!("act-stale-{i:04}"),
                                "order-zzz",
                                "2024-01-15T10:31:00Z",
                            )
                        })
                        .collect();
                    acts.push(activity_json(
                        STALE_LAST_ID,
                        "order-zzz",
                        "2024-01-15T10:31:59Z",
                    ));
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                }
            } else {
                // Order lookup (only for page 1 activities; guard fires before page 2 orders).
                write_json_response(&mut stream, &order_json("order-zzz").to_string());
            }
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let result = adapter.fetch_events(Some(&cursor), &token);

    assert!(result.is_err(), "P05: stale full page must return Err");
    match result.unwrap_err() {
        BrokerError::Transient { detail } => {
            assert!(
                detail.contains("no progress"),
                "P05: error detail must mention no progress; got: {detail}"
            );
        }
        other => panic!("P05: expected Transient error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// P06/P07 — PAPER-SOAK-PARTIAL-FILL-DEDUP-04: direct broker-native cum_qty
// ---------------------------------------------------------------------------

#[test]
fn p06_single_partial_fill_cum_qty_after_is_activitys_own_cum_qty() {
    // The activity's own `cum_qty=10` is passed straight through — no
    // fetch_order, no bracket, no arithmetic of any kind.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts = serde_json::json!([partial_fill_activity_json(
                "act-single-1",
                "order-single",
                "2024-01-15T10:30:00Z",
                "10",
                "10"
            )]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // 1 per-activity order lookup (client_order_id only).
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-single").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P06: single partial fill must succeed");

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].cum_qty_after(),
        Some(10),
        "P06: cum_qty_after must be the activity's own broker-native cum_qty"
    );
}

#[test]
fn p07_two_partial_fills_same_order_same_page_each_keep_own_cum_qty() {
    // TWO PARTIAL_FILL activities for the SAME order in ONE page, each
    // carrying its own distinct broker-native cum_qty (10, then 25). Unlike
    // -02/-03, there is no page-relative computation that could confuse them
    // — each value is read directly off its own activity record.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts = serde_json::json!([
                partial_fill_activity_json(
                    "act-multi-1",
                    "order-multi",
                    "2024-01-15T10:30:00Z",
                    "10",
                    "10"
                ),
                partial_fill_activity_json(
                    "act-multi-2",
                    "order-multi",
                    "2024-01-15T10:30:05Z",
                    "15",
                    "25"
                ),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // 1 GET /v2/orders/{id} call per activity.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-multi").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P07: two partial fills in one page must succeed");

    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].cum_qty_after(),
        Some(10),
        "P07: first activity keeps its own cum_qty (10)"
    );
    assert_eq!(
        events[1].cum_qty_after(),
        Some(25),
        "P07: second activity keeps its own cum_qty (25)"
    );
    let deltas: Vec<i64> = events
        .iter()
        .map(|e| match e {
            mqk_execution::BrokerEvent::PartialFill { delta_qty, .. } => *delta_qty,
            other => panic!("expected PartialFill, got {other:?}"),
        })
        .collect();
    assert_eq!(deltas, vec![10, 15]);
}

// ---------------------------------------------------------------------------
// P08–P10 — PAPER-SOAK-PARTIAL-FILL-DEDUP-04: mixed-type pages and the
// missing-cum_qty ambiguity policy.
// ---------------------------------------------------------------------------

/// P08 (mirrors mission A03-1/A04-3): a PARTIAL_FILL sharing a REST page with
/// the same order's terminal FILL. There is no cross-activity arithmetic at
/// all now, so the FILL's quantity structurally cannot be folded into the
/// PARTIAL_FILL's cum_qty_after — the defect -03 fixed with a summed-baseline
/// heuristic is now impossible by construction, not merely tested against.
#[test]
fn p08_mixed_partial_fill_and_terminal_fill_same_page_each_activity_stays_independent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts = serde_json::json!([
                partial_fill_activity_json(
                    "act-mixed-1",
                    "order-mixed",
                    "2024-01-15T10:30:00Z",
                    "10",
                    "10"
                ),
                fill_activity_json(
                    "act-mixed-2",
                    "order-mixed",
                    "2024-01-15T10:30:05Z",
                    "10",
                    "102.00"
                ),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // 1 per-activity order lookup each.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-mixed").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P08: mixed partial+terminal fill page must succeed");

    assert_eq!(events.len(), 2);
    match &events[0] {
        mqk_execution::BrokerEvent::PartialFill {
            delta_qty,
            price_micros,
            cum_qty_after,
            ..
        } => {
            assert_eq!(*delta_qty, 10);
            assert_eq!(*price_micros, 150_000_000);
            assert_eq!(
                *cum_qty_after,
                Some(10),
                "P08: PARTIAL_FILL's cum_qty_after is its own activity's cum_qty (10), \
                 structurally independent of the adjacent terminal FILL's quantity"
            );
        }
        other => panic!("expected PartialFill, got {other:?}"),
    }
    match &events[1] {
        mqk_execution::BrokerEvent::Fill {
            delta_qty,
            price_micros,
            ..
        } => {
            assert_eq!(*delta_qty, 10);
            assert_eq!(*price_micros, 102_000_000);
        }
        other => panic!("expected Fill, got {other:?}"),
    }
}

/// P09 (mirrors mission A03-2): two partials plus a terminal fill, all on
/// one page. Each activity's own cum_qty is used unmodified.
#[test]
fn p09_two_partials_plus_terminal_fill_same_page_all_independent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts = serde_json::json!([
                partial_fill_activity_json(
                    "act-p09-1",
                    "order-p09",
                    "2024-01-15T10:30:00Z",
                    "5",
                    "5"
                ),
                partial_fill_activity_json(
                    "act-p09-2",
                    "order-p09",
                    "2024-01-15T10:30:05Z",
                    "5",
                    "10"
                ),
                fill_activity_json(
                    "act-p09-3",
                    "order-p09",
                    "2024-01-15T10:30:10Z",
                    "10",
                    "102.00"
                ),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // 1 per-activity read x3 activities.
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-p09").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("P09: two partials plus terminal fill must succeed");

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].cum_qty_after(), Some(5));
    assert_eq!(events[1].cum_qty_after(), Some(10));
    assert_eq!(
        events[2].cum_qty_after(),
        None,
        "P09: Fill events never carry cum_qty_after"
    );
    match &events[2] {
        mqk_execution::BrokerEvent::Fill { delta_qty, .. } => assert_eq!(*delta_qty, 10),
        other => panic!("expected Fill, got {other:?}"),
    }
}

/// P10 (MANDATORY, mirrors mission A04-7): a PARTIAL_FILL activity with no
/// parseable broker-native `cum_qty` must fail the WHOLE page closed
/// (`BrokerError::Transient`), never fall back to an ambiguous
/// `cum_qty_after=None` that the OMS's plain event-id dedup could
/// misclassify as a brand-new fill for a cross-lane duplicate. The broker
/// side of this record is durably retained regardless (Alpaca's activity
/// history is not affected by this adapter failing to parse a response);
/// since the cursor does not advance, the next poll retries the same page.
#[test]
fn p10_missing_cum_qty_on_partial_fill_fails_whole_page_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request_path(&mut stream);
        let acts = serde_json::json!([partial_fill_activity_json_no_cum_qty(
            "act-ambig-1",
            "order-ambig",
            "2024-01-15T10:30:00Z",
            "10"
        )]);
        write_json_response(&mut stream, &acts.to_string());
        // No further requests: the adapter must fail before any per-activity
        // order lookup for this activity.
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let result = adapter.fetch_events(Some(&cursor), &token);

    assert!(
        result.is_err(),
        "P10: missing cum_qty on a PARTIAL_FILL must fail the page, not degrade to ambiguous None"
    );
    match result.unwrap_err() {
        BrokerError::Transient { detail } => {
            assert!(
                detail.contains("cum_qty"),
                "P10: error detail must name the missing cum_qty; got: {detail}"
            );
        }
        other => panic!("P10: expected Transient error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// A04-1 / A04-2 (MANDATORY) — end-to-end negative controls from the mission
// brief, exercising the real fetch_events -> OmsOrder::apply_with_watermark
// production seam together. Both scenarios were confirmed, before this
// patch, to double-apply the same physical fill under the pre-patch
// bracket/reconstruction design (captured as this patch's NEG-1/NEG-2
// evidence). They are retained here permanently as regression coverage.
// ---------------------------------------------------------------------------

/// A04-1: a new distinct execution (5@$101) lands on the broker AFTER the
/// activities page is fetched but BEFORE `fetch_events`'s per-activity order
/// lookup. Under the pre-patch design this contaminated a same-valued
/// bracket read undetectably; under this design the per-activity order
/// lookup is never used for `cum_qty_after` at all, so the later execution
/// has no path to affect this activity's `cum_qty_after`.
#[test]
fn a04_1_pre_bracket_race_no_longer_double_applies_same_physical_fill() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        // Activities page reflects broker state BEFORE the new 5@$101
        // execution landed -- only the original 10@$100 partial fill, with
        // its own broker-native cum_qty=10 fixed at execution time.
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts = serde_json::json!([partial_fill_activity_json(
                "act-a04-1",
                "order-a04-1",
                "2024-01-15T10:30:00Z",
                "10",
                "10"
            )]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // Per-activity order lookup -- happens AFTER the new 5@$101
        // execution has conceptually landed on the broker, exactly as in the
        // mission's race, but its filled_qty is not read for anything
        // economic anymore.
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(&mut stream, &order_json("order-a04-1").to_string());
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("A04-1: fetch_events must succeed");

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cum_qty_after(), Some(10));

    // Simulate the WS lane having already applied this exact physical fill
    // under a different transport event_id.
    let mut order = OmsOrder::new("order-a04-1", "AAPL", 20);
    order
        .apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 10 },
            Some("ws-msg-a04-1"),
            Some(10),
        )
        .unwrap();
    assert_eq!(order.filled_qty, 10);

    let (delta_qty, rest_cum_qty_after) = match &events[0] {
        BrokerEvent::PartialFill {
            delta_qty,
            cum_qty_after,
            ..
        } => (*delta_qty, *cum_qty_after),
        other => panic!("expected PartialFill, got {other:?}"),
    };
    order
        .apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty },
            Some("rest-act-a04-1"),
            rest_cum_qty_after,
        )
        .unwrap();

    assert_eq!(
        order.filled_qty, 10,
        "A04-1: the REST redelivery of the same physical fill must be recognized \
         as a duplicate; final economics must show exactly one 10-share application, \
         never 20"
    );
}

/// A04-2: page 1 has FILL_ACTIVITIES_PAGE_SIZE one-share PARTIAL_FILL
/// activities, page 2 has one more -- the order's CURRENT total already
/// includes page 2's quantity by the time page 1 is processed, exactly as in
/// the mission's deterministic (no-race) cross-page scenario. Each page-1
/// activity's cum_qty_after comes from its own activity record, so it cannot
/// be shifted by page 2's quantity at all.
#[test]
fn a04_2_cross_page_contamination_no_longer_shifts_page1_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let activities_call = Arc::new(Mutex::new(0usize));
    let act_call = activities_call.clone();

    let total_requests = 2 /* activities pages */
        + FILL_ACTIVITIES_PAGE_SIZE /* page-1 per-activity order lookups */
        + 1 /* page-2 per-activity order lookup */;

    std::thread::spawn(move || {
        for _ in 0..total_requests {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);

            if path.contains("activities") {
                let mut n = act_call.lock().unwrap();
                let page = *n;
                *n += 1;
                drop(n);

                if page == 0 {
                    let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                        .map(|i| {
                            partial_fill_activity_json(
                                &format!("act-a04-2-p1-{i:04}"),
                                "order-a04-2",
                                "2024-01-15T10:30:00Z",
                                "1",
                                &(i + 1).to_string(),
                            )
                        })
                        .collect();
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                } else {
                    let acts = serde_json::json!([partial_fill_activity_json(
                        "act-a04-2-p2-0000",
                        "order-a04-2",
                        "2024-01-15T10:31:00Z",
                        "1",
                        &(FILL_ACTIVITIES_PAGE_SIZE + 1).to_string()
                    )]);
                    write_json_response(&mut stream, &acts.to_string());
                }
            } else {
                write_json_response(&mut stream, &order_json("order-a04-2").to_string());
            }
        }
    });

    let adapter = adapter_for(port);
    let cursor = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();

    let (events, _new_cursor) = adapter
        .fetch_events(Some(&cursor), &token)
        .expect("A04-2: fetch_events must succeed");

    assert_eq!(events.len(), FILL_ACTIVITIES_PAGE_SIZE + 1);
    assert_eq!(
        events[0].cum_qty_after(),
        Some(1),
        "A04-2: page-1's first activity's cum_qty_after must be its own value (1), \
         never shifted by page 2's quantity"
    );

    // Simulate the WS lane having already applied the order's first physical
    // fill (true cumulative = 1) under a different transport event_id.
    let mut order = OmsOrder::new(
        "order-a04-2",
        "AAPL",
        (FILL_ACTIVITIES_PAGE_SIZE + 1) as i64,
    );
    order
        .apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty: 1 },
            Some("ws-msg-a04-2"),
            Some(1),
        )
        .unwrap();
    assert_eq!(order.filled_qty, 1);

    let (delta_qty, rest_cum_qty_after) = match &events[0] {
        BrokerEvent::PartialFill {
            delta_qty,
            cum_qty_after,
            ..
        } => (*delta_qty, *cum_qty_after),
        other => panic!("expected PartialFill, got {other:?}"),
    };
    order
        .apply_with_watermark(
            &OmsEvent::PartialFill { delta_qty },
            Some("rest-act-a04-2-p1-0000"),
            rest_cum_qty_after,
        )
        .unwrap();

    assert_eq!(
        order.filled_qty, 1,
        "A04-2: the REST redelivery of the order's first physical fill must be \
         recognized as a duplicate; final economics must show exactly one \
         1-share application, never 2"
    );
}

// ---------------------------------------------------------------------------
// A04-8 — pagination restart: a fresh `fetch_events` call, given the cursor
// persisted from a prior successful multi-page recovery, resumes from
// exactly that point with no reprocessing.
//
// `fetch_events` itself recovers all available pages within a single call
// (it loops until a partial page terminates it) — there is no partial,
// mid-batch checkpoint inside one call to interrupt. The restart-safety
// contract that matters is therefore: given the cursor persisted after a
// call completes, does the NEXT call (a fresh adapter, standing in for a
// fresh process) resume from precisely that point rather than reprocessing
// anything already recovered? Cursor/pagination mechanics are unchanged by
// PAPER-SOAK-PARTIAL-FILL-DEDUP-04 (only `cum_qty_after` sourcing changed);
// this proves the contract still holds with the new per-activity `cum_qty`
// design in place. (Crash-safety for a genuinely interrupted single HTTP
// call is a DB/orchestrator-layer property — the inbox's unique-constraint
// dedup on `broker_message_id` — unchanged and out of this patch's scope.)
// ---------------------------------------------------------------------------

#[test]
fn a04_8_restart_resumes_from_persisted_cursor_no_reprocessing() {
    // Session 1: full two-page recovery (page 1 = FILL_ACTIVITIES_PAGE_SIZE
    // items, page 2 = 1 item), exactly like P04, ending with the persisted
    // cursor pointing at the last activity actually recovered.
    let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let port1 = listener1.local_addr().unwrap().port();
    let activities_call1 = Arc::new(Mutex::new(0usize));
    let act_call1 = activities_call1.clone();
    let total1 = 2 + FILL_ACTIVITIES_PAGE_SIZE + 1;
    std::thread::spawn(move || {
        for _ in 0..total1 {
            let (mut stream, _) = listener1.accept().unwrap();
            let path = read_request_path(&mut stream);
            if path.contains("activities") {
                let mut n = act_call1.lock().unwrap();
                let page = *n;
                *n += 1;
                drop(n);
                if page == 0 {
                    let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                        .map(|j| {
                            partial_fill_activity_json(
                                &format!("act-a04-8-p1-{j:04}"),
                                "order-a04-8",
                                "2024-01-15T10:30:00Z",
                                "1",
                                &(j + 1).to_string(),
                            )
                        })
                        .collect();
                    write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
                } else {
                    let acts = serde_json::json!([partial_fill_activity_json(
                        "act-a04-8-p2-0000",
                        "order-a04-8",
                        "2024-01-15T10:31:00Z",
                        "1",
                        &(FILL_ACTIVITIES_PAGE_SIZE + 1).to_string()
                    )]);
                    write_json_response(&mut stream, &acts.to_string());
                }
            } else {
                write_json_response(&mut stream, &order_json("order-a04-8").to_string());
            }
        }
    });
    let adapter1 = adapter_for(port1);
    let cursor0 = live_cursor_no_prior();
    let token = BrokerInvokeToken::for_test();
    let (events1, cursor1) = adapter1
        .fetch_events(Some(&cursor0), &token)
        .expect("A04-8: session 1's full recovery must succeed");
    assert_eq!(events1.len(), FILL_ACTIVITIES_PAGE_SIZE + 1);
    let persisted_cursor = cursor1.expect("A04-8: cursor must advance after recovery");

    // "Restart": a brand-new adapter/mock-server pair (standing in for a
    // fresh process), resuming from the persisted cursor. Nothing new has
    // happened on the broker since — the activities response is empty —
    // proving no already-recovered activity is refetched or reapplied.
    let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let port2 = listener2.local_addr().unwrap().port();
    let seen_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = seen_paths.clone();
    std::thread::spawn(move || {
        let (mut stream, _) = listener2.accept().unwrap();
        let path = read_request_path(&mut stream);
        seen.lock().unwrap().push(path);
        write_json_response(&mut stream, "[]");
    });
    let adapter2 = adapter_for(port2);
    let (events2, cursor2) = adapter2
        .fetch_events(Some(&persisted_cursor), &token)
        .expect("A04-8: restart must resume from the persisted cursor");

    assert!(
        events2.is_empty(),
        "A04-8: restart must not reprocess any already-recovered activity"
    );
    assert!(
        cursor2.is_none(),
        "A04-8: an empty page must not advance the cursor further"
    );

    let paths = seen_paths.lock().unwrap();
    assert!(
        paths[0].contains("page_token=act-a04-8-p2-0000"),
        "A04-8: restart request must carry the persisted page_token (the last \
         activity actually recovered in session 1, from page 2 — not page 1); got: {}",
        paths[0]
    );
}
