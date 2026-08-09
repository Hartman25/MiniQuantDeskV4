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
//!
//! All tests use a local std::net::TcpListener mock server — no live network, no DB.
//!
//! # Key contract assertions
//!
//! - `page_token=` is the pagination parameter for second and subsequent requests.
//! - `after=` is a date-time filter and must NOT carry an activity ID.
//! - Cursor (`rest_activity_after`) stores the last activity `id`, not `transaction_time`.

use mqk_broker_alpaca::{
    decode_fetch_cursor, encode_fetch_cursor,
    types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume},
    AlpacaBrokerAdapter, AlpacaConfig, FILL_ACTIVITIES_PAGE_SIZE,
};
use mqk_execution::{BrokerAdapter, BrokerError, BrokerInvokeToken};
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

/// Build a PARTIAL_FILL activity JSON object with an explicit `qty`.
fn partial_fill_activity_json(id: &str, order_id: &str, ts: &str, qty: &str) -> serde_json::Value {
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

/// Build an order JSON object for GET /v2/orders/{id}.
fn order_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "client_order_id": "internal-order-1",
        "symbol": "AAPL",
        "side": "buy",
        "qty": "100",
        "filled_qty": "1"
    })
}

/// Build an order JSON object with an explicit current `filled_qty`.
fn order_json_with_filled(id: &str, filled_qty: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "client_order_id": "internal-order-1",
        "symbol": "AAPL",
        "side": "buy",
        "qty": "100",
        "filled_qty": filled_qty
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
    // 1 activities request (2 items, < page_size) + 2 order lookups = 3 requests.
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
        // Requests 2–3: GET /v2/orders/order-aaa (one per activity).
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
    // Requests: 1 activities (50 items) + 50 order lookups + 1 activities (empty) = 52.
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
    // Requests: 2 activities + (FILL_ACTIVITIES_PAGE_SIZE + 1) order lookups.
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
    // Server handles: 1 activities (page 1) + PS order lookups + 1 activities (stale page 2).
    // The guard fires after receiving stale page 2 but BEFORE processing its order lookups,
    // so the server loop ends at exactly 1 + PS + 1 = PS + 2 requests.
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
// P06/P07 — PAPER-SOAK-PARTIAL-FILL-DEDUP-02: cum_qty_after reconstruction
// ---------------------------------------------------------------------------

#[test]
fn p06_single_partial_fill_in_page_cum_qty_after_matches_current_filled_qty() {
    // Unambiguous case: exactly one PARTIAL_FILL activity for this order in
    // the page, so "current" filled_qty from GET /v2/orders IS "as of this
    // event" -- no reconstruction needed, value passes through directly.
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
                "10"
            )]);
            write_json_response(&mut stream, &acts.to_string());
        }
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(
                &mut stream,
                &order_json_with_filled("order-single", "10").to_string(),
            );
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
        "P06: unambiguous single-activity page must pass current filled_qty through as cum_qty_after"
    );
}

#[test]
fn p07_two_partial_fills_same_order_same_page_reconstruct_distinct_cumulative() {
    // Ambiguous-by-default case: TWO PARTIAL_FILL activities for the SAME
    // order in ONE page. A naive "reuse current filled_qty for every
    // activity" implementation would wrongly attach the SAME (final) total
    // to both. The reconstruction must instead recover each activity's own
    // true point-in-time cumulative: first=10, second=25 (10+15), not both=25.
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
                    "10"
                ),
                partial_fill_activity_json(
                    "act-multi-2",
                    "order-multi",
                    "2024-01-15T10:30:05Z",
                    "15"
                ),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
        // One GET /v2/orders/{id} call per activity; current total is 25
        // (10 + 15) both times, since nothing else has landed since.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            write_json_response(
                &mut stream,
                &order_json_with_filled("order-multi", "25").to_string(),
            );
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
        "P07: first activity's reconstructed cumulative must be 10, not the page-final 25"
    );
    assert_eq!(
        events[1].cum_qty_after(),
        Some(25),
        "P07: second activity's reconstructed cumulative must be 25 (10+15)"
    );
    // Both delta_qty values must still be correct and distinct.
    let deltas: Vec<i64> = events
        .iter()
        .map(|e| match e {
            mqk_execution::BrokerEvent::PartialFill { delta_qty, .. } => *delta_qty,
            other => panic!("expected PartialFill, got {other:?}"),
        })
        .collect();
    assert_eq!(deltas, vec![10, 15]);
}
