//! OPS-01 — Incident lifecycle foundation proof.
//!
//! Tests:
//!
//! - OPS01-I1: GET /api/v1/incidents without DB → 200, truth_state="no_db",
//!   rows=[], backend non-empty.  Empty rows must not be interpreted as absence
//!   of incidents.
//! - OPS01-I2: POST /api/v1/incidents without DB → 503, gate="db_pool".
//! - OPS01-I3: POST /api/v1/incidents with empty title → 400, gate="title_present".
//! - OPS01-I4: POST /api/v1/incidents with invalid severity → 400, gate="severity_valid".
//! - OPS01-I5: GET /api/v1/alerts/triage without DB — linked_incident_id is
//!   None for all rows (incident linkage requires DB; fail-closed to None).
//! - OPS01-I6 (#[ignore], DB-backed): POST create incident + GET list shows row.
//! - OPS01-I7 (#[ignore], DB-backed): POST create incident with linked_alert_id;
//!   GET /api/v1/alerts/triage shows linked_incident_id populated on matching row.

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

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body must be valid JSON");
    (status, json)
}

async fn get(router: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    call(router, req).await
}

async fn post_json(
    router: axum::Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    call(router, req).await
}

fn str_field<'a>(json: &'a serde_json::Value, key: &str) -> &'a str {
    json.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("missing string field '{key}' in: {json}"))
}

// ---------------------------------------------------------------------------
// OPS01-I1: GET /api/v1/incidents without DB → no_db, empty rows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ops01_i1_get_incidents_no_db_returns_no_db_empty_rows() {
    let (status, json) = get(make_router(), "/api/v1/incidents").await;
    assert_eq!(status, StatusCode::OK, "OPS01-I1: expected 200: {json}");
    assert_eq!(
        str_field(&json, "truth_state"),
        "no_db",
        "OPS01-I1: truth_state must be no_db when no pool"
    );
    assert_eq!(
        str_field(&json, "canonical_route"),
        "/api/v1/incidents",
        "OPS01-I1: canonical_route must be /api/v1/incidents"
    );
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("OPS01-I1: rows must be an array");
    assert!(
        rows.is_empty(),
        "OPS01-I1: rows must be empty without DB: {json}"
    );
    let backend = str_field(&json, "backend");
    assert!(!backend.is_empty(), "OPS01-I1: backend must be non-empty");
}

// ---------------------------------------------------------------------------
// OPS01-I2: POST /api/v1/incidents without DB → 503 db_pool gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ops01_i2_post_incident_no_db_returns_503() {
    let body = serde_json::json!({
        "title": "Manual test incident",
        "severity": "warning"
    });
    let (status, json) = post_json(make_router(), "/api/v1/incidents", body).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "OPS01-I2: expected 503: {json}"
    );
    assert_eq!(
        json.get("gate").and_then(|v| v.as_str()),
        Some("db_pool"),
        "OPS01-I2: gate must be db_pool: {json}"
    );
}

// ---------------------------------------------------------------------------
// OPS01-I3: POST /api/v1/incidents with empty title → 400 title_present gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ops01_i3_post_incident_empty_title_returns_400() {
    let body = serde_json::json!({
        "title": "   ",
        "severity": "warning"
    });
    let (status, json) = post_json(make_router(), "/api/v1/incidents", body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "OPS01-I3: expected 400: {json}"
    );
    assert_eq!(
        json.get("gate").and_then(|v| v.as_str()),
        Some("title_present"),
        "OPS01-I3: gate must be title_present: {json}"
    );
}

// ---------------------------------------------------------------------------
// OPS01-I4: POST /api/v1/incidents with invalid severity → 400 severity_valid
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ops01_i4_post_incident_invalid_severity_returns_400() {
    let body = serde_json::json!({
        "title": "Severity test",
        "severity": "catastrophic"
    });
    let (status, json) = post_json(make_router(), "/api/v1/incidents", body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "OPS01-I4: expected 400: {json}"
    );
    assert_eq!(
        json.get("gate").and_then(|v| v.as_str()),
        Some("severity_valid"),
        "OPS01-I4: gate must be severity_valid: {json}"
    );
}

// ---------------------------------------------------------------------------
// OPS01-I5: triage rows have linked_incident_id=null when no DB
// ---------------------------------------------------------------------------

/// Without a DB pool the incident_map is empty; all triage rows must carry
/// linked_incident_id=null.  Proves fail-closed linkage: no DB → no linkage,
/// never a stale/fabricated incident reference.
#[tokio::test]
async fn ops01_i5_triage_linked_incident_id_null_when_no_db() {
    let (status, json) = get(make_router(), "/api/v1/alerts/triage").await;
    assert_eq!(status, StatusCode::OK, "OPS01-I5: expected 200: {json}");
    let rows = json["rows"]
        .as_array()
        .expect("OPS01-I5: rows must be array");
    for row in rows {
        assert!(
            row.get("linked_incident_id").is_none() || row["linked_incident_id"].is_null(),
            "OPS01-I5: linked_incident_id must be null without DB: {row}"
        );
    }
}

// ---------------------------------------------------------------------------
// OPS01-I6: DB-backed — POST create + GET list (requires MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

/// POST /api/v1/incidents creates a row; GET /api/v1/incidents returns it with
/// truth_state="active" and the correct fields.
#[tokio::test]
#[ignore]
async fn ops01_i6_db_create_incident_and_list() {
    let db_url = match std::env::var("MQK_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return,
    };

    let pool = sqlx::PgPool::connect(&db_url)
        .await
        .expect("OPS01-I6: pool connect");

    // Pre-test cleanup.
    sqlx::query("DELETE FROM sys_incidents WHERE title = 'OPS01-I6 test incident'")
        .execute(&pool)
        .await
        .expect("OPS01-I6: pre-test cleanup");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let body = serde_json::json!({
        "title": "OPS01-I6 test incident",
        "severity": "warning",
        "opened_by": "ops01_test"
    });
    let (create_status, create_json) = post_json(router.clone(), "/api/v1/incidents", body).await;
    assert_eq!(
        create_status,
        StatusCode::OK,
        "OPS01-I6: create must return 200: {create_json}"
    );
    assert_eq!(
        str_field(&create_json, "status"),
        "open",
        "OPS01-I6: new incident must have status=open"
    );
    assert_eq!(
        str_field(&create_json, "severity"),
        "warning",
        "OPS01-I6: severity must be warning"
    );
    let incident_id = str_field(&create_json, "incident_id").to_string();
    assert!(
        !incident_id.is_empty(),
        "OPS01-I6: incident_id must be non-empty"
    );

    // List must include the new row.
    let (list_status, list_json) = get(router, "/api/v1/incidents").await;
    assert_eq!(
        list_status,
        StatusCode::OK,
        "OPS01-I6: list must return 200: {list_json}"
    );
    assert_eq!(
        str_field(&list_json, "truth_state"),
        "active",
        "OPS01-I6: truth_state must be active with DB"
    );
    let rows = list_json["rows"]
        .as_array()
        .expect("OPS01-I6: rows must be array");
    let found = rows
        .iter()
        .any(|r| r.get("incident_id").and_then(|v| v.as_str()) == Some(&incident_id));
    assert!(
        found,
        "OPS01-I6: created incident must appear in list rows: {list_json}"
    );

    // Cleanup.
    sqlx::query("DELETE FROM sys_incidents WHERE title = 'OPS01-I6 test incident'")
        .execute(&pool)
        .await
        .expect("OPS01-I6: post-test cleanup");
}

// ---------------------------------------------------------------------------
// OPS01-I7: DB-backed — linked_incident_id populates in triage
// ---------------------------------------------------------------------------

/// POST an incident with a `linked_alert_id` that matches a real fault-signal
/// class slug.  GET /api/v1/alerts/triage must show that triage row with
/// `linked_incident_id` set to the created incident's ID.
///
/// Uses `reconcile.dispatch_block.dirty` as the synthetic alert because it is
/// reliably triggered by publishing a dirty reconcile snapshot.
#[tokio::test]
#[ignore]
async fn ops01_i7_db_linked_incident_id_surfaces_in_triage() {
    let db_url = match std::env::var("MQK_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return,
    };

    let pool = sqlx::PgPool::connect(&db_url)
        .await
        .expect("OPS01-I7: pool connect");

    sqlx::query("DELETE FROM sys_incidents WHERE title = 'OPS01-I7 linked incident'")
        .execute(&pool)
        .await
        .expect("OPS01-I7: pre-test cleanup");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Inject a dirty reconcile so the alert_id is present in triage rows.
    st.publish_reconcile_snapshot(state::ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-04-11T00:00:00Z".to_string()),
        snapshot_watermark_ms: None,
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("ops01-i7 synthetic dirty".to_string()),
    })
    .await;

    let router = routes::build_router(st);

    // Create an incident referencing the reconcile alert class slug.
    let body = serde_json::json!({
        "title": "OPS01-I7 linked incident",
        "severity": "critical",
        "linked_alert_id": "reconcile.dispatch_block.dirty",
        "opened_by": "ops01_test"
    });
    let (create_status, create_json) = post_json(router.clone(), "/api/v1/incidents", body).await;
    assert_eq!(
        create_status,
        StatusCode::OK,
        "OPS01-I7: create must return 200: {create_json}"
    );
    let incident_id = str_field(&create_json, "incident_id").to_string();

    // Triage must surface linked_incident_id on the matching row.
    let (triage_status, triage_json) = get(router, "/api/v1/alerts/triage").await;
    assert_eq!(
        triage_status,
        StatusCode::OK,
        "OPS01-I7: triage must return 200: {triage_json}"
    );

    let rows = triage_json["rows"]
        .as_array()
        .expect("OPS01-I7: rows must be array");
    let alert_row = rows
        .iter()
        .find(|r| {
            r.get("alert_id").and_then(|v| v.as_str()) == Some("reconcile.dispatch_block.dirty")
        })
        .expect("OPS01-I7: reconcile.dispatch_block.dirty row must be present in triage");

    let linked = alert_row
        .get("linked_incident_id")
        .and_then(|v| v.as_str())
        .expect("OPS01-I7: linked_incident_id must be present and non-null");
    assert_eq!(
        linked, incident_id,
        "OPS01-I7: linked_incident_id must match created incident_id"
    );

    // Cleanup.
    sqlx::query("DELETE FROM sys_incidents WHERE title = 'OPS01-I7 linked incident'")
        .execute(&pool)
        .await
        .expect("OPS01-I7: post-test cleanup");
}
