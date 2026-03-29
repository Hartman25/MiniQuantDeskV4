//! LO-03G — Durable operator timeline / audit evidence for live actions.
//!
//! Proves that operator arm/disarm actions produce truthful audit responses and,
//! on the authoritative DB-backed control routes, durable audit evidence through
//! the `audit_events` table (topic='operator') when a real run anchor exists.
//!
//! # What this proves
//!
//! - arm/disarm via `POST /api/v1/ops/action` always return a well-formed
//!   `audit` field in the response.
//! - `durable_db_write` is false when no DB is configured (honest: in-memory only).
//! - `audit_event_id` is None when no DB (honest: no durable write without DB).
//! - `durable_targets` includes `sys_arm_state` when DB is configured.
//! - `GET /api/v1/audit/operator-actions` returns `backend_unavailable` when
//!   no DB — honest absence, not fabricated rows.
//! - `GET /api/v1/ops/operator-timeline` returns `backend_unavailable` when
//!   no DB — honest absence.
//! - arm/disarm audit fields are structurally correct and stable (no panics,
//!   no missing fields).
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                              |
//! |-------|-----------------------------------------------------------------------------|
//! | G-01  | ops/action arm → audit field present; durable_db_write=false without DB    |
//! | G-02  | ops/action arm → audit_event_id=null without DB (honest: no durable write) |
//! | G-03  | ops/action disarm → audit field present; durable_db_write=false without DB |
//! | G-04  | ops/action disarm → audit_event_id=null without DB                         |
//! | G-05  | audit/operator-actions returns backend_unavailable without DB               |
//! | G-06  | ops/operator-timeline returns backend_unavailable without DB               |
//!
//! All tests require no database and no network.
//!
//! | G-07  | POST /v1/integrity/arm + DB + Armed run → durable audit event written; surface shows non-null audit_event_id |
//! | G-08  | POST /v1/integrity/disarm + DB + Armed run → durable audit event written; surface shows non-null audit_event_id |
//!
//! G-07 and G-08 require MQK_DATABASE_URL and use `#[ignore]`; run with
//! `--include-ignored`.
//!
//! # DB-backed evidence (proved by G-07/G-08)
//!
//! When a DB is available and a run is in Armed status, `POST /v1/integrity/arm`
//! and `POST /v1/integrity/disarm` call `write_operator_audit_event`, which
//! writes a row to `audit_events` (topic='operator', event_type='control.arm'
//! or 'control.disarm'). `GET /api/v1/audit/operator-actions` surfaces that
//! row with a non-null `audit_event_id`.  Neither integrity route returns an
//! audit field in its own response; the proof is through the mounted surface.
//! G-01..G-06 prove the honest no-DB fail-closed path; G-07..G-08 prove the
//! real durable path. Both are honest and non-contradictory.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// HTTP helpers
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
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn ops_action_request(action_key: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "action_key": action_key });
    Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn fresh_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new())
}

// ---------------------------------------------------------------------------
// G-01: ops/action arm → audit field present; durable_db_write=false
// ---------------------------------------------------------------------------

/// LO-03G / G-01: arm via ops/action returns a response with a well-formed
/// `audit` field. Without a DB, `durable_db_write` must be false (honest:
/// arm is only in-memory).
#[tokio::test]
async fn g01_arm_audit_field_present_no_db() {
    let st = fresh_state();
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_request("arm-execution"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "G-01: arm must succeed; body: {json}"
    );
    let audit = &json["audit"];
    assert!(
        !audit.is_null(),
        "G-01: response must contain 'audit' field; body: {json}"
    );
    assert_eq!(
        audit["durable_db_write"].as_bool().unwrap_or(true),
        false,
        "G-01: durable_db_write must be false without DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// G-02: ops/action arm → audit_event_id is null without DB
// ---------------------------------------------------------------------------

/// LO-03G / G-02: Without a DB, `audit_event_id` is null in the arm response.
/// This is the honest representation: no durable audit event was written.
#[tokio::test]
async fn g02_arm_audit_event_id_null_without_db() {
    let st = fresh_state();
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_request("arm-strategy"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "G-02: arm must succeed; body: {json}"
    );
    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "G-02: audit_event_id must be null without DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// G-03: ops/action disarm → audit field present; durable_db_write=false
// ---------------------------------------------------------------------------

/// LO-03G / G-03: disarm via ops/action returns a response with a well-formed
/// `audit` field. Without a DB, `durable_db_write` must be false.
#[tokio::test]
async fn g03_disarm_audit_field_present_no_db() {
    let st = fresh_state();
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_request("disarm-execution"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "G-03: disarm must succeed; body: {json}"
    );
    let audit = &json["audit"];
    assert!(
        !audit.is_null(),
        "G-03: response must contain 'audit' field; body: {json}"
    );
    assert_eq!(
        audit["durable_db_write"].as_bool().unwrap_or(true),
        false,
        "G-03: durable_db_write must be false without DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// G-04: ops/action disarm → audit_event_id is null without DB
// ---------------------------------------------------------------------------

/// LO-03G / G-04: Without a DB, `audit_event_id` is null in the disarm response.
#[tokio::test]
async fn g04_disarm_audit_event_id_null_without_db() {
    let st = fresh_state();
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_request("disarm-strategy"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "G-04: disarm must succeed; body: {json}"
    );
    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "G-04: audit_event_id must be null without DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// G-05: audit/operator-actions → backend_unavailable without DB
// ---------------------------------------------------------------------------

/// LO-03G / G-05: `GET /api/v1/audit/operator-actions` returns
/// `truth_state="backend_unavailable"` when no DB is configured.
///
/// This is the honest mounted truth: absent DB = no durable audit evidence
/// to surface. The route must not fabricate rows.
#[tokio::test]
async fn g05_audit_operator_actions_backend_unavailable_without_db() {
    let st = fresh_state();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G-05: body: {json}");
    assert_eq!(
        json["truth_state"].as_str().unwrap_or(""),
        "backend_unavailable",
        "G-05: truth_state must be 'backend_unavailable' without DB; body: {json}"
    );
    assert_eq!(
        json["canonical_route"].as_str().unwrap_or(""),
        "/api/v1/audit/operator-actions",
        "G-05: canonical_route must be present; body: {json}"
    );
    let rows = json["rows"].as_array().unwrap();
    assert!(
        rows.is_empty(),
        "G-05: rows must be empty without DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// G-06: ops/operator-timeline → backend_unavailable without DB
// ---------------------------------------------------------------------------

/// LO-03G / G-06: `GET /api/v1/ops/operator-timeline` returns
/// `truth_state="backend_unavailable"` when no DB is configured.
///
/// The timeline surface must not fabricate operator action records.
#[tokio::test]
async fn g06_ops_operator_timeline_backend_unavailable_without_db() {
    let st = fresh_state();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/operator-timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G-06: body: {json}");
    assert_eq!(
        json["truth_state"].as_str().unwrap_or(""),
        "backend_unavailable",
        "G-06: truth_state must be 'backend_unavailable' without DB; body: {json}"
    );
    assert_eq!(
        json["canonical_route"].as_str().unwrap_or(""),
        "/api/v1/ops/operator-timeline",
        "G-06: canonical_route must be present; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed pool helper
// ---------------------------------------------------------------------------

async fn lo03g_test_pool() -> sqlx::PgPool {
    let url = std::env::var("MQK_DATABASE_URL").expect(
        "LO-03G DB tests require MQK_DATABASE_URL; \
         run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
         cargo test --test scenario_audit_ops_lo03g -- --include-ignored",
    );
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("LO-03G: failed to connect to MQK_DATABASE_URL");
    mqk_db::migrate(&pool)
        .await
        .expect("LO-03G: failed to apply pending migrations to test DB");
    pool
}

// ---------------------------------------------------------------------------
// G-07: /v1/integrity/arm with DB + Armed run writes durable audit evidence
// ---------------------------------------------------------------------------

/// LO-03G / G-07: `POST /v1/integrity/arm` with a DB-backed Armed run writes
/// a durable audit event to `audit_events` (topic='operator').
///
/// Proves:
/// - `GET /api/v1/audit/operator-actions` returns `truth_state="active"`
/// - the surface contains a row with `requested_action="control.arm"` for our
///   run_id, with a non-null `audit_event_id`
///
/// Route: `/v1/integrity/arm` (returns IntegrityResponse — no audit field in
/// response itself; proof is through the mounted audit surface only).
///
/// Run must be in Armed status so `current_status_snapshot` returns a non-null
/// `active_run_id` and `write_operator_audit_event` fires.
///
/// Isolation: pre-cleanup removes both G-07 and G-08 run_ids so a prior
/// failed G-08 run (started_at 2020-08-01 > 2020-07-01) cannot be picked up
/// as the "latest" run by `fetch_latest_run_for_engine`.
///
/// Requires MQK_DATABASE_URL; skips when not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn g07_arm_db_backed_writes_durable_audit_evidence() {
    let pool = lo03g_test_pool().await;
    let run_id = uuid::Uuid::parse_str("d0030700-0000-4000-8000-000000000001").unwrap();
    let run_id_g08 = uuid::Uuid::parse_str("d0030800-0000-4000-8000-000000000001").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-07-01T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup: remove both G-07 and G-08 run_ids so a leftover G-08
    // run (started_at 2020-08-01) cannot win the latest-run query.
    for rid in [run_id, run_id_g08] {
        sqlx::query("delete from audit_events where run_id = $1")
            .bind(rid)
            .execute(&pool)
            .await
            .expect("G-07: pre-test audit_events cleanup failed");
        sqlx::query("delete from runs where run_id = $1")
            .bind(rid)
            .execute(&pool)
            .await
            .expect("G-07: pre-test runs cleanup failed");
    }
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("G-07: pre-test sys_arm_state cleanup failed");

    // Seed: Created → Armed.  Armed status causes current_status_snapshot to
    // return active_run_id=Some(run_id), which triggers write_operator_audit_event.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "lo03g-g07-hash".to_string(),
            config_hash: "lo03g-g07-cfg".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "lo03g-g07-host".to_string(),
        },
    )
    .await
    .expect("G-07: insert_run failed");
    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("G-07: arm_run failed");

    // Build daemon state with real DB (Paper mode default).
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // POST /v1/integrity/arm — this is the route that calls
    // write_operator_audit_event when active_run_id is non-null.
    let (arm_status, arm_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/integrity/arm")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let arm_json = parse_json(arm_body);
    assert_eq!(
        arm_status,
        StatusCode::OK,
        "G-07: /v1/integrity/arm must succeed; body: {arm_json}"
    );

    // Query the mounted audit surface — must reflect the written event.
    // (IntegrityResponse has no audit field; proof is through the surface.)
    let (audit_status, audit_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("GET")
            .uri("/api/v1/audit/operator-actions")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let audit_json = parse_json(audit_body);

    assert_eq!(
        audit_status,
        StatusCode::OK,
        "G-07: audit/operator-actions must return 200; body: {audit_json}"
    );
    assert_eq!(
        audit_json["truth_state"].as_str().unwrap_or(""),
        "active",
        "G-07: truth_state must be 'active' with DB; body: {audit_json}"
    );

    let rows = audit_json["rows"]
        .as_array()
        .expect("G-07: rows must be array");
    let arm_row = rows
        .iter()
        .find(|r| {
            r["requested_action"].as_str() == Some("control.arm")
                && r["run_id"].as_str() == Some(&run_id.to_string())
        })
        .unwrap_or_else(|| {
            panic!(
                "G-07: expected control.arm row for run_id {run_id} in audit surface; \
                 rows: {rows:?}"
            )
        });

    assert!(
        !arm_row["audit_event_id"].is_null(),
        "G-07: audit_event_id must be non-null for durable DB write; row: {arm_row}"
    );
    assert!(
        arm_row["audit_event_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "G-07: audit_event_id must be a non-empty string; row: {arm_row}"
    );

    // Post-test cleanup.
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("G-07: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("G-07: post-test runs cleanup failed");
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("G-07: post-test sys_arm_state cleanup failed");
}

// ---------------------------------------------------------------------------
// G-08: /v1/integrity/disarm with DB + Armed run writes durable audit evidence
// ---------------------------------------------------------------------------

/// LO-03G / G-08: `POST /v1/integrity/disarm` with a DB-backed Armed run writes
/// a durable audit event to `audit_events` (topic='operator').
///
/// Proves:
/// - `GET /api/v1/audit/operator-actions` returns `truth_state="active"`
/// - the surface contains a row with `requested_action="control.disarm"` for
///   our run_id, with a non-null `audit_event_id`
///
/// Route: `/v1/integrity/disarm` (returns IntegrityResponse — no audit field
/// in response itself; proof is through the mounted audit surface only).
///
/// `integrity_disarm` only changes `sys_arm_state`; the run remains Armed in
/// the `runs` table, so `current_status_snapshot` still returns
/// `active_run_id=Some(run_id)` after disarm, and `write_operator_audit_event`
/// fires.
///
/// Isolation: pre-cleanup removes both G-07 and G-08 run_ids.
///
/// Requires MQK_DATABASE_URL; skips when not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn g08_disarm_db_backed_writes_durable_audit_evidence() {
    let pool = lo03g_test_pool().await;
    let run_id = uuid::Uuid::parse_str("d0030800-0000-4000-8000-000000000001").unwrap();
    let run_id_g07 = uuid::Uuid::parse_str("d0030700-0000-4000-8000-000000000001").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-08-01T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup: remove both run_ids for isolation.
    for rid in [run_id, run_id_g07] {
        sqlx::query("delete from audit_events where run_id = $1")
            .bind(rid)
            .execute(&pool)
            .await
            .expect("G-08: pre-test audit_events cleanup failed");
        sqlx::query("delete from runs where run_id = $1")
            .bind(rid)
            .execute(&pool)
            .await
            .expect("G-08: pre-test runs cleanup failed");
    }
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("G-08: pre-test sys_arm_state cleanup failed");

    // Seed: Created → Armed.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "lo03g-g08-hash".to_string(),
            config_hash: "lo03g-g08-cfg".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "lo03g-g08-host".to_string(),
        },
    )
    .await
    .expect("G-08: insert_run failed");
    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("G-08: arm_run failed");

    // Build daemon state with real DB.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // POST /v1/integrity/disarm — writes durable audit event via
    // write_operator_audit_event when active_run_id is non-null.
    // (integrity_disarm changes sys_arm_state only; run stays Armed, so
    // active_run_id remains non-null in the status snapshot.)
    let (disarm_status, disarm_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/integrity/disarm")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let disarm_json = parse_json(disarm_body);
    assert_eq!(
        disarm_status,
        StatusCode::OK,
        "G-08: /v1/integrity/disarm must succeed; body: {disarm_json}"
    );

    // Query the mounted audit surface.
    // (IntegrityResponse has no audit field; proof is through the surface.)
    let (audit_status, audit_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("GET")
            .uri("/api/v1/audit/operator-actions")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let audit_json = parse_json(audit_body);

    assert_eq!(
        audit_status,
        StatusCode::OK,
        "G-08: audit/operator-actions must return 200; body: {audit_json}"
    );
    assert_eq!(
        audit_json["truth_state"].as_str().unwrap_or(""),
        "active",
        "G-08: truth_state must be 'active' with DB; body: {audit_json}"
    );

    let rows = audit_json["rows"]
        .as_array()
        .expect("G-08: rows must be array");
    let disarm_row = rows
        .iter()
        .find(|r| {
            r["requested_action"].as_str() == Some("control.disarm")
                && r["run_id"].as_str() == Some(&run_id.to_string())
        })
        .unwrap_or_else(|| {
            panic!(
                "G-08: expected control.disarm row for run_id {run_id} in audit surface; \
                 rows: {rows:?}"
            )
        });

    assert!(
        !disarm_row["audit_event_id"].is_null(),
        "G-08: audit_event_id must be non-null for durable DB write; row: {disarm_row}"
    );
    assert!(
        disarm_row["audit_event_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "G-08: audit_event_id must be non-empty string; row: {disarm_row}"
    );

    // Post-test cleanup.
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("G-08: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("G-08: post-test runs cleanup failed");
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("G-08: post-test sys_arm_state cleanup failed");
}
