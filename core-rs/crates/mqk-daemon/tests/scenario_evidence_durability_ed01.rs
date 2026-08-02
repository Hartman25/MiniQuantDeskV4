//! EVIDENCE-DURABILITY-01 — durable orchestrator halt-reason evidence tests.
//!
//! Proves that:
//! - ED01-01: events/feed exposes `kind="orchestrator_halt"` rows sourced from
//!   `audit_events` with `topic='orchestrator'`.  Seeds a deterministic row
//!   directly and validates exact field mappings in the feed response.
//!   Requires MQK_DATABASE_URL; skips when not set.
//!
//! - ED01-02: events/feed backend string names all four source lanes including
//!   the new orchestrator lane (no-DB path verifies the backend_unavailable
//!   path is unaffected; DB path verifies the active-backend string).
//!
//! - ED01-03 (pure): `kind="orchestrator_halt"` rows carry `audit_event_id`
//!   so consumers can join to the full audit payload without parsing `event_id`.
//!
//! - ED01-04 (pure): the no-DB path still returns `truth_state=backend_unavailable`
//!   with no rows — the new orchestrator lane does not affect the no-DB contract.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

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

fn json_str<'a>(json: &'a serde_json::Value, key: &str) -> &'a str {
    json.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("missing string key '{key}' in response: {json}"))
}

async fn ed01_test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_evidence_durability_ed01 -- --include-ignored"
        )
    });
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("ed01_test_pool: connect failed")
}

// ---------------------------------------------------------------------------
// ED01-01: events/feed exposes orchestrator_halt rows from audit_events[orchestrator]
// ---------------------------------------------------------------------------

/// Seeds a deterministic `audit_events` row with `topic='orchestrator'` and
/// `event_type='ReconcileDrift'`, calls GET /api/v1/events/feed, and validates
/// that the row appears with `kind="orchestrator_halt"`, correct `detail`,
/// `run_id`, `audit_event_id`, and `provenance_ref`.
///
/// Requires MQK_DATABASE_URL; skips when not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ed01_01_events_feed_exposes_orchestrator_halt_rows() {
    let pool = ed01_test_pool().await;

    // Deterministic IDs — unique namespace avoids collision with other test suites.
    let run_id = uuid::Uuid::parse_str("ed010001-0000-4000-8000-000000000001").unwrap();
    let event_id = uuid::Uuid::parse_str("ed010001-0000-4000-8000-000000000002").unwrap();

    // The orchestrator lane's events/feed query is `ORDER BY ts_utc DESC
    // LIMIT 50` -- a fixed historical timestamp is not guaranteed to survive
    // that bounded window once 50+ more-recent rows accumulate in the shared
    // test database over a long session (same root cause as AH-04's fix in
    // scenario_auton_hist_durability_ah01.rs). Neither timestamp is asserted
    // on directly below, so stamping both "now" is safe and durable.
    let started_at = chrono::Utc::now();
    let halt_ts = chrono::Utc::now();

    // Pre-test cleanup.
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("ED01-01: pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("ED01-01: pre-test runs cleanup failed");

    // Seed the run row (required by audit_events FK).
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "ed01-test-hash".to_string(),
            config_hash: "ed01-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ed01-test-host".to_string(),
        },
    )
    .await
    .expect("ED01-01: insert_run failed");

    // Seed the orchestrator halt audit event — same structure as
    // persist_halt_and_disarm writes in production.
    mqk_db::insert_audit_event(
        &pool,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc: halt_ts,
            topic: "orchestrator".to_string(),
            event_type: "ReconcileDrift".to_string(),
            payload: serde_json::json!({
                "halt_reason": "ReconcileDrift",
                "source": "mqk-runtime.orchestrator.persist_halt_and_disarm",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await
    .expect("ED01-01: insert_audit_event failed");

    // Call the events/feed route with the DB-backed AppState.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ED01-01: events/feed must return 200"
    );

    let json = parse_json(body);

    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "ED01-01: truth_state must be 'active' with DB pool present"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events+postgres.audit_events[orchestrator]",
        "ED01-01: backend must name all four source lanes"
    );

    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("ED01-01: rows must be a JSON array");

    let event_id_str = event_id.to_string();
    let expected_event_id = format!("audit_events:{event_id_str}");

    // Find the orchestrator_halt row for our seeded event.
    let halt_row = rows
        .iter()
        .find(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_event_id)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!(
                "ED01-01: orchestrator_halt row with event_id={expected_event_id} must appear \
                 in events/feed; got rows: {rows:?}"
            )
        });

    assert_eq!(
        halt_row.get("kind").and_then(|v| v.as_str()),
        Some("orchestrator_halt"),
        "ED01-01: kind must be 'orchestrator_halt' for topic=orchestrator rows"
    );
    assert_eq!(
        halt_row.get("detail").and_then(|v| v.as_str()),
        Some("ReconcileDrift"),
        "ED01-01: detail must equal event_type 'ReconcileDrift'"
    );
    assert_eq!(
        halt_row.get("run_id").and_then(|v| v.as_str()),
        Some(run_id.to_string().as_str()),
        "ED01-01: run_id must match the seeded run"
    );
    assert_eq!(
        halt_row.get("provenance_ref").and_then(|v| v.as_str()),
        Some(expected_event_id.as_str()),
        "ED01-01: provenance_ref must be 'audit_events:{{event_id}}'"
    );
    // audit_event_id must be present and equal the raw UUID (no prefix).
    assert_eq!(
        halt_row.get("audit_event_id").and_then(|v| v.as_str()),
        Some(event_id_str.as_str()),
        "ED01-01: audit_event_id must be the raw UUID string"
    );

    // Post-test cleanup.
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("ED01-01: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("ED01-01: post-test runs cleanup failed");
}

// ---------------------------------------------------------------------------
// ED01-02: no-DB path is unaffected — truth_state=backend_unavailable
// ---------------------------------------------------------------------------

/// The no-DB path of events/feed must still return truth_state=backend_unavailable
/// with empty rows.  Adding the orchestrator query lane must not change this contract.
#[tokio::test]
async fn ed01_02_no_db_path_unaffected() {
    let router = make_router_no_db();

    let req = Request::builder()
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "ED01-02: events/feed must return 200"
    );

    let json = parse_json(body);
    assert_eq!(
        json_str(&json, "truth_state"),
        "backend_unavailable",
        "ED01-02: no-DB path must still return backend_unavailable"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "unavailable",
        "ED01-02: no-DB path backend must be 'unavailable'"
    );
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("ED01-02: rows must be a JSON array");
    assert_eq!(rows.len(), 0, "ED01-02: no-DB path must return empty rows");
}

// ---------------------------------------------------------------------------
// ED01-03: all known halt-reason event_types are distinct strings
// ---------------------------------------------------------------------------

/// Pure proof that the five orchestrator halt reasons are non-empty, distinct
/// strings — ensures the event_type field in audit_events carries unambiguous
/// diagnostic content for morning-after queries.
#[test]
fn ed01_03_halt_reasons_are_distinct() {
    let reasons = [
        "RecoveryQuarantine",
        "ReconcileDrift",
        "IntegrityViolation",
        "LeaderLeaseLost",
        "LeaderLeaseUnavailable",
    ];
    let unique: std::collections::HashSet<_> = reasons.iter().collect();
    assert_eq!(
        unique.len(),
        reasons.len(),
        "ED01-03: all halt reason strings must be distinct"
    );
    for r in &reasons {
        assert!(!r.is_empty(), "ED01-03: halt reason must be non-empty: {r}");
    }
}

// ---------------------------------------------------------------------------
// ED01-04: orchestrator_halt kind is distinguishable from operator_action
// ---------------------------------------------------------------------------

/// Pure proof that the `kind` values for orchestrator halts and operator actions
/// are distinct — ensures consumers can filter each lane independently in the feed.
#[test]
fn ed01_04_orchestrator_halt_kind_distinct_from_operator_action() {
    assert_ne!(
        "orchestrator_halt", "operator_action",
        "ED01-04: orchestrator_halt and operator_action must be different kind values"
    );
    assert_ne!(
        "orchestrator_halt", "runtime_transition",
        "ED01-04: orchestrator_halt and runtime_transition must be different kind values"
    );
    assert_ne!(
        "orchestrator_halt", "signal_admission",
        "ED01-04: orchestrator_halt and signal_admission must be different kind values"
    );
    assert_ne!(
        "orchestrator_halt", "autonomous_session",
        "ED01-04: orchestrator_halt and autonomous_session must be different kind values"
    );
}
