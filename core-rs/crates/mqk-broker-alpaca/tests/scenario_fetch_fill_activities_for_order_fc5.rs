//! PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (§7) — HTTP-mock proof
//! for `AlpacaBrokerAdapter::fetch_fill_activities_for_order`.
//!
//! This function had no direct test coverage before this patch (only
//! exercised indirectly through the halted-run REST fill repair route). It
//! backs a repair path that may mutate durable accounting truth, so its
//! pagination, local filtering, and fail-closed page-budget behavior are
//! proven directly here against a local HTTP mock server — no real Alpaca
//! calls, no DB.
//!
//! # Coverage
//!
//! FC-5A  Target activity on page 2 (page 1 is full of unrelated orders):
//!        the function returns exactly the target, the first request has no
//!        `page_token`, the second request carries
//!        `page_token=<last_id_from_page1>`, and NO request ever carries an
//!        `order_id=` query parameter.
//! FC-5B  Every activity across multiple pages belongs to unrelated orders:
//!        returns empty only once the actual end of the feed (a partial
//!        page) is reached — not merely once a page contains no match.
//! FC-5C  Every page up to the configured maximum is full and unrelated to
//!        the target (the end of the feed is never reached): the function
//!        must return `Err`, never a partial/possibly-incomplete match set.

use mqk_broker_alpaca::{
    AlpacaBrokerAdapter, AlpacaConfig, FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES,
    FILL_ACTIVITIES_PAGE_SIZE,
};
use mqk_execution::BrokerError;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Mock HTTP server helpers (mirrors scenario_rest_pagination_brk_rest_02.rs)
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

fn activity_json(id: &str, order_id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activity_type": "FILL",
        "type": "fill",
        "order_id": order_id,
        "transaction_time": "2024-01-15T10:30:00Z",
        "price": "150.00",
        "qty": "1",
        "side": "buy",
        "symbol": "AAPL"
    })
}

fn adapter_for(port: u16) -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
    })
}

// ---------------------------------------------------------------------------
// FC-5A: target on page 2
// ---------------------------------------------------------------------------

#[test]
fn fc5a_target_on_page_two_found_no_order_id_param_correct_page_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();

    std::thread::spawn(move || {
        // Page 1: FILL_ACTIVITIES_PAGE_SIZE unrelated-order activities (full
        // page -> triggers a second request).
        {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);
            cap.lock().unwrap().push(path);
            let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                .map(|i| activity_json(&format!("act-p1-{i:04}"), "order-unrelated"))
                .collect();
            write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
        }
        // Page 2: the target activity, alone (partial page -> terminates).
        {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);
            cap.lock().unwrap().push(path);
            let acts = serde_json::json!([activity_json("act-p2-target", "order-target")]);
            write_json_response(&mut stream, &acts.to_string());
        }
    });

    let adapter = adapter_for(port);
    let result = adapter
        .fetch_fill_activities_for_order("order-target")
        .expect("FC-5A: must succeed");

    assert_eq!(
        result.len(),
        1,
        "FC-5A: exactly the target activity must be returned"
    );
    assert_eq!(result[0].id, "act-p2-target");
    assert_eq!(result[0].order_id, "order-target");

    let paths = captured.lock().unwrap();
    assert_eq!(
        paths.len(),
        2,
        "FC-5A: exactly two activities requests expected"
    );

    // First request: no prior page_token.
    assert!(
        !paths[0].contains("page_token="),
        "FC-5A: first request must not carry page_token; got: {}",
        paths[0]
    );
    // Second request: page_token = last activity id from page 1.
    let p1_last = format!("act-p1-{:04}", FILL_ACTIVITIES_PAGE_SIZE - 1);
    assert!(
        paths[1].contains(&format!("page_token={p1_last}")),
        "FC-5A: second request must carry page_token={p1_last}; got: {}",
        paths[1]
    );
    // Neither request may ever carry an order_id query parameter -- Alpaca's
    // documented Account Activities endpoint does not support filtering by
    // order_id; local exact filtering is the only safety boundary.
    for (i, p) in paths.iter().enumerate() {
        assert!(
            !p.contains("order_id="),
            "FC-5A: request {i} must NOT carry an order_id query parameter; got: {p}"
        );
    }
    // direction=desc is the documented pagination direction this function uses.
    assert!(
        paths[0].contains("direction=desc"),
        "FC-5A: requests must use direction=desc; got: {}",
        paths[0]
    );
}

// ---------------------------------------------------------------------------
// FC-5B: unrelated orders only, across multiple pages
// ---------------------------------------------------------------------------

#[test]
fn fc5b_no_match_across_multiple_pages_returns_empty_only_at_true_end_of_feed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();

    std::thread::spawn(move || {
        // Page 1: full page, all unrelated.
        {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);
            cap.lock().unwrap().push(path);
            let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                .map(|i| activity_json(&format!("act-p1-{i:04}"), "order-unrelated-a"))
                .collect();
            write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
        }
        // Page 2: partial page (2 items), all unrelated -- true end of feed.
        {
            let (mut stream, _) = listener.accept().unwrap();
            let path = read_request_path(&mut stream);
            cap.lock().unwrap().push(path);
            let acts = serde_json::json!([
                activity_json("act-p2-0000", "order-unrelated-b"),
                activity_json("act-p2-0001", "order-unrelated-b"),
            ]);
            write_json_response(&mut stream, &acts.to_string());
        }
    });

    let adapter = adapter_for(port);
    let result = adapter
        .fetch_fill_activities_for_order("order-target-never-appears")
        .expect("FC-5B: must succeed (empty is a valid, proven result)");

    assert!(
        result.is_empty(),
        "FC-5B: no activity anywhere in the feed belongs to the target order"
    );

    let paths = captured.lock().unwrap();
    assert_eq!(
        paths.len(),
        2,
        "FC-5B: must scan through to the true end of the feed (partial page), not stop early"
    );
}

// ---------------------------------------------------------------------------
// FC-5C: page budget exhausted before reaching the end of the feed
// ---------------------------------------------------------------------------

#[test]
fn fc5c_page_budget_exhausted_before_end_of_feed_fails_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        // Every page up to the max is full and unrelated -- the feed never
        // actually ends within the searched window.
        for page in 0..FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request_path(&mut stream);
            let acts: Vec<serde_json::Value> = (0..FILL_ACTIVITIES_PAGE_SIZE)
                .map(|i| activity_json(&format!("act-p{page}-{i:04}"), "order-unrelated"))
                .collect();
            write_json_response(&mut stream, &serde_json::to_string(&acts).unwrap());
        }
    });

    let adapter = adapter_for(port);
    let result = adapter.fetch_fill_activities_for_order("order-target-out-of-window");

    assert!(
        result.is_err(),
        "FC-5C: exhausting the page budget without reaching end-of-feed must return Err, \
         never a possibly-incomplete match set"
    );
    match result.unwrap_err() {
        BrokerError::Transient { detail } => {
            assert!(
                detail.contains("exhausted") || detail.to_lowercase().contains("page"),
                "FC-5C: error detail must explain the page-budget exhaustion; got: {detail}"
            );
        }
        other => panic!("FC-5C: expected Transient error, got {other:?}"),
    }
}
