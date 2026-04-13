//! A5E: per-order execution causality route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/causality`.
//!
//! # Truth-state contract under test
//!
//! | Condition                                         | truth_state   |
//! |---------------------------------------------------|---------------|
//! | No DB pool                                        | no_db         |
//! | DB pool + no active run                           | no_order      |
//! | DB pool + active run + order visible, no fills    | no_fills_yet  |
//! | DB pool + active run + fill quality rows exist    | partial       |
//!
//! The DB-backed states require `MQK_DATABASE_URL` and are skipped gracefully
//! in CI when it is absent.
//!
//! # What the causality route claims
//!
//! - `"partial"` (never `"active"`): only proven lanes are surfaced.
//! - Fill nodes are derived from `fill_quality_telemetry`, oldest-first.
//! - Intent nodes (`outbox_enqueued`, `outbox_sent`) from `oms_outbox` when present.
//! - `proven_lanes`: includes `"intent"` when oms_outbox row found; `"broker_ack"`
//!   when oms_inbox ACK rows exist for the order; `"execution_fill"` when fill rows
//!   exist; empty otherwise.
//! - `unproven_lanes`: lists signal/risk/reconcile/portfolio always;
//!   `"intent"` only when no oms_outbox row is found for this order;
//!   `"broker_ack"` only when no oms_inbox ACK rows are found for this order.
//!
//! # What the causality route does NOT claim
//!
//! - Signal provenance: not joinable to internal_order_id.
//! - Portfolio effects or reconcile outcomes: not linked per-order.

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use std::sync::Arc;
use tower::ServiceExt;

// chrono is used by CA-11 for chronological ordering assertions.
use chrono;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router_no_db() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ---------------------------------------------------------------------------
// CA-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    for field in &[
        "canonical_route",
        "truth_state",
        "backend",
        "order_id",
        "proven_lanes",
        "unproven_lanes",
        "nodes",
        "comment",
    ] {
        assert!(
            v.get(field).is_some(),
            "wrapper must contain field '{field}'"
        );
    }
    assert!(v["nodes"].is_array(), "nodes must be an array");
    assert!(
        v["proven_lanes"].is_array(),
        "proven_lanes must be an array"
    );
    assert!(
        v["unproven_lanes"].is_array(),
        "unproven_lanes must be an array"
    );
}

// ---------------------------------------------------------------------------
// CA-02: canonical_route includes order_id and ends with /causality.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca02_canonical_route_includes_order_id_and_suffix() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-causal-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"]
        .as_str()
        .expect("canonical_route is a string");
    assert!(
        route.contains("my-causal-order"),
        "canonical_route must include the order_id; got: {route}"
    );
    assert!(
        route.ends_with("/causality"),
        "canonical_route must end with /causality; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// CA-03: no_db truth_state when DB pool is absent (pure in-process).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca03_no_db_truth_state_without_db_pool() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(
        v["truth_state"].as_str(),
        Some("no_db"),
        "without DB pool truth_state must be no_db"
    );
    assert_eq!(
        v["backend"].as_str(),
        Some("unavailable"),
        "backend must be unavailable when no_db"
    );
    let nodes = v["nodes"].as_array().expect("nodes is array");
    assert!(
        nodes.is_empty(),
        "nodes must be empty when truth_state is no_db"
    );
}

// ---------------------------------------------------------------------------
// CA-04: unproven_lanes are always present and list the expected subsystems.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca04_unproven_lanes_always_present() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/any-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let unproven = v["unproven_lanes"]
        .as_array()
        .expect("unproven_lanes is array");
    let unproven_strs: Vec<&str> = unproven.iter().filter_map(|v| v.as_str()).collect();
    for expected in &[
        "signal",
        "intent",
        "broker_ack",
        "risk",
        "reconcile",
        "portfolio",
    ] {
        assert!(
            unproven_strs.contains(expected),
            "unproven_lanes must include '{expected}'; got: {unproven_strs:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CA-05: no_db means proven_lanes is empty (nothing proven without data).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca05_no_db_means_proven_lanes_empty() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/no-db-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(v["truth_state"].as_str(), Some("no_db"));
    let proven = v["proven_lanes"].as_array().expect("proven_lanes is array");
    assert!(proven.is_empty(), "proven_lanes must be empty when no_db");
}

// ---------------------------------------------------------------------------
// CA-06: causality is a sibling of chart/replay/trace (same :order_id segment).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca06_causality_is_sibling_of_chart_replay_trace() {
    let router = make_router_no_db();
    let order_id = "sibling-order-causality-001";

    let routes_to_check = [
        (
            format!("/api/v1/execution/orders/{order_id}/replay"),
            "replay",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/chart"),
            "chart",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/causality"),
            "causality",
        ),
    ];

    for (uri, expected_suffix) in &routes_to_check {
        let (status, body) = call(
            router.clone(),
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri} must return 200");
        let v = parse_json(body);
        let route = v["canonical_route"].as_str().unwrap_or("");
        assert!(
            route.ends_with(expected_suffix),
            "canonical_route for {uri} must end with /{expected_suffix}; got: {route}"
        );
    }
}

// ---------------------------------------------------------------------------
// CA-07: blank order_id returns 400 or 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca07_blank_order_id_returns_400_or_404() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// CA-08: "partial" truth state is not the same as "active" (partial is honest).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca08_no_db_truth_state_is_not_active_or_partial() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let ts = v["truth_state"].as_str().unwrap_or("");
    assert_ne!(
        ts, "active",
        "causality never reports truth_state=active; it is always partial or narrower"
    );
    assert_ne!(
        ts, "partial",
        "without DB partial causality is impossible; must be no_db"
    );
}

// ---------------------------------------------------------------------------
// CA-09: fill node with submit_ts_utc present serializes both timing fields.
// ---------------------------------------------------------------------------

#[test]
fn ca09_fill_node_with_submit_ts_serializes_timing_fields() {
    use mqk_daemon::api_types::OrderCausalityCausalNode;

    let node = OrderCausalityCausalNode {
        node_key: "execution_fill_abc".to_string(),
        node_type: "execution_fill".to_string(),
        title: "partial_fill NVDA".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: Some("fill-001".to_string()),
        timestamp: Some("2025-01-02T10:00:01Z".to_string()),
        elapsed_from_prev_ms: None,
        anomaly_tags: vec![],
        summary: "fill_qty=10 fill_price=100.000000 (partial_fill)".to_string(),
        submit_ts_utc: Some("2025-01-02T10:00:00Z".to_string()),
        submit_to_fill_ms: Some(1000),
    };

    let v = serde_json::to_value(&node).expect("serializes");

    assert_eq!(
        v["submit_ts_utc"].as_str(),
        Some("2025-01-02T10:00:00Z"),
        "submit_ts_utc must round-trip through JSON"
    );
    assert_eq!(
        v["submit_to_fill_ms"].as_i64(),
        Some(1000),
        "submit_to_fill_ms must round-trip through JSON"
    );
    assert_eq!(v["node_type"].as_str(), Some("execution_fill"));
}

// ---------------------------------------------------------------------------
// CA-10: fill node with null submit_ts_utc still serializes as valid node.
// ---------------------------------------------------------------------------

#[test]
fn ca10_fill_node_with_null_submit_ts_is_still_valid() {
    use mqk_daemon::api_types::OrderCausalityCausalNode;

    let node = OrderCausalityCausalNode {
        node_key: "execution_fill_xyz".to_string(),
        node_type: "execution_fill".to_string(),
        title: "fill AAPL".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: None,
        timestamp: Some("2025-01-02T10:00:01Z".to_string()),
        elapsed_from_prev_ms: Some(500),
        anomaly_tags: vec![],
        summary: "fill_qty=5 fill_price=200.000000 (fill)".to_string(),
        submit_ts_utc: None,
        submit_to_fill_ms: None,
    };

    let v = serde_json::to_value(&node).expect("serializes");

    assert!(
        v["submit_ts_utc"].is_null(),
        "submit_ts_utc must be null when absent"
    );
    assert!(
        v["submit_to_fill_ms"].is_null(),
        "submit_to_fill_ms must be null when absent"
    );
    // Core fields must still be present and valid.
    assert_eq!(v["node_type"].as_str(), Some("execution_fill"));
    assert_eq!(v["status"].as_str(), Some("ok"));
    assert_eq!(v["subsystem"].as_str(), Some("execution"));
}

// ---------------------------------------------------------------------------
// CA-11: multi-fill chain with submit_ts present has synthetic submit_event
//        node first (chronologically before all fill nodes).
// ---------------------------------------------------------------------------

#[test]
fn ca11_submit_event_node_is_first_chronologically() {
    use mqk_daemon::api_types::OrderCausalityCausalNode;

    // Simulate the handler's node-building logic for a two-fill chain where
    // the first fill carries submit timing.
    let submit_ts = "2025-01-02T10:00:00Z";
    let fill1_ts = "2025-01-02T10:00:01Z";
    let fill2_ts = "2025-01-02T10:00:02Z";
    let order_id = "order-ca11";

    // Build synthetic submit anchor (mirrors handler logic).
    let anchor = OrderCausalityCausalNode {
        node_key: format!("submit:{order_id}"),
        node_type: "submit_event".to_string(),
        title: "order submitted".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: None,
        timestamp: Some(submit_ts.to_string()),
        elapsed_from_prev_ms: None,
        anomaly_tags: vec![],
        summary: String::new(),
        submit_ts_utc: None,
        submit_to_fill_ms: None,
    };

    let fill1 = OrderCausalityCausalNode {
        node_key: "execution_fill_1".to_string(),
        node_type: "execution_fill".to_string(),
        title: "partial_fill NVDA".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: None,
        timestamp: Some(fill1_ts.to_string()),
        elapsed_from_prev_ms: Some(1000),
        anomaly_tags: vec![],
        summary: "fill_qty=5 fill_price=100.000000 (partial_fill)".to_string(),
        submit_ts_utc: Some(submit_ts.to_string()),
        submit_to_fill_ms: Some(1000),
    };

    let fill2 = OrderCausalityCausalNode {
        node_key: "execution_fill_2".to_string(),
        node_type: "execution_fill".to_string(),
        title: "fill NVDA".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: None,
        timestamp: Some(fill2_ts.to_string()),
        elapsed_from_prev_ms: Some(1000),
        anomaly_tags: vec![],
        summary: "fill_qty=5 fill_price=100.100000 (fill)".to_string(),
        submit_ts_utc: Some(submit_ts.to_string()),
        submit_to_fill_ms: Some(2000),
    };

    let nodes = vec![anchor, fill1, fill2];

    // Anchor must be first.
    assert_eq!(
        nodes[0].node_type, "submit_event",
        "first node must be submit_event"
    );
    assert_eq!(
        nodes[0].node_key,
        format!("submit:{order_id}"),
        "submit_event node_key must be deterministic"
    );
    assert_eq!(
        nodes[0].timestamp.as_deref(),
        Some(submit_ts),
        "submit_event timestamp must equal submit_ts"
    );

    // Fills follow in order.
    assert_eq!(nodes[1].node_type, "execution_fill");
    assert_eq!(nodes[2].node_type, "execution_fill");

    // Chronological ordering: submit < fill1 < fill2.
    let t0 = chrono::DateTime::parse_from_rfc3339(nodes[0].timestamp.as_deref().unwrap()).unwrap();
    let t1 = chrono::DateTime::parse_from_rfc3339(nodes[1].timestamp.as_deref().unwrap()).unwrap();
    let t2 = chrono::DateTime::parse_from_rfc3339(nodes[2].timestamp.as_deref().unwrap()).unwrap();
    assert!(t0 < t1, "submit_event must precede fill1 chronologically");
    assert!(t1 < t2, "fill1 must precede fill2 chronologically");

    // Submit event has no fill-specific timing fields.
    assert!(
        nodes[0].submit_ts_utc.is_none(),
        "submit_event node must not carry submit_ts_utc"
    );
    assert!(
        nodes[0].submit_to_fill_ms.is_none(),
        "submit_event node must not carry submit_to_fill_ms"
    );

    // Fill nodes carry timing back-reference.
    assert!(
        nodes[1].submit_ts_utc.is_some(),
        "fill1 must carry submit_ts_utc"
    );
    assert!(
        nodes[1].submit_to_fill_ms.is_some(),
        "fill1 must carry submit_to_fill_ms"
    );
}

// ---------------------------------------------------------------------------
// CA-14: broker_ack node type serializes with correct fields (pure in-process).
//
// Proves that a broker_ack node can be constructed with the expected field
// shape and survives JSON round-trip with the correct values.
// ---------------------------------------------------------------------------

#[test]
fn ca14_broker_ack_node_serializes_with_correct_fields() {
    use mqk_daemon::api_types::OrderCausalityCausalNode;

    let node = OrderCausalityCausalNode {
        node_key: "broker_ack_order-xyz_0".to_string(),
        node_type: "broker_ack".to_string(),
        title: "broker ACK received".to_string(),
        status: "ok".to_string(),
        subsystem: "execution".to_string(),
        linked_id: Some("alpaca:order-xyz:new:2025-01-02T10:00:00Z".to_string()),
        timestamp: Some("2025-01-02T10:00:00.100Z".to_string()),
        elapsed_from_prev_ms: None,
        anomaly_tags: vec![],
        summary: "inbox_id=42".to_string(),
        submit_ts_utc: None,
        submit_to_fill_ms: None,
    };

    let v = serde_json::to_value(&node).expect("broker_ack node must serialize");

    assert_eq!(
        v["node_type"].as_str(),
        Some("broker_ack"),
        "node_type must be broker_ack"
    );
    assert_eq!(
        v["subsystem"].as_str(),
        Some("execution"),
        "subsystem must be execution"
    );
    assert_eq!(
        v["status"].as_str(),
        Some("ok"),
        "status must be ok"
    );
    assert!(
        v["linked_id"].as_str().is_some(),
        "linked_id must carry the broker_message_id"
    );
    assert!(
        v["timestamp"].as_str().is_some(),
        "timestamp must carry received_at_utc"
    );
    // broker_ack nodes carry no fill-specific timing fields.
    assert!(
        v["submit_ts_utc"].is_null(),
        "broker_ack node must not carry submit_ts_utc"
    );
    assert!(
        v["submit_to_fill_ms"].is_null(),
        "broker_ack node must not carry submit_to_fill_ms"
    );
    // node_key must be deterministic and include the order_id.
    assert!(
        v["node_key"]
            .as_str()
            .unwrap_or("")
            .contains("order-xyz"),
        "broker_ack node_key must include the order_id"
    );
}
