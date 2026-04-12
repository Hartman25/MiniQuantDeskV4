//! A3/A4 — Operator surface mount tests.
//!
//! Tests:
//!
//! - A3-01: GET /api/v1/system/topology returns 200 with truth_state="active",
//!   canonical_route correct, 5 service nodes present with expected service_keys.
//! - A3-02: topology services array contains "daemon.runtime" with health="ok".
//! - A3-03: topology no-DB path — postgres node has health="not_configured".
//! - A3-04: GET /api/v1/incidents returns 200 with truth_state="not_wired",
//!   rows=[], canonical_route correct.
//! - A3-05: incidents note field is non-empty (operator guidance present).
//! - A4-01: GET /api/v1/execution/replace-cancel-chains returns 200 with
//!   truth_state="not_wired", chains=[], canonical_route correct.
//! - A4-02: replace-cancel-chains note field is non-empty.
//! - A4-03: GET /api/v1/alerts/triage returns 200 with
//!   truth_state="no_db" (no DB pool in test router; OPS-02), canonical_route correct.
//! - A4-04: alerts/triage triage_note field is non-empty.
//! - A4-05: alerts/triage rows contain status="unacked" for every emitted row
//!   (triage lifecycle not backed).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn get(router: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body must be valid JSON");
    (status, json)
}

fn str_field<'a>(json: &'a serde_json::Value, key: &str) -> &'a str {
    json.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("missing string field '{key}' in: {json}"))
}

// ---------------------------------------------------------------------------
// A3-01: topology 200 + truth_state + canonical_route + 5 services
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a3_01_topology_200_active_five_services() {
    let (status, json) = get(make_router(), "/api/v1/system/topology").await;
    assert_eq!(status, StatusCode::OK, "expected 200: {json}");
    assert_eq!(str_field(&json, "truth_state"), "active");
    assert_eq!(
        str_field(&json, "canonical_route"),
        "/api/v1/system/topology"
    );
    let services = json
        .get("services")
        .and_then(|v| v.as_array())
        .expect("services must be an array");
    assert_eq!(services.len(), 5, "expected 5 service nodes: {json}");
}

// ---------------------------------------------------------------------------
// A3-02: daemon.runtime node has health="ok"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a3_02_topology_daemon_runtime_ok() {
    let (_, json) = get(make_router(), "/api/v1/system/topology").await;
    let services = json["services"].as_array().expect("services array");
    let runtime_node = services
        .iter()
        .find(|s| s.get("service_key").and_then(|v| v.as_str()) == Some("daemon.runtime"))
        .expect("daemon.runtime node must be present");
    assert_eq!(
        runtime_node.get("health").and_then(|v| v.as_str()),
        Some("ok"),
        "daemon.runtime health must be ok"
    );
}

// ---------------------------------------------------------------------------
// A3-03: topology no-DB — postgres node has health="not_configured"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a3_03_topology_no_db_postgres_not_configured() {
    // Default router has no DB pool.
    let (_, json) = get(make_router(), "/api/v1/system/topology").await;
    let services = json["services"].as_array().expect("services array");
    let pg_node = services
        .iter()
        .find(|s| s.get("service_key").and_then(|v| v.as_str()) == Some("postgres"))
        .expect("postgres node must be present");
    assert_eq!(
        pg_node.get("health").and_then(|v| v.as_str()),
        Some("not_configured"),
        "postgres health must be not_configured when no pool"
    );
}

// ---------------------------------------------------------------------------
// A3-04: incidents 200 + truth_state="no_db" (no pool in test router)
// ---------------------------------------------------------------------------

/// Without DB pool: truth_state="no_db", rows=[] (authoritative empty; not
/// absence of incidents).  With DB: truth_state="active" + sys_incidents rows.
/// Test router has no DB, so expects "no_db" (OPS-01).
#[tokio::test]
async fn a3_04_incidents_no_db_empty_rows() {
    let (status, json) = get(make_router(), "/api/v1/incidents").await;
    assert_eq!(status, StatusCode::OK, "expected 200: {json}");
    assert_eq!(str_field(&json, "truth_state"), "no_db");
    assert_eq!(str_field(&json, "canonical_route"), "/api/v1/incidents");
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be an array");
    assert!(rows.is_empty(), "rows must be empty when no DB: {json}");
}

// ---------------------------------------------------------------------------
// A3-05: incidents backend field is non-empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a3_05_incidents_backend_non_empty() {
    let (_, json) = get(make_router(), "/api/v1/incidents").await;
    let backend = str_field(&json, "backend");
    assert!(!backend.is_empty(), "backend must be non-empty");
}

// ---------------------------------------------------------------------------
// A4-01: replace-cancel-chains 200 + truth_state="no_db" (no pool) + empty chains
//
// EXEC-02: route is now DB-backed. Without DB pool truth_state = "no_db"
// (previously "not_wired" when the surface was a static stub).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a4_01_replace_cancel_chains_no_db_empty() {
    let (status, json) = get(make_router(), "/api/v1/execution/replace-cancel-chains").await;
    assert_eq!(status, StatusCode::OK, "expected 200: {json}");
    // EXEC-02: no DB pool → truth_state="no_db" (not "not_wired").
    assert_eq!(str_field(&json, "truth_state"), "no_db");
    assert_eq!(
        str_field(&json, "canonical_route"),
        "/api/v1/execution/replace-cancel-chains"
    );
    let chains = json
        .get("chains")
        .and_then(|v| v.as_array())
        .expect("chains must be an array");
    assert!(chains.is_empty(), "chains must be empty when no DB: {json}");
}

// ---------------------------------------------------------------------------
// A4-02: replace-cancel-chains note field non-empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a4_02_replace_cancel_chains_note_non_empty() {
    let (_, json) = get(make_router(), "/api/v1/execution/replace-cancel-chains").await;
    let note = str_field(&json, "note");
    assert!(!note.is_empty(), "note must be non-empty operator guidance");
}

// ---------------------------------------------------------------------------
// A4-03: alerts/triage 200 + truth_state conditional on DB presence
// ---------------------------------------------------------------------------

/// Without DB: truth_state="no_db" (ack state unavailable; OPS-02).
/// With DB:    truth_state="active" (ack state from sys_alert_acks).
/// The test router has no DB, so expects "no_db".
#[tokio::test]
async fn a4_03_alerts_triage_200_alerts_no_triage() {
    let (status, json) = get(make_router(), "/api/v1/alerts/triage").await;
    assert_eq!(status, StatusCode::OK, "expected 200: {json}");
    // No DB pool in make_router() — truth_state must be "no_db".
    assert_eq!(str_field(&json, "truth_state"), "no_db");
    assert_eq!(str_field(&json, "canonical_route"), "/api/v1/alerts/triage");
    // rows must be an array (may be empty in clean state)
    json.get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be an array");
}

// ---------------------------------------------------------------------------
// A4-04: alerts/triage triage_note non-empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a4_04_alerts_triage_note_non_empty() {
    let (_, json) = get(make_router(), "/api/v1/alerts/triage").await;
    let note = str_field(&json, "triage_note");
    assert!(
        !note.is_empty(),
        "triage_note must be non-empty operator guidance"
    );
}

// ---------------------------------------------------------------------------
// A4-05: alerts/triage all rows have status="unacked"
// ---------------------------------------------------------------------------

/// In a fresh state the rows array is empty; if any rows are present (e.g.,
/// WS continuity signals), each must carry status="unacked".
#[tokio::test]
async fn a4_05_alerts_triage_rows_all_unacked() {
    let (_, json) = get(make_router(), "/api/v1/alerts/triage").await;
    let rows = json["rows"].as_array().expect("rows array");
    for row in rows {
        let status = row
            .get("status")
            .and_then(|v| v.as_str())
            .expect("row must have status field");
        assert_eq!(
            status, "unacked",
            "all triage rows must have status=unacked (triage not backed): {row}"
        );
    }
}
