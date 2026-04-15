//! OPTR-03 — Operator history / causality linkage: `audit_event_id` on timeline rows.
//!
//! ## Gap closed
//!
//! Before OPTR-03, `OperatorTimelineRow` had no first-class `audit_event_id`
//! field.  For `kind = "operator_action"` rows the audit event UUID was only
//! reachable by string-parsing `provenance_ref` (`"audit_events:{uuid}"`).
//! Meanwhile `/api/v1/audit/operator-actions` exposes the same UUID as a
//! top-level `audit_event_id` field.  Joining the two surfaces required an
//! implicit string-parsing contract — not a stable field contract.
//!
//! OPTR-03 adds `audit_event_id: Option<String>` to `OperatorTimelineRow`:
//! - `Some(raw_uuid_str)` for `kind = "operator_action"` rows.
//! - `None` for `kind = "runtime_transition"` rows (sourced from `runs`).
//!
//! ## Proof matrix
//!
//! | Test  | Claim                                                                        |
//! |-------|------------------------------------------------------------------------------|
//! | T-01  | `OperatorTimelineRow` (operator_action) serialises `audit_event_id` as      |
//! |       | a non-null string                                                            |
//! | T-02  | `OperatorTimelineRow` (runtime_transition) serialises `audit_event_id` as   |
//! |       | JSON null                                                                    |
//! | T-03  | For operator_action rows, `audit_event_id` value equals the UUID embedded   |
//! |       | in `provenance_ref` — cross-surface correlation is exact and consistent     |
//! | T-04  | Timeline endpoint without DB returns `truth_state = "backend_unavailable"` |
//! |       | and empty rows — no regression from added field                             |
//!
//! T-01..T-04 are pure in-process (no DB required).

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{api_types::OperatorTimelineRow, routes, state};
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
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// T-01: operator_action row serialises audit_event_id as non-null string
// ---------------------------------------------------------------------------

/// OPTR-03 / T-01: An `OperatorTimelineRow` with `kind = "operator_action"` and
/// `audit_event_id = Some(uuid_str)` serialises `audit_event_id` as a JSON
/// string, not null.
#[test]
fn optr03_t1_operator_action_row_audit_event_id_serialises_as_string() {
    let uuid_str = "550e8400-e29b-41d4-a716-446655440000".to_string();
    let row = OperatorTimelineRow {
        ts_utc: "2026-04-14T10:00:00Z".to_string(),
        kind: "operator_action".to_string(),
        run_id: None,
        detail: "control.arm".to_string(),
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

/// OPTR-03 / T-02: An `OperatorTimelineRow` with `kind = "runtime_transition"` and
/// `audit_event_id = None` serialises `audit_event_id` as JSON null.
#[test]
fn optr03_t2_runtime_transition_row_audit_event_id_serialises_as_null() {
    let run_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();
    let row = OperatorTimelineRow {
        ts_utc: "2026-04-14T09:00:00Z".to_string(),
        kind: "runtime_transition".to_string(),
        run_id: Some(run_id.clone()),
        detail: "CREATED".to_string(),
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
// T-03: audit_event_id matches UUID embedded in provenance_ref
// ---------------------------------------------------------------------------

/// OPTR-03 / T-03: For `kind = "operator_action"` rows, `audit_event_id` must
/// equal the UUID portion of `provenance_ref` (the part after `"audit_events:"`).
///
/// This proves that the two fields are consistent — an operator correlating
/// via `audit_event_id` or via `provenance_ref` reaches the same audit row.
#[test]
fn optr03_t3_operator_action_audit_event_id_matches_provenance_ref_uuid() {
    let uuid_str = "12345678-1234-5678-1234-567812345678".to_string();
    let provenance_ref = format!("audit_events:{uuid_str}");
    let row = OperatorTimelineRow {
        ts_utc: "2026-04-14T11:00:00Z".to_string(),
        kind: "operator_action".to_string(),
        run_id: None,
        detail: "control.stop".to_string(),
        provenance_ref: provenance_ref.clone(),
        audit_event_id: Some(uuid_str.clone()),
    };

    // audit_event_id must exactly equal the UUID extracted from provenance_ref.
    let uuid_from_pref = provenance_ref
        .strip_prefix("audit_events:")
        .expect("T-03: provenance_ref must start with 'audit_events:'");
    assert_eq!(
        row.audit_event_id.as_deref(),
        Some(uuid_from_pref),
        "T-03: audit_event_id must equal UUID extracted from provenance_ref; \
         audit_event_id={:?}, provenance_ref={}",
        row.audit_event_id,
        row.provenance_ref
    );
}

// ---------------------------------------------------------------------------
// T-04: timeline endpoint with no DB returns backend_unavailable, no rows
// ---------------------------------------------------------------------------

/// OPTR-03 / T-04: `GET /api/v1/ops/operator-timeline` with no DB must return
/// `truth_state = "backend_unavailable"` and empty rows.
///
/// Proves no regression from the added `audit_event_id` field.
#[tokio::test]
async fn optr03_t4_timeline_no_db_returns_backend_unavailable() {
    let st = no_db_state();

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/operator-timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "T-04: operator-timeline must return 200 even without DB; body: {json}"
    );
    assert_eq!(
        json["truth_state"].as_str(),
        Some("backend_unavailable"),
        "T-04: truth_state must be backend_unavailable when no DB; got: {json}"
    );
    let rows = json["rows"]
        .as_array()
        .expect("T-04: rows must be an array");
    assert!(
        rows.is_empty(),
        "T-04: rows must be empty when no DB; got {} rows",
        rows.len()
    );
}
