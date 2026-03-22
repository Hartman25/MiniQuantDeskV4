//! IR-02: Operator-action durable audit truth — DB-backed proof.
//!
//! Proves that:
//! A. /api/v1/audit/operator-actions returns real DB-backed row-level truth with
//!    correct field mappings (audit_event_id, ts_utc, requested_action, disposition,
//!    run_id, runtime_transition, provenance_ref).
//! B. /api/v1/audit/artifacts returns real run rows from postgres.runs with correct
//!    field mappings (artifact_id, artifact_type, run_id, created_at_utc, provenance_ref).
//! C. /api/v1/ops/operator-timeline returns combined rows from postgres.runs +
//!    postgres.audit_events with correct kind/detail/provenance_ref semantics.
//! D. arm/disarm actions are durable via sys_arm_state, not audit_events — the
//!    honest boundary is declared in the response and confirmed here.
//! E. change-system-mode (intentionally not_authoritative) writes no durable
//!    success row to audit_events.
//! F. (IR-02-06) Accepted authoritative action (POST /control/arm) creates a durable
//!    audit_events row end-to-end: the audit_event_id returned in the response is
//!    visible through /api/v1/audit/operator-actions AND /api/v1/ops/operator-timeline
//!    without any fixture insertion standing in for the audit write.
//!
//! All tests require MQK_DATABASE_URL.
//! Run: cargo test -p mqk-daemon --test scenario_operator_audit_ir02 -- --include-ignored

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
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
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

async fn ir02_test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_operator_audit_ir02 -- --include-ignored"
        )
    });
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("ir02_test_pool: connect failed")
}

// ---------------------------------------------------------------------------
// IR-02-01: operator-actions row-level field contract
//
// Proves: after inserting a run row and an audit_event row (topic='operator',
// event_type='run.start') directly into the DB, the
// GET /api/v1/audit/operator-actions route returns:
//   - truth_state="active", backend="postgres.audit_events"
//   - the row with correct field mappings that the GUI/doc contract depends on
//
// This is a direct DB injection test — it does not start a live daemon run.
// It proves the read-path contracts independently of the write-path.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_01_operator_actions_row_field_contract() {
    let pool = ir02_test_pool().await;

    let run_id = uuid::Uuid::parse_str("cc000001-0000-4000-8000-000000000001").unwrap();
    let event_id = uuid::Uuid::parse_str("cc000001-0000-4000-8000-000000000002").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let event_ts = chrono::DateTime::parse_from_rfc3339("2020-01-01T12:01:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup (in case a prior run failed and left rows).
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("pre-test runs cleanup failed");

    // Insert a run row (required by audit_events FK).
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "ir02-test-hash".to_string(),
            config_hash: "ir02-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ir02-test-host".to_string(),
        },
    )
    .await
    .expect("insert_run failed");

    // Insert an audit_event row (topic='operator', event_type='run.start').
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
    .expect("insert_audit_event failed");

    // Build AppState with the DB pool.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-02-01: operator-actions must return 200"
    );
    let json = parse_json(body);

    // Wrapper identity.
    assert_eq!(
        json["canonical_route"].as_str(),
        Some("/api/v1/audit/operator-actions"),
        "IR-02-01: canonical_route must self-identify; got: {json}"
    );
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active"),
        "IR-02-01: truth_state must be active when DB pool is present; got: {json}"
    );
    assert_eq!(
        json["backend"].as_str(),
        Some("postgres.audit_events"),
        "IR-02-01: backend must be postgres.audit_events; got: {json}"
    );

    // Find the inserted row by audit_event_id.
    let rows = json["rows"]
        .as_array()
        .expect("IR-02-01: rows must be an array");
    let event_id_str = event_id.to_string();
    let row = rows
        .iter()
        .find(|r| r["audit_event_id"].as_str() == Some(&event_id_str))
        .unwrap_or_else(|| {
            panic!(
                "IR-02-01: inserted audit_event row must appear in route response; \
                 event_id={event_id_str}; got rows: {rows:?}"
            )
        });

    // Field contract: audit_event_id maps to event_id.
    assert_eq!(
        row["audit_event_id"].as_str(),
        Some(event_id_str.as_str()),
        "IR-02-01: audit_event_id must match inserted event_id; got: {row}"
    );

    // Field contract: ts_utc is a non-empty RFC3339 string.
    assert!(
        row["ts_utc"].as_str().is_some_and(|s| !s.is_empty()),
        "IR-02-01: ts_utc must be a non-empty RFC3339 string; got: {row}"
    );

    // Field contract: requested_action maps to event_type.
    assert_eq!(
        row["requested_action"].as_str(),
        Some("run.start"),
        "IR-02-01: requested_action must equal event_type 'run.start'; got: {row}"
    );

    // Field contract: disposition is always "applied" for audit_events rows.
    assert_eq!(
        row["disposition"].as_str(),
        Some("applied"),
        "IR-02-01: disposition must be 'applied' for operator audit rows; got: {row}"
    );

    // Field contract: run_id is present and matches the inserted run.
    let run_id_str = run_id.to_string();
    assert_eq!(
        row["run_id"].as_str(),
        Some(run_id_str.as_str()),
        "IR-02-01: run_id must match the inserted run row; got: {row}"
    );

    // Field contract: runtime_transition maps 'run.start' → 'RUNNING'.
    assert_eq!(
        row["runtime_transition"].as_str(),
        Some("RUNNING"),
        "IR-02-01: runtime_transition must be RUNNING for run.start event; got: {row}"
    );

    // Field contract: provenance_ref is "audit_events:{event_id}".
    let expected_pref = format!("audit_events:{event_id_str}");
    assert_eq!(
        row["provenance_ref"].as_str(),
        Some(expected_pref.as_str()),
        "IR-02-01: provenance_ref must be 'audit_events:{{event_id}}'; got: {row}"
    );

    // Cleanup.
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("IR-02-01: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-01: post-test runs cleanup failed");
}

// ---------------------------------------------------------------------------
// IR-02-02: audit-artifacts row-level field contract
//
// Proves: after inserting a run row directly into the DB, the
// GET /api/v1/audit/artifacts route returns:
//   - truth_state="active", backend="postgres.runs"
//   - the artifact row with correct field mappings
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_02_audit_artifacts_row_field_contract() {
    let pool = ir02_test_pool().await;

    let run_id = uuid::Uuid::parse_str("cc000002-0000-4000-8000-000000000001").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-02T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup.
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("pre-test runs cleanup failed");

    // Insert a run row.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "ir02-test-hash".to_string(),
            config_hash: "ir02-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ir02-test-host".to_string(),
        },
    )
    .await
    .expect("insert_run failed");

    // Build AppState with pool.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/artifacts")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-02-02: audit-artifacts must return 200"
    );
    let json = parse_json(body);

    // Wrapper identity.
    assert_eq!(
        json["canonical_route"].as_str(),
        Some("/api/v1/audit/artifacts"),
        "IR-02-02: canonical_route must self-identify; got: {json}"
    );
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active"),
        "IR-02-02: truth_state must be active when DB pool is present; got: {json}"
    );
    assert_eq!(
        json["backend"].as_str(),
        Some("postgres.runs"),
        "IR-02-02: backend must be postgres.runs; got: {json}"
    );

    // Find artifact row by run_id.
    let rows = json["rows"]
        .as_array()
        .expect("IR-02-02: rows must be an array");
    let run_id_str = run_id.to_string();
    let row = rows
        .iter()
        .find(|r| r["run_id"].as_str() == Some(&run_id_str))
        .unwrap_or_else(|| {
            panic!(
                "IR-02-02: inserted run row must appear as artifact; \
                 run_id={run_id_str}; got rows: {rows:?}"
            )
        });

    // Field contract: artifact_id is "run-config:{run_id}".
    let expected_artifact_id = format!("run-config:{run_id_str}");
    assert_eq!(
        row["artifact_id"].as_str(),
        Some(expected_artifact_id.as_str()),
        "IR-02-02: artifact_id must be 'run-config:{{run_id}}'; got: {row}"
    );

    // Field contract: artifact_type is "run_config".
    assert_eq!(
        row["artifact_type"].as_str(),
        Some("run_config"),
        "IR-02-02: artifact_type must be 'run_config'; got: {row}"
    );

    // Field contract: run_id matches.
    assert_eq!(
        row["run_id"].as_str(),
        Some(run_id_str.as_str()),
        "IR-02-02: run_id must match the inserted run; got: {row}"
    );

    // Field contract: created_at_utc is a non-empty RFC3339 string.
    assert!(
        row["created_at_utc"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "IR-02-02: created_at_utc must be a non-empty RFC3339 string; got: {row}"
    );

    // Field contract: provenance_ref is "runs:{run_id}".
    let expected_pref = format!("runs:{run_id_str}");
    assert_eq!(
        row["provenance_ref"].as_str(),
        Some(expected_pref.as_str()),
        "IR-02-02: provenance_ref must be 'runs:{{run_id}}'; got: {row}"
    );

    // Cleanup.
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-02: post-test runs cleanup failed");
}

// ---------------------------------------------------------------------------
// IR-02-03: ops/operator-timeline row-level field contract
//
// Proves: after inserting a run row and an audit_event row, the
// GET /api/v1/ops/operator-timeline route returns:
//   - truth_state="active", backend="postgres.runs+postgres.audit_events"
//   - a runtime_transition row (kind="runtime_transition", detail="CREATED")
//     derived from the runs.started_at_utc column
//   - an operator_action row (kind="operator_action", detail="run.start")
//     derived from audit_events
// This proves the combined timeline query and its two source backends.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_03_operator_timeline_row_field_contract() {
    let pool = ir02_test_pool().await;

    let run_id = uuid::Uuid::parse_str("cc000003-0000-4000-8000-000000000001").unwrap();
    let event_id = uuid::Uuid::parse_str("cc000003-0000-4000-8000-000000000002").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-03T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let event_ts = chrono::DateTime::parse_from_rfc3339("2020-01-03T12:01:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup.
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("pre-test runs cleanup failed");

    // Insert run + audit_event.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "ir02-test-hash".to_string(),
            config_hash: "ir02-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ir02-test-host".to_string(),
        },
    )
    .await
    .expect("insert_run failed");

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
    .expect("insert_audit_event failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/operator-timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-02-03: operator-timeline must return 200"
    );
    let json = parse_json(body);

    // Wrapper identity.
    assert_eq!(
        json["canonical_route"].as_str(),
        Some("/api/v1/ops/operator-timeline"),
        "IR-02-03: canonical_route must self-identify; got: {json}"
    );
    assert_eq!(
        json["truth_state"].as_str(),
        Some("active"),
        "IR-02-03: truth_state must be active when DB pool is present; got: {json}"
    );
    assert_eq!(
        json["backend"].as_str(),
        Some("postgres.runs+postgres.audit_events"),
        "IR-02-03: backend must be postgres.runs+postgres.audit_events; got: {json}"
    );

    let rows = json["rows"]
        .as_array()
        .expect("IR-02-03: rows must be an array");
    let run_id_str = run_id.to_string();
    let event_id_str = event_id.to_string();

    // Find the CREATED runtime_transition row for this run.
    let created_row = rows.iter().find(|r| {
        r["kind"].as_str() == Some("runtime_transition")
            && r["detail"].as_str() == Some("CREATED")
            && r["run_id"].as_str() == Some(&run_id_str)
    });
    assert!(
        created_row.is_some(),
        "IR-02-03: must find a runtime_transition CREATED row for run {run_id_str}; \
         got rows: {rows:?}"
    );
    let created_row = created_row.unwrap();

    // Field contract: ts_utc is non-empty RFC3339.
    assert!(
        created_row["ts_utc"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "IR-02-03: runtime_transition CREATED row ts_utc must be non-empty; got: {created_row}"
    );

    // Field contract: provenance_ref for CREATED references runs table.
    let pref = created_row["provenance_ref"].as_str().unwrap_or("");
    assert!(
        pref.starts_with(&format!("runs:{run_id_str}")),
        "IR-02-03: CREATED provenance_ref must start with 'runs:{{run_id}}'; \
         got: '{pref}' in {created_row}"
    );

    // Find the operator_action row for the run.start audit event.
    let action_row = rows.iter().find(|r| {
        r["kind"].as_str() == Some("operator_action")
            && r["detail"].as_str() == Some("run.start")
            && r["provenance_ref"].as_str() == Some(&format!("audit_events:{event_id_str}"))
    });
    assert!(
        action_row.is_some(),
        "IR-02-03: must find an operator_action row for event {event_id_str}; \
         got rows: {rows:?}"
    );
    let action_row = action_row.unwrap();

    // Field contract: ts_utc is non-empty RFC3339.
    assert!(
        action_row["ts_utc"].as_str().is_some_and(|s| !s.is_empty()),
        "IR-02-03: operator_action row ts_utc must be non-empty; got: {action_row}"
    );

    // Field contract: provenance_ref is "audit_events:{event_id}".
    assert_eq!(
        action_row["provenance_ref"].as_str(),
        Some(format!("audit_events:{event_id_str}").as_str()),
        "IR-02-03: operator_action provenance_ref must be 'audit_events:{{event_id}}'; \
         got: {action_row}"
    );

    // Cleanup.
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("IR-02-03: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-03: post-test runs cleanup failed");
}

// ---------------------------------------------------------------------------
// IR-02-04: arm-execution with DB pool is durable via sys_arm_state, not
//           audit_events — the honest durable boundary.
//
// Proves:
//   1. POST /api/v1/ops/action arm-execution returns accepted=true, applied.
//   2. Response declares durable_db_write=true, durable_targets=["sys_arm_state"],
//      audit_event_id=null — the response is honest about WHERE the write went.
//   3. The audit_events table row count for topic='operator' does NOT increase
//      after an arm action (arm writes to sys_arm_state, not audit_events).
//
// This is the honest durable-boundary proof: arm/disarm are durable but
// not through audit_events — the sys_arm_state table is the durable store.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_04_arm_execution_durable_target_is_sys_arm_state_not_audit_events() {
    let pool = ir02_test_pool().await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Capture a timestamp just before the action so the count check is scoped to
    // rows written *after* this point.  This is robust against other tests running
    // concurrently against the same DB.
    let ts_before = chrono::Utc::now() - chrono::TimeDelta::milliseconds(1);

    let body = serde_json::json!({"action_key": "arm-execution"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, resp_body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-02-04: arm-execution must return 200"
    );
    let json = parse_json(resp_body);

    // Response must confirm accepted/applied.
    assert_eq!(
        json["accepted"], true,
        "IR-02-04: arm-execution must be accepted; got: {json}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("applied"),
        "IR-02-04: arm-execution disposition must be 'applied'; got: {json}"
    );

    // Audit fields: DB was present so durable_db_write must be true.
    assert_eq!(
        json["audit"]["durable_db_write"], true,
        "IR-02-04: durable_db_write must be true when DB pool is present; got: {json}"
    );

    // Durable target is sys_arm_state, NOT audit_events.
    let targets = json["audit"]["durable_targets"]
        .as_array()
        .expect("IR-02-04: durable_targets must be an array");
    let target_strs: Vec<&str> = targets.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        target_strs.contains(&"sys_arm_state"),
        "IR-02-04: durable_targets must contain sys_arm_state; got: {target_strs:?}"
    );
    assert!(
        !target_strs.contains(&"audit_events"),
        "IR-02-04: durable_targets must NOT contain audit_events for arm action; \
         got: {target_strs:?}"
    );

    // audit_event_id is None — arm actions do not emit an audit_events row.
    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "IR-02-04: audit_event_id must be null for arm-execution; got: {json}"
    );

    // Confirm: no new operator audit_events rows were written after ts_before.
    // The timestamp scope makes this robust against concurrent tests writing rows.
    let new_count: i64 = sqlx::query_scalar(
        "select count(*) from audit_events where topic = 'operator' and ts_utc > $1",
    )
    .bind(ts_before)
    .fetch_one(&pool)
    .await
    .expect("IR-02-04: failed to count new audit_events after arm");

    assert_eq!(
        new_count, 0,
        "IR-02-04: arm-execution must NOT write to audit_events table; \
         found {new_count} new rows after action"
    );
}

// ---------------------------------------------------------------------------
// IR-02-05: change-system-mode (not_authoritative) writes no durable success
//           row to audit_events.
//
// Proves:
//   1. POST /api/v1/ops/action change-system-mode returns 409 CONFLICT.
//   2. The response is ModeChangeGuidanceResponse — no accepted=true, no
//      durable_db_write=true claim.
//   3. The audit_events table row count for topic='operator' does NOT increase
//      after a change-system-mode call (no success row written).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_05_change_system_mode_writes_no_durable_success_row() {
    let pool = ir02_test_pool().await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Timestamp before the action; the count check is scoped to rows written after
    // this point so concurrent tests cannot cause false failures.
    let ts_before = chrono::Utc::now() - chrono::TimeDelta::milliseconds(1);

    let body = serde_json::json!({"action_key": "change-system-mode"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, resp_body) = call(routes::build_router(Arc::clone(&st)), req).await;

    // Must return 409 — intentionally not authoritative.
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "IR-02-05: change-system-mode must return 409 CONFLICT"
    );
    let json = parse_json(resp_body);

    // Response is ModeChangeGuidanceResponse: transition_permitted=false.
    assert_eq!(
        json["transition_permitted"], false,
        "IR-02-05: transition_permitted must be false; got: {json}"
    );

    // No accepted=true claim in the response (ModeChangeGuidanceResponse has
    // no accepted field at all — it is not an OperatorActionResponse).
    assert!(
        json.get("accepted").is_none() || json["accepted"] != true,
        "IR-02-05: response must not claim accepted=true for not_authoritative action; \
         got: {json}"
    );

    // No durable_db_write=true claim in the response.
    assert!(
        json.get("durable_db_write").is_none()
            || json["durable_db_write"] != true
            || json.get("audit").and_then(|a| a.get("durable_db_write"))
                != Some(&serde_json::json!(true)),
        "IR-02-05: response must not claim durable_db_write=true for not_authoritative action; \
         got: {json}"
    );

    // Confirm: no new operator audit_events rows were written after ts_before.
    let new_count: i64 = sqlx::query_scalar(
        "select count(*) from audit_events where topic = 'operator' and ts_utc > $1",
    )
    .bind(ts_before)
    .fetch_one(&pool)
    .await
    .expect("IR-02-05: failed to count new audit_events after change-system-mode");

    assert_eq!(
        new_count, 0,
        "IR-02-05: change-system-mode must NOT write to audit_events table; \
         found {new_count} new rows after action"
    );
}

// ---------------------------------------------------------------------------
// IR-02-06: POST /control/arm (accepted authoritative action) creates a durable
//           audit_events row end-to-end; that row is visible through the history
//           endpoints without any fixture insertion standing in for the audit write.
//
// This is the end-to-end write-path proof that IR-02 requires.
//
// The test inserts a run row as a FK anchor so write_control_operator_audit_event
// can resolve a run_id via fetch_latest_run_for_engine. The audit write itself is
// done entirely by the /control/arm route action — no audit_events fixture is used.
//
// Proves:
//   1. POST /control/arm returns accepted=true, durable_db_write=true,
//      audit_event_id non-null (the route wrote a real durable row).
//   2. GET /api/v1/audit/operator-actions returns that exact row by audit_event_id
//      with correct field mappings (requested_action, disposition, provenance_ref).
//   3. GET /api/v1/ops/operator-timeline returns the same row as kind="operator_action"
//      with correct detail and provenance_ref.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir02_06_control_arm_accepted_action_durable_row_visible_in_history() {
    let pool = ir02_test_pool().await;

    // A run row with engine_id='mqk-daemon' is needed so that
    // write_control_operator_audit_event can resolve a run_id anchor via
    // fetch_latest_run_for_engine(db, "mqk-daemon", "PAPER").
    // This run is the FK prerequisite only; the audit write happens through the route.
    let run_id = uuid::Uuid::parse_str("cc000006-0000-4000-8000-000000000001").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-06T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup: remove any audit_events and run row from a prior failed run.
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-06: pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-06: pre-test runs cleanup failed");

    // Insert the FK anchor run.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "ir02-06-test-hash".to_string(),
            config_hash: "ir02-06-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ir02-06-test-host".to_string(),
        },
    )
    .await
    .expect("IR-02-06: insert_run failed");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Step 1: POST /control/arm — accepted authoritative action.
    // The route calls write_control_operator_audit_event which finds the run we just
    // inserted and writes a real audit_events row, then returns audit_event_id.
    let req = Request::builder()
        .method("POST")
        .uri("/control/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "IR-02-06: /control/arm must return 200; got body: {}",
        String::from_utf8_lossy(&body)
    );
    let arm_json = parse_json(body);

    // Response must declare accepted/applied.
    assert_eq!(
        arm_json["accepted"], true,
        "IR-02-06: arm must be accepted; got: {arm_json}"
    );
    assert_eq!(
        arm_json["disposition"].as_str(),
        Some("applied"),
        "IR-02-06: disposition must be 'applied'; got: {arm_json}"
    );

    // audit_event_id must be non-null — the route wrote a real durable row.
    let event_id_str = arm_json["audit"]["audit_event_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "IR-02-06: audit_event_id must be non-null when a run anchor exists; \
                 got: {arm_json}"
            )
        });

    // durable_db_write must be true.
    assert_eq!(
        arm_json["audit"]["durable_db_write"], true,
        "IR-02-06: durable_db_write must be true; got: {arm_json}"
    );

    // Step 2: GET /api/v1/audit/operator-actions.
    // Find the row by the audit_event_id that the route returned.
    // No fixture insertion was used for the audit write — this is read-back from the
    // row the action itself wrote.
    let req2 = Request::builder()
        .method("GET")
        .uri("/api/v1/audit/operator-actions")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(routes::build_router(Arc::clone(&st)), req2).await;
    assert_eq!(
        status2,
        StatusCode::OK,
        "IR-02-06: audit/operator-actions must return 200"
    );
    let actions_json = parse_json(body2);

    assert_eq!(
        actions_json["truth_state"].as_str(),
        Some("active"),
        "IR-02-06: audit/operator-actions truth_state must be active; got: {actions_json}"
    );

    let rows = actions_json["rows"]
        .as_array()
        .expect("IR-02-06: rows must be array");
    let action_row = rows
        .iter()
        .find(|r| r["audit_event_id"].as_str() == Some(event_id_str))
        .unwrap_or_else(|| {
            panic!(
                "IR-02-06: audit_event_id {event_id_str} returned by /control/arm must be \
                 visible in /api/v1/audit/operator-actions; got rows: {rows:?}"
            )
        });

    // Field contract for the action-written row.
    assert_eq!(
        action_row["requested_action"].as_str(),
        Some("control.arm"),
        "IR-02-06: requested_action must be 'control.arm'; got: {action_row}"
    );
    assert_eq!(
        action_row["disposition"].as_str(),
        Some("applied"),
        "IR-02-06: disposition must be 'applied'; got: {action_row}"
    );
    assert!(
        action_row["ts_utc"].as_str().is_some_and(|s| !s.is_empty()),
        "IR-02-06: ts_utc must be non-empty RFC3339; got: {action_row}"
    );
    let expected_pref = format!("audit_events:{event_id_str}");
    assert_eq!(
        action_row["provenance_ref"].as_str(),
        Some(expected_pref.as_str()),
        "IR-02-06: provenance_ref must be 'audit_events:{{event_id}}'; got: {action_row}"
    );

    // Step 3: GET /api/v1/ops/operator-timeline.
    // The same durable row must appear as kind="operator_action".
    let req3 = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/operator-timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status3, body3) = call(routes::build_router(Arc::clone(&st)), req3).await;
    assert_eq!(
        status3,
        StatusCode::OK,
        "IR-02-06: operator-timeline must return 200"
    );
    let timeline_json = parse_json(body3);

    assert_eq!(
        timeline_json["truth_state"].as_str(),
        Some("active"),
        "IR-02-06: operator-timeline truth_state must be active; got: {timeline_json}"
    );

    let tl_rows = timeline_json["rows"]
        .as_array()
        .expect("IR-02-06: timeline rows must be array");
    let tl_row = tl_rows
        .iter()
        .find(|r| {
            r["kind"].as_str() == Some("operator_action")
                && r["provenance_ref"].as_str() == Some(&expected_pref)
        })
        .unwrap_or_else(|| {
            panic!(
                "IR-02-06: operator_action row for event {event_id_str} written by \
                 /control/arm must be visible in /api/v1/ops/operator-timeline; \
                 got rows: {tl_rows:?}"
            )
        });

    // Field contract for the timeline row.
    assert_eq!(
        tl_row["detail"].as_str(),
        Some("control.arm"),
        "IR-02-06: timeline row detail must be 'control.arm'; got: {tl_row}"
    );
    assert!(
        tl_row["ts_utc"].as_str().is_some_and(|s| !s.is_empty()),
        "IR-02-06: timeline row ts_utc must be non-empty; got: {tl_row}"
    );

    // Cleanup.
    let returned_event_id = uuid::Uuid::parse_str(event_id_str)
        .expect("IR-02-06: audit_event_id from response must be a valid UUID");
    sqlx::query("delete from audit_events where event_id = $1")
        .bind(returned_event_id)
        .execute(&pool)
        .await
        .expect("IR-02-06: post-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("IR-02-06: post-test runs cleanup failed");
}
