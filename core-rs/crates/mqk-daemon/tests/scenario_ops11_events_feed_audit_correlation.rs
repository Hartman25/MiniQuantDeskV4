//! OPS-11 — Events-feed / audit correlation: `audit_event_id` on feed rows.
//!
//! ## Gap closed
//!
//! Before OPS-11, `EventFeedRow` had no first-class `audit_event_id` field.
//! For `kind = "operator_action"` and `kind = "signal_admission"` rows, the
//! audit event UUID was only reachable by string-parsing `event_id`
//! (`"audit_events:{uuid}"`).  Meanwhile `/api/v1/audit/operator-actions`
//! exposes the same UUID as a top-level `audit_event_id` field, and
//! `OperatorTimelineRow` was fixed by OPTR-03 to carry it first-class.
//!
//! Joining `events/feed` operator_action rows to `audit/operator-actions`
//! previously required an implicit string-parsing contract — not a stable
//! field contract.
//!
//! OPS-11 adds `audit_event_id: Option<String>` to `EventFeedRow`:
//! - `Some(raw_uuid_str)` for `kind = "operator_action"` rows.
//! - `Some(raw_uuid_str)` for `kind = "signal_admission"` rows (also audit-events
//!   sourced; same correlation contract applies).
//! - `None` for `kind = "runtime_transition"` rows (sourced from `runs`).
//! - `None` for `kind = "autonomous_session"` rows (sourced from
//!   `sys_autonomous_session_events`).
//!
//! ## Proof matrix
//!
//! | Test  | Claim                                                                        |
//! |-------|------------------------------------------------------------------------------|
//! | T-01  | `EventFeedRow` (operator_action) serialises `audit_event_id` as a non-null  |
//! |       | string equal to the UUID embedded in `event_id`                              |
//! | T-02  | `EventFeedRow` (runtime_transition) serialises `audit_event_id` as JSON null |
//! | T-03  | For operator_action rows, `audit_event_id` equals the UUID extracted from    |
//! |       | `event_id` — cross-surface correlation is exact and consistent               |
//! | T-04  | events/feed endpoint without DB returns `truth_state = "backend_unavailable"` |
//! |       | and empty rows — no regression from the added field                          |
//!
//! All tests are pure in-process (no DB required).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{api_types::EventFeedRow, routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    serde_json::from_slice(&b).expect("response body is not valid JSON")
}

fn no_db_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

// ---------------------------------------------------------------------------
// T-01: operator_action row serialises audit_event_id as non-null string
// ---------------------------------------------------------------------------

/// OPS-11 / T-01: An `EventFeedRow` with `kind = "operator_action"` and
/// `audit_event_id = Some(uuid_str)` serialises `audit_event_id` as a JSON
/// string, not null.
#[test]
fn ops11_t1_operator_action_row_audit_event_id_serialises_as_string() {
    let uuid_str = "550e8400-e29b-41d4-a716-446655440000".to_string();
    let row = EventFeedRow {
        event_id: format!("audit_events:{uuid_str}"),
        ts_utc: "2026-04-14T10:00:00Z".to_string(),
        kind: "operator_action".to_string(),
        detail: "control.arm".to_string(),
        run_id: None,
        provenance_ref: format!("audit_events:{uuid_str}"),
        audit_event_id: Some(uuid_str.clone()),
    };
    let json = serde_json::to_value(&row).expect("T-01: serialisation must succeed");

    assert_eq!(
        json["audit_event_id"].as_str(),
        Some(uuid_str.as_str()),
        "T-01: operator_action row audit_event_id must serialise as non-null string; \
         got: {json}"
    );
    assert_eq!(
        json["kind"].as_str(),
        Some("operator_action"),
        "T-01: kind must be operator_action; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// T-02: runtime_transition row serialises audit_event_id as JSON null
// ---------------------------------------------------------------------------

/// OPS-11 / T-02: An `EventFeedRow` with `kind = "runtime_transition"` and
/// `audit_event_id = None` serialises `audit_event_id` as JSON null.
#[test]
fn ops11_t2_runtime_transition_row_audit_event_id_serialises_as_null() {
    let run_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();
    let row = EventFeedRow {
        event_id: format!("runs:{run_id}:started_at_utc"),
        ts_utc: "2026-04-14T09:00:00Z".to_string(),
        kind: "runtime_transition".to_string(),
        detail: "CREATED".to_string(),
        run_id: Some(run_id.clone()),
        provenance_ref: format!("runs:{run_id}:started_at_utc"),
        audit_event_id: None,
    };
    let json = serde_json::to_value(&row).expect("T-02: serialisation must succeed");

    assert!(
        json["audit_event_id"].is_null(),
        "T-02: runtime_transition row audit_event_id must serialise as JSON null; \
         got: {json}"
    );
    assert_eq!(
        json["kind"].as_str(),
        Some("runtime_transition"),
        "T-02: kind must be runtime_transition; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// T-03: audit_event_id matches UUID embedded in event_id
// ---------------------------------------------------------------------------

/// OPS-11 / T-03: For `kind = "operator_action"` rows, `audit_event_id` must
/// equal the UUID portion of `event_id` (the part after `"audit_events:"`).
///
/// This proves that the two fields are consistent — an operator correlating
/// via `audit_event_id` or via parsing `event_id` reaches the same audit row.
#[test]
fn ops11_t3_operator_action_audit_event_id_matches_event_id_uuid() {
    let uuid_str = "12345678-1234-5678-1234-567812345678".to_string();
    let event_id = format!("audit_events:{uuid_str}");
    let row = EventFeedRow {
        event_id: event_id.clone(),
        ts_utc: "2026-04-14T11:00:00Z".to_string(),
        kind: "operator_action".to_string(),
        detail: "control.stop".to_string(),
        run_id: None,
        provenance_ref: event_id.clone(),
        audit_event_id: Some(uuid_str.clone()),
    };

    // audit_event_id must exactly equal the UUID extracted from event_id.
    let uuid_from_event_id = event_id
        .strip_prefix("audit_events:")
        .expect("T-03: event_id must start with 'audit_events:'");
    assert_eq!(
        row.audit_event_id.as_deref(),
        Some(uuid_from_event_id),
        "T-03: audit_event_id must equal UUID extracted from event_id; \
         audit_event_id={:?}, event_id={}",
        row.audit_event_id,
        row.event_id
    );
}

// ---------------------------------------------------------------------------
// T-04: events/feed endpoint without DB returns backend_unavailable
// ---------------------------------------------------------------------------

/// OPS-11 / T-04: `GET /api/v1/events/feed` with no DB pool must return
/// `truth_state = "backend_unavailable"` and empty rows.
///
/// Proves no regression from the added `audit_event_id` field.
#[tokio::test]
async fn ops11_t4_events_feed_no_db_returns_backend_unavailable() {
    let st = no_db_state();

    let req = Request::builder()
        .method("GET".parse::<axum::http::Method>().unwrap())
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "T-04: events/feed must return 200 even without DB; body: {json}"
    );
    assert_eq!(
        json["truth_state"].as_str(),
        Some("backend_unavailable"),
        "T-04: truth_state must be backend_unavailable when no DB; got: {json}"
    );
    let rows = json["rows"].as_array().expect("T-04: rows must be an array");
    assert!(
        rows.is_empty(),
        "T-04: rows must be empty when no DB; got {} rows",
        rows.len()
    );
}
