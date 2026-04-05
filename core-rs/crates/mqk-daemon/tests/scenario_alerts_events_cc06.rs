//! CC-06 — alerts/active and events/feed surface tests.
//!
//! Tests:
//!
//! - CC06-01: alerts/active clean state returns truth_state=active, empty rows,
//!   correct canonical_route and backend identity.
//! - CC06-02: alerts/active with dirty reconcile injected returns an alert row
//!   with the correct class, severity, summary, and source.
//! - CC06-03: events/feed with no DB returns truth_state=backend_unavailable,
//!   backend=unavailable, empty rows.
//! - CC06-04: events/feed canonical_route and backend identity (no-DB path).
//! - CC06-05: events/feed DB-backed positive-path proof — seeds a deterministic
//!   run row (runtime_transition lane) and a deterministic audit_event row
//!   (operator_action lane), calls GET /api/v1/events/feed, and validates exact
//!   field mappings for both returned rows plus newest-first ordering.
//!   Requires MQK_DATABASE_URL; skips when not set.

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

async fn cc06_test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_alerts_events_cc06 -- --include-ignored"
        )
    });
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("cc06_test_pool: connect failed")
}

// ---------------------------------------------------------------------------
// CC06-01: alerts/active clean state
// ---------------------------------------------------------------------------

/// alerts/active in a fresh daemon state (idle, clean reconcile defaults to
/// "unknown" which does NOT emit a fault signal unless running).
/// truth_state="active", rows=[], alert_count=0, canonical_route/backend correct.
#[tokio::test]
async fn cc06_01_alerts_active_clean_state_empty_rows() {
    let router = make_router();
    let req = Request::builder()
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK, "alerts/active must return 200");

    let json = parse_json(body);

    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "truth_state must be 'active' in clean state"
    );
    assert_eq!(
        json_str(&json, "canonical_route"),
        "/api/v1/alerts/active",
        "canonical_route must self-identify"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "daemon.runtime_state",
        "backend must be 'daemon.runtime_state'"
    );
    let alert_count = json
        .get("alert_count")
        .and_then(|v| v.as_u64())
        .expect("alert_count must be present and numeric");
    assert_eq!(alert_count, 0, "clean daemon state has no active alerts");

    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be a JSON array");
    assert_eq!(
        rows.len(),
        0,
        "rows must be empty in clean state — not a fabricated placeholder"
    );
    assert_eq!(
        alert_count as usize,
        rows.len(),
        "alert_count must equal rows.len()"
    );
}

// ---------------------------------------------------------------------------
// CC06-02: alerts/active with dirty reconcile injected
// ---------------------------------------------------------------------------

/// Inject a "dirty" reconcile state.  alerts/active must return an alert row
/// with class="reconcile.dispatch_block.dirty", severity="critical",
/// source="daemon.runtime_state".  This proves the real fault-signal source
/// (build_fault_signals) is wired into the route.
#[tokio::test]
async fn cc06_02_alerts_active_dirty_reconcile_emits_critical_alert() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Inject a dirty reconcile snapshot via the public publish API.
    // Without DB, publish_reconcile_snapshot writes to the in-memory lock.
    st.publish_reconcile_snapshot(state::ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-03-22T00:00:00Z".to_string()),
        snapshot_watermark_ms: None,
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("injected dirty reconcile for CC06-02".to_string()),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json_str(&json, "truth_state"), "active");

    let alert_count = json
        .get("alert_count")
        .and_then(|v| v.as_u64())
        .expect("alert_count must be present");
    assert!(
        alert_count >= 1,
        "dirty reconcile must produce at least one alert row"
    );

    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be a JSON array");
    assert_eq!(
        alert_count as usize,
        rows.len(),
        "alert_count must match rows.len()"
    );

    // Find the reconcile alert row.
    let reconcile_alert = rows
        .iter()
        .find(|r| {
            r.get("class")
                .and_then(|v| v.as_str())
                .map(|c| c.starts_with("reconcile.dispatch_block.dirty"))
                .unwrap_or(false)
        })
        .expect("must have a reconcile.dispatch_block.dirty alert row");

    assert_eq!(
        reconcile_alert
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "critical",
        "dirty reconcile alert must be severity=critical"
    );
    assert_eq!(
        reconcile_alert
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "daemon.runtime_state",
        "source must be daemon.runtime_state"
    );
    let alert_id = reconcile_alert
        .get("alert_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let class = reconcile_alert
        .get("class")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        alert_id, class,
        "alert_id must equal class (no separate lifecycle ID)"
    );

    let summary = reconcile_alert
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        !summary.is_empty(),
        "alert row must have a non-empty summary"
    );
}

// ---------------------------------------------------------------------------
// CC06-03: events/feed no-DB returns backend_unavailable
// ---------------------------------------------------------------------------

/// events/feed with no DB pool returns truth_state=backend_unavailable,
/// backend=unavailable, rows=[].  The empty rows must NOT be treated as
/// authoritative empty history.
#[tokio::test]
async fn cc06_03_events_feed_no_db_is_backend_unavailable() {
    let router = make_router(); // no DB pool in default AppState

    let req = Request::builder()
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "events/feed must return 200 even without DB"
    );

    let json = parse_json(body);
    assert_eq!(
        json_str(&json, "truth_state"),
        "backend_unavailable",
        "truth_state must be 'backend_unavailable' when no DB pool"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "unavailable",
        "backend must be 'unavailable' when no DB pool"
    );
    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("rows must be a JSON array");
    assert_eq!(rows.len(), 0, "rows must be empty when no DB");
}

// ---------------------------------------------------------------------------
// CC06-04: events/feed canonical_route and backend identity
// ---------------------------------------------------------------------------

/// events/feed must self-identify its canonical_route in all truth states.
#[tokio::test]
async fn cc06_04_events_feed_canonical_route_identity() {
    let router = make_router();

    let req = Request::builder()
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(
        json_str(&json, "canonical_route"),
        "/api/v1/events/feed",
        "canonical_route must self-identify as /api/v1/events/feed"
    );
    // truth_state must be one of the valid values (no-DB path in this test)
    let ts = json_str(&json, "truth_state");
    assert!(
        ts == "active" || ts == "backend_unavailable",
        "truth_state must be 'active' or 'backend_unavailable', got '{ts}'"
    );
    // backend must match truth_state
    let backend = json_str(&json, "backend");
    if ts == "backend_unavailable" {
        assert_eq!(backend, "unavailable");
    } else {
        assert_eq!(
            backend,
            "postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events"
        );
    }
    // rows must be a JSON array
    assert!(
        json.get("rows").and_then(|v| v.as_array()).is_some(),
        "rows must be a JSON array"
    );
}

// ---------------------------------------------------------------------------
// CC06-05: events/feed DB-backed positive-path proof (real durable rows)
// ---------------------------------------------------------------------------

/// events/feed positive-path DB-backed proof.
///
/// Seeds:
/// - A deterministic run row into `postgres.runs` (runtime_transition lane).
/// - A deterministic audit_event row into `postgres.audit_events` with
///   topic='operator' (operator_action lane).
///
/// The audit_event timestamp (T+60s) is later than the run timestamp (T),
/// so the operator_action row must sort before the runtime_transition row
/// in newest-first order.
///
/// Validates exact field mappings for both returned rows:
/// - event_id, ts_utc, kind, detail, run_id, provenance_ref.
///
/// Requires MQK_DATABASE_URL; skips when not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn cc06_05_events_feed_db_backed_positive_path_real_rows() {
    let pool = cc06_test_pool().await;

    // Deterministic IDs — unique namespace so parallel test suites do not collide.
    let run_id = uuid::Uuid::parse_str("cc060005-0000-4000-8000-000000000001").unwrap();
    let event_id = uuid::Uuid::parse_str("cc060005-0000-4000-8000-000000000002").unwrap();

    // run created at T; audit event at T+60s — audit event is newer, so it
    // must appear before the run row in newest-first feed order.
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-06-01T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let event_ts = chrono::DateTime::parse_from_rfc3339("2020-06-01T10:01:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup (idempotent — guard against prior failure).
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("CC06-05: pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("CC06-05: pre-test runs cleanup failed");

    // --- Seed the run row (runtime_transition source) ---
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "cc06-test-hash".to_string(),
            config_hash: "cc06-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "cc06-test-host".to_string(),
        },
    )
    .await
    .expect("CC06-05: insert_run failed");

    // --- Seed the audit_event row (operator_action source) ---
    mqk_db::insert_audit_event(
        &pool,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc: event_ts,
            topic: "operator".to_string(),
            event_type: "run.start".to_string(),
            payload: serde_json::json!({
                "runtime_transition": "RUNNING",
                "source": "mqk-daemon.routes"
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await
    .expect("CC06-05: insert_audit_event failed");

    // --- Call the route ---
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
        "CC06-05: events/feed must return 200"
    );

    let json = parse_json(body);

    // --- Wrapper identity ---
    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "CC06-05: truth_state must be 'active' when DB pool is present"
    );
    assert_eq!(
        json_str(&json, "backend"),
        "postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events",
        "CC06-05: backend must name all three source tables"
    );
    assert_eq!(
        json_str(&json, "canonical_route"),
        "/api/v1/events/feed",
        "CC06-05: canonical_route must self-identify"
    );

    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("CC06-05: rows must be a JSON array");

    // At minimum the two seeded rows must be present.
    assert!(
        rows.len() >= 2,
        "CC06-05: at least 2 rows must be present (seeded run + audit_event); got {}",
        rows.len()
    );
    assert!(
        rows.len() <= 50,
        "CC06-05: events/feed must return at most 50 rows"
    );

    // --- Validate the operator_action row (seeded audit_event) ---
    let run_id_str = run_id.to_string();
    let event_id_str = event_id.to_string();
    let expected_action_event_id = format!("audit_events:{event_id_str}");
    let expected_action_provenance = expected_action_event_id.clone();

    let action_row = rows
        .iter()
        .find(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_action_event_id)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!(
                "CC06-05: operator_action row with event_id={expected_action_event_id} \
                 must appear in events/feed response; got rows: {rows:?}"
            )
        });

    assert_eq!(
        action_row.get("kind").and_then(|v| v.as_str()),
        Some("operator_action"),
        "CC06-05: kind must be 'operator_action' for audit_events-sourced row; got: {action_row}"
    );
    assert_eq!(
        action_row.get("detail").and_then(|v| v.as_str()),
        Some("run.start"),
        "CC06-05: detail must equal event_type 'run.start'; got: {action_row}"
    );
    assert_eq!(
        action_row.get("run_id").and_then(|v| v.as_str()),
        Some(run_id_str.as_str()),
        "CC06-05: run_id must match the seeded run; got: {action_row}"
    );
    assert_eq!(
        action_row.get("provenance_ref").and_then(|v| v.as_str()),
        Some(expected_action_provenance.as_str()),
        "CC06-05: provenance_ref must be 'audit_events:{{event_id}}'; got: {action_row}"
    );
    // ts_utc must be a non-empty RFC 3339 string matching the seeded event_ts.
    let action_ts = action_row
        .get("ts_utc")
        .and_then(|v| v.as_str())
        .expect("CC06-05: ts_utc must be present on operator_action row");
    assert!(
        !action_ts.is_empty(),
        "CC06-05: ts_utc must be non-empty; got: {action_row}"
    );
    let expected_event_ts_rfc = event_ts.to_rfc3339();
    assert_eq!(
        action_ts, expected_event_ts_rfc,
        "CC06-05: ts_utc must exactly match the seeded event_ts ({expected_event_ts_rfc}); \
         got: {action_ts}"
    );

    // --- Validate the runtime_transition CREATED row (seeded run) ---
    let expected_created_event_id = format!("runs:{run_id_str}:started_at_utc");
    let expected_created_provenance = expected_created_event_id.clone();

    let created_row = rows
        .iter()
        .find(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_created_event_id)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!(
                "CC06-05: runtime_transition CREATED row with event_id={expected_created_event_id} \
                 must appear in events/feed response; got rows: {rows:?}"
            )
        });

    assert_eq!(
        created_row.get("kind").and_then(|v| v.as_str()),
        Some("runtime_transition"),
        "CC06-05: kind must be 'runtime_transition' for runs-sourced row; got: {created_row}"
    );
    assert_eq!(
        created_row.get("detail").and_then(|v| v.as_str()),
        Some("CREATED"),
        "CC06-05: detail must be 'CREATED' for started_at_utc transition; got: {created_row}"
    );
    assert_eq!(
        created_row.get("run_id").and_then(|v| v.as_str()),
        Some(run_id_str.as_str()),
        "CC06-05: run_id must match the seeded run_id; got: {created_row}"
    );
    assert_eq!(
        created_row.get("provenance_ref").and_then(|v| v.as_str()),
        Some(expected_created_provenance.as_str()),
        "CC06-05: provenance_ref must be 'runs:{{run_id}}:started_at_utc'; got: {created_row}"
    );
    let created_ts = created_row
        .get("ts_utc")
        .and_then(|v| v.as_str())
        .expect("CC06-05: ts_utc must be present on runtime_transition row");
    let expected_started_rfc = started_at.to_rfc3339();
    assert_eq!(
        created_ts, expected_started_rfc,
        "CC06-05: ts_utc must exactly match the seeded started_at ({expected_started_rfc}); \
         got: {created_ts}"
    );

    // --- Validate newest-first ordering ---
    // The audit_event (event_ts = 10:01) is newer than the run (started_at = 10:00).
    // In newest-first feed order, the operator_action row must appear before the
    // runtime_transition CREATED row.
    let action_pos = rows
        .iter()
        .position(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_action_event_id)
                .unwrap_or(false)
        })
        .expect("CC06-05: operator_action row must be in rows");
    let created_pos = rows
        .iter()
        .position(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_created_event_id)
                .unwrap_or(false)
        })
        .expect("CC06-05: runtime_transition CREATED row must be in rows");
    assert!(
        action_pos < created_pos,
        "CC06-05: operator_action row (pos={action_pos}, ts={event_ts}) must appear before \
         runtime_transition CREATED row (pos={created_pos}, ts={started_at}) in newest-first order"
    );

    // --- Post-test cleanup ---
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("CC06-05: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("CC06-05: post-test runs cleanup failed");
}
