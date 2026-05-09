//! END-TO-END-HALTED-FILL-REPAIR-WORKFLOW-01 proof tests.
//!
//! Simulates the complete operator repair sequence for a halted run with
//! cursor-only fill evidence (no unapplied inbox row, but broker cursor confirms
//! a fill event was received).
//!
//! ## Workflow under proof
//!
//! ```text
//! cursor-only evidence
//!   → REST recovery dry-run      (plan only — no mutation)
//!   → REST recovery refused      (dry_run=false, no confirmation)
//!   → REST recovery confirmed    (inbox row inserted + applied)
//!   → REST recovery idempotent   (already_repaired — no duplicate)
//!   → portfolio snapshot dry-run (computes from applied inbox, no write)
//!   → snapshot refused           (dry_run=false, no confirmation)
//!   → snapshot confirmed         (audit event written)
//!   → snapshot idempotent        (already_current — no duplicate)
//!   → final DB invariant check
//! ```
//!
//! | Test | Claim                                                                                  |
//! |------|----------------------------------------------------------------------------------------|
//! | W01  | No DB → fail-closed (503/no_db) for REST recovery; 400 for snapshot (Gate 1 first)    |
//! | W02–W12 | DB-backed sequential workflow; all steps run, zero skips with MQK_DATABASE_URL   |
//!
//! W01 is pure in-process (no DB required).
//! W02–W12 require MQK_DATABASE_URL and are run as a single sequential test function.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaOrderActivity;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fake fill activity fetcher (no real Alpaca network calls)
// ---------------------------------------------------------------------------

struct FakeFillFetcher {
    activities: Vec<AlpacaOrderActivity>,
}

impl FakeFillFetcher {
    fn returning(activities: Vec<AlpacaOrderActivity>) -> Arc<Self> {
        Arc::new(Self { activities })
    }
}

impl state::BrokerFillActivityFetcher for FakeFillFetcher {
    fn fetch_fill_activities_for_order(
        &self,
        _broker_order_id: &str,
    ) -> Result<Vec<AlpacaOrderActivity>, String> {
        Ok(self.activities.clone())
    }
}

fn make_fill_activity(activity_id: &str, order_id: &str) -> AlpacaOrderActivity {
    AlpacaOrderActivity {
        id: activity_id.to_string(),
        activity_type: "FILL".to_string(),
        order_id: order_id.to_string(),
        transaction_time: "2026-05-08T14:30:00.000000000Z".to_string(),
        price: Some("152.75".to_string()),
        qty: Some("5".to_string()),
        side: "buy".to_string(),
        symbol: "NVDA".to_string(),
    }
}

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
    serde_json::from_slice(&b).expect("response body is not valid JSON")
}

fn rest_recovery_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/halted-run-fill-rest-recovery")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("serialize"),
        ))
        .unwrap()
}

fn snapshot_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/halted-run-portfolio-snapshot")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("serialize"),
        ))
        .unwrap()
}

fn no_db_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// DB fixture helpers — use "wf01" label namespace to avoid conflicts
// ---------------------------------------------------------------------------

async fn seed_halted_run(pool: &sqlx::PgPool) -> uuid::Uuid {
    let run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"mqk.test.halted-fill-repair-workflow-01.run",
    );
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into runs (run_id, engine_id, mode, started_at_utc, git_hash,
                          config_hash, config_json, host_fingerprint)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (run_id) do nothing
        "#,
    )
    .bind(run_id)
    .bind("test-daemon")
    .bind("PAPER")
    .bind(now)
    .bind("test-git-hash")
    .bind("test-config-hash")
    .bind(serde_json::json!({"source": "scenario_halted_fill_repair_workflow_01"}))
    .bind("test-host")
    .execute(pool)
    .await
    .expect("seed run");

    sqlx::query(
        r#"
        update runs
           set status = 'HALTED', halted_at_utc = $2
         where run_id = $1 and status != 'HALTED'
        "#,
    )
    .bind(run_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("halt run");

    run_id
}

async fn seed_sent_outbox_and_broker_map(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
) -> (String, String) {
    let internal_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"mqk.test.halted-fill-repair-workflow-01.order",
    )
    .to_string();
    let broker_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"mqk.test.halted-fill-repair-workflow-01.broker",
    )
    .to_string();
    let now = chrono::Utc::now();

    sqlx::query(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status,
                                created_at_utc, sent_at_utc)
        values ($1, $2, $3, 'SENT', $4, $4)
        on conflict (idempotency_key) do nothing
        "#,
    )
    .bind(run_id)
    .bind(&internal_id)
    .bind(serde_json::json!({"source": "halted-fill-repair-workflow-01-test"}))
    .bind(now)
    .execute(pool)
    .await
    .expect("seed outbox row");

    sqlx::query(
        r#"
        insert into broker_order_map (internal_id, broker_id, registered_at_utc)
        values ($1, $2, $3)
        on conflict (internal_id) do nothing
        "#,
    )
    .bind(&internal_id)
    .bind(&broker_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed broker_order_map");

    (internal_id, broker_id)
}

/// Seed the broker cursor so the broker_id appears in `last_message_id`,
/// giving the entry `cursor_only_fill_evidence` classification.
async fn seed_cursor_only_evidence(pool: &sqlx::PgPool, broker_id: &str) {
    let cursor_json = serde_json::json!({
        "schema_version": 1,
        "rest_activity_after": "",
        "trade_updates": {
            "status": "live",
            "last_message_id": format!("alpaca:{broker_id}:fill:2026-05-08T14:30:00.000000000Z"),
            "last_event_at": "2026-05-08T14:30:00.000000000Z"
        }
    })
    .to_string();
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into broker_event_cursor (adapter_id, cursor_value, updated_at)
        values ($1, $2, $3)
        on conflict (adapter_id) do update
            set cursor_value = excluded.cursor_value,
                updated_at   = excluded.updated_at
        "#,
    )
    .bind("alpaca")
    .bind(&cursor_json)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed broker cursor");
}

// Cleanup helpers — best-effort, non-fatal.
async fn cleanup(pool: &sqlx::PgPool, run_id: uuid::Uuid, internal_id: &str, act_id: &str) {
    let stable_msg_id = format!("alpaca-rest-recovery:{act_id}");
    sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from oms_inbox where broker_message_id = $1")
        .bind(&stable_msg_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from broker_order_map where internal_id = $1")
        .bind(internal_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from oms_outbox where idempotency_key = $1")
        .bind(internal_id)
        .execute(pool)
        .await
        .ok();
}

// DB assertion helpers.
async fn inbox_row_count(pool: &sqlx::PgPool, run_id: uuid::Uuid, msg_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select count(*) from oms_inbox where run_id = $1 and broker_message_id = $2",
    )
    .bind(run_id)
    .bind(msg_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

async fn inbox_applied_at(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    msg_id: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "select applied_at_utc from oms_inbox where run_id = $1 and broker_message_id = $2",
    )
    .bind(run_id)
    .bind(msg_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
}

async fn portfolio_snapshot_audit_count(pool: &sqlx::PgPool, run_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select count(*) from audit_events where run_id = $1 and event_type = $2",
    )
    .bind(run_id)
    .bind("ops.repair.portfolio_snapshot")
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

async fn run_status(pool: &sqlx::PgPool, run_id: uuid::Uuid) -> String {
    sqlx::query_scalar::<_, String>("select status from runs where run_id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ===========================================================================
// W01 — pure in-process: no DB → fail-closed
//
// REST recovery Gate 1 (DB required) → 503 no_db.
// Portfolio snapshot Gate 1 (confirmation) fires before Gate 2 (DB), so
// dry_run=false without token → 400 even without DB (Gate ordering proof).
// ===========================================================================

#[tokio::test]
async fn w01_no_db_returns_fail_closed() {
    let state = no_db_state();

    // REST recovery: DB gate fires first → 503 no_db.
    let (status, body) = call(
        routes::build_router(Arc::clone(&state)),
        rest_recovery_req(serde_json::json!({
            "run_id": "00000000-0000-0000-0000-000000000001",
            "internal_order_id": "test-order",
            "broker_order_id": "broker-abc"
        })),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "W01-REST: expected 503"
    );
    assert_eq!(
        j["truth_state"].as_str().unwrap_or(""),
        "no_db",
        "W01-REST: truth_state must be 'no_db'"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "W01-REST: decision must be 'refused'"
    );

    // Portfolio snapshot: Gate 1 (confirmation check) fires before Gate 2 (DB).
    // dry_run=false without token → 400 even without DB.
    let (status2, body2) = call(
        routes::build_router(Arc::clone(&state)),
        snapshot_req(serde_json::json!({
            "run_id": "00000000-0000-0000-0000-000000000001",
            "dry_run": false
        })),
    )
    .await;
    let j2 = parse_json(body2);
    assert_eq!(
        status2,
        StatusCode::BAD_REQUEST,
        "W01-SNAP-unconfirmed: expected 400"
    );
    assert_eq!(
        j2["gate"].as_str().unwrap_or(""),
        "snapshot.confirmation_required",
        "W01-SNAP-unconfirmed: gate must be 'snapshot.confirmation_required'"
    );

    // Portfolio snapshot: dry_run=true with no DB → 503 no_db (Gate 2 fires after Gate 1 passes).
    let (status3, body3) = call(
        routes::build_router(Arc::clone(&state)),
        snapshot_req(serde_json::json!({
            "run_id": "00000000-0000-0000-0000-000000000001",
            "dry_run": true
        })),
    )
    .await;
    let j3 = parse_json(body3);
    assert_eq!(
        status3,
        StatusCode::SERVICE_UNAVAILABLE,
        "W01-SNAP-nodb: expected 503"
    );
    assert_eq!(
        j3["truth_state"].as_str().unwrap_or(""),
        "no_db",
        "W01-SNAP-nodb: truth_state must be 'no_db'"
    );
}

// ===========================================================================
// W02–W12 — DB-backed sequential end-to-end workflow
//
// One function, sequential steps. Skips gracefully without MQK_DATABASE_URL.
// ===========================================================================

#[tokio::test]
async fn w02_to_w12_end_to_end_workflow() {
    // Connect to DB — skip if not available.
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP w02_to_w12: MQK_DATABASE_URL not set");
            return;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("w02: DB connect");
    mqk_db::migrate(&pool).await.expect("w02: migrate");

    // -----------------------------------------------------------------------
    // W02: Seed a halted run with cursor-only fill evidence.
    //
    // Preconditions:
    // - run exists and is HALTED
    // - outbox row + broker_order_map entry exist (SENT)
    // - broker cursor contains the broker_id in last_message_id (cursor_only evidence)
    // - NO unapplied inbox fill row exists
    // -----------------------------------------------------------------------
    let run_id = seed_halted_run(&pool).await;
    let (internal_id, broker_id) = seed_sent_outbox_and_broker_map(&pool, run_id).await;
    seed_cursor_only_evidence(&pool, &broker_id).await;

    // Verify the run is HALTED before proceeding.
    assert_eq!(
        run_status(&pool, run_id).await,
        "HALTED",
        "W02: run must be HALTED"
    );

    let act_id = "wf01-authoritative-fill";
    let stable_msg_id = format!("alpaca-rest-recovery:{act_id}");

    // No inbox row should exist yet.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        0,
        "W02: precondition — no inbox row before workflow begins"
    );

    // Build state with fake REST fetcher returning exactly one authoritative fill.
    let make_state_with_fetcher = || {
        let act = make_fill_activity(act_id, &broker_id);
        let mut inner = state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        );
        inner.set_fill_activity_fetcher_for_test(FakeFillFetcher::returning(vec![act]));
        Arc::new(inner)
    };

    let req_body_base = serde_json::json!({
        "run_id": run_id.to_string(),
        "internal_order_id": internal_id,
        "broker_order_id": broker_id
    });

    // -----------------------------------------------------------------------
    // W03: Fake REST fetcher is configured and returns exactly one fill.
    //      (Structural: fetcher is wired — proven by W04 returning the fill.)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // W04: dry_run=true on REST recovery → recovered fill details, nothing written.
    // -----------------------------------------------------------------------
    let mut req_w04 = req_body_base.clone();
    req_w04["dry_run"] = serde_json::json!(true);

    let (status, body) = call(
        routes::build_router(make_state_with_fetcher()),
        rest_recovery_req(req_w04),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(status, StatusCode::OK, "W04: expected 200; body={j}");
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "rest_recovered_fill_evidence",
        "W04: decision must be 'rest_recovered_fill_evidence'"
    );
    assert_eq!(
        j["dry_run"].as_bool(),
        Some(true),
        "W04: dry_run must be true"
    );
    assert_eq!(
        j["mutated"].as_bool(),
        Some(false),
        "W04: mutated must be false"
    );
    assert_eq!(
        j["classification"].as_str().unwrap_or(""),
        "cursor_only_fill_evidence",
        "W04: classification must be 'cursor_only_fill_evidence'"
    );
    let rf = &j["rest_fill"];
    assert!(!rf.is_null(), "W04: rest_fill must be present");
    assert_eq!(
        rf["broker_activity_id"].as_str().unwrap_or(""),
        act_id,
        "W04: broker_activity_id must match"
    );
    assert_eq!(
        rf["symbol"].as_str().unwrap_or(""),
        "NVDA",
        "W04: symbol must be NVDA"
    );
    assert_eq!(
        rf["side"].as_str().unwrap_or(""),
        "buy",
        "W04: side must be buy"
    );
    assert_eq!(
        rf["price_str"].as_str().unwrap_or(""),
        "152.75",
        "W04: price_str must match"
    );
    assert_eq!(
        rf["qty_str"].as_str().unwrap_or(""),
        "5",
        "W04: qty_str must match"
    );
    assert!(
        j["inbox_broker_message_id"].is_null(),
        "W04: no inbox_broker_message_id on dry_run"
    );

    // dry_run=true must not insert any inbox row.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        0,
        "W04: dry_run must not insert inbox row"
    );

    // -----------------------------------------------------------------------
    // W05: dry_run=false without confirmation → 400 refused, nothing written.
    // -----------------------------------------------------------------------
    let mut req_w05 = req_body_base.clone();
    req_w05["dry_run"] = serde_json::json!(false);
    // No confirmation field.

    let (status, body) = call(
        routes::build_router(make_state_with_fetcher()),
        rest_recovery_req(req_w05),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "W05: expected 400 refused; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "W05: decision must be 'refused'"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "repair.confirmation_required",
        "W05: gate must be 'repair.confirmation_required'"
    );
    assert_eq!(
        j["mutated"].as_bool(),
        Some(false),
        "W05: mutated must be false"
    );

    // Still no inbox row.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        0,
        "W05: refused must not insert inbox row"
    );

    // -----------------------------------------------------------------------
    // W06: dry_run=false + APPLY_REST_FILL_RECOVERY → 200 applied.
    //      Exactly one canonical inbox fill inserted and stamped applied.
    // -----------------------------------------------------------------------
    let mut req_w06 = req_body_base.clone();
    req_w06["dry_run"] = serde_json::json!(false);
    req_w06["confirmation"] = serde_json::json!("APPLY_REST_FILL_RECOVERY");

    let (status, body) = call(
        routes::build_router(make_state_with_fetcher()),
        rest_recovery_req(req_w06),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::OK,
        "W06: expected 200 applied; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "applied",
        "W06: decision must be 'applied'"
    );
    assert_eq!(
        j["mutated"].as_bool(),
        Some(true),
        "W06: mutated must be true"
    );
    assert_eq!(
        j["inbox_broker_message_id"].as_str().unwrap_or(""),
        stable_msg_id,
        "W06: inbox_broker_message_id must match stable key"
    );
    let rf = &j["rest_fill"];
    assert!(!rf.is_null(), "W06: rest_fill must be present");
    assert_eq!(
        rf["mutation_safe"].as_bool(),
        Some(true),
        "W06: mutation_safe must be true after apply"
    );

    // Exactly one inbox row must exist, with applied_at_utc set.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        1,
        "W06: exactly one inbox row after confirmed apply"
    );
    assert!(
        inbox_applied_at(&pool, run_id, &stable_msg_id)
            .await
            .is_some(),
        "W06: applied_at_utc must be set after confirmed apply"
    );

    // -----------------------------------------------------------------------
    // W07: Second confirmed REST apply → 200 already_repaired, no duplicate row.
    // -----------------------------------------------------------------------
    let mut req_w07 = req_body_base.clone();
    req_w07["dry_run"] = serde_json::json!(false);
    req_w07["confirmation"] = serde_json::json!("APPLY_REST_FILL_RECOVERY");

    let (status, body) = call(
        routes::build_router(make_state_with_fetcher()),
        rest_recovery_req(req_w07),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::OK,
        "W07: expected 200 already_repaired; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "already_repaired",
        "W07: decision must be 'already_repaired'"
    );
    assert_eq!(
        j["mutated"].as_bool(),
        Some(false),
        "W07: mutated must be false on idempotent call"
    );

    // Still exactly one inbox row — no duplicate.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        1,
        "W07: still exactly one inbox row after idempotent call"
    );

    // -----------------------------------------------------------------------
    // W08: Portfolio snapshot dry_run=true → computes from applied fill, no audit written.
    // -----------------------------------------------------------------------
    let state_snap = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&state_snap)),
        snapshot_req(serde_json::json!({
            "run_id": run_id.to_string(),
            "dry_run": true
        })),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::OK,
        "W08: expected 200 dry_run_ok; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "dry_run_ok",
        "W08: decision must be 'dry_run_ok'"
    );
    assert_eq!(
        j["dry_run"].as_bool(),
        Some(true),
        "W08: dry_run must be true"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        1,
        "W08: applied_fill_count must be 1 (from the REST-recovered fill)"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "W08: snapshot_written must be false on dry_run"
    );
    assert!(
        j["audit_event_id"].is_null(),
        "W08: audit_event_id must be null on dry_run"
    );
    // Positions must reflect the one NVDA buy fill.
    let positions = j["positions"]
        .as_array()
        .expect("W08: positions must be an array");
    assert_eq!(positions.len(), 1, "W08: must have exactly one position");
    assert_eq!(
        positions[0]["symbol"].as_str().unwrap_or(""),
        "NVDA",
        "W08: position symbol must be NVDA"
    );
    assert!(
        positions[0]["qty_signed"].as_i64().unwrap_or(0) > 0,
        "W08: NVDA qty_signed must be positive (buy fill)"
    );

    // No audit event written.
    assert_eq!(
        portfolio_snapshot_audit_count(&pool, run_id).await,
        0,
        "W08: dry_run must not write audit event"
    );

    // -----------------------------------------------------------------------
    // W09: Portfolio snapshot dry_run=false without confirmation → 400 refused.
    //      (Gate 1 fires before Gate 2 — no DB touch)
    // -----------------------------------------------------------------------
    let (status, body) = call(
        routes::build_router(Arc::clone(&state_snap)),
        snapshot_req(serde_json::json!({
            "run_id": run_id.to_string(),
            "dry_run": false
        })),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "W09: expected 400 refused; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "W09: decision must be 'refused'"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "snapshot.confirmation_required",
        "W09: gate must be 'snapshot.confirmation_required'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "W09: snapshot_written must be false"
    );

    // Still no audit event written.
    assert_eq!(
        portfolio_snapshot_audit_count(&pool, run_id).await,
        0,
        "W09: refused must not write audit event"
    );

    // -----------------------------------------------------------------------
    // W10: Portfolio snapshot dry_run=false + WRITE_PORTFOLIO_SNAPSHOT → 200 snapshot_written.
    //      One durable audit event written.
    // -----------------------------------------------------------------------
    let (status, body) = call(
        routes::build_router(Arc::clone(&state_snap)),
        snapshot_req(serde_json::json!({
            "run_id": run_id.to_string(),
            "dry_run": false,
            "confirmation": "WRITE_PORTFOLIO_SNAPSHOT"
        })),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::OK,
        "W10: expected 200 snapshot_written; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "snapshot_written",
        "W10: decision must be 'snapshot_written'"
    );
    assert_eq!(
        j["dry_run"].as_bool(),
        Some(false),
        "W10: dry_run must be false"
    );
    assert!(
        j["snapshot_written"].as_bool().unwrap_or(false),
        "W10: snapshot_written must be true"
    );
    assert!(
        !j["audit_event_id"].is_null(),
        "W10: audit_event_id must be present"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        1,
        "W10: applied_fill_count must be 1"
    );

    // Exactly one portfolio snapshot audit event.
    assert_eq!(
        portfolio_snapshot_audit_count(&pool, run_id).await,
        1,
        "W10: exactly one portfolio snapshot audit event after confirmed write"
    );

    // -----------------------------------------------------------------------
    // W11: Second confirmed snapshot → 200 already_current, no duplicate audit event.
    // -----------------------------------------------------------------------
    let (status, body) = call(
        routes::build_router(Arc::clone(&state_snap)),
        snapshot_req(serde_json::json!({
            "run_id": run_id.to_string(),
            "dry_run": false,
            "confirmation": "WRITE_PORTFOLIO_SNAPSHOT"
        })),
    )
    .await;
    let j = parse_json(body);
    assert_eq!(
        status,
        StatusCode::OK,
        "W11: expected 200 already_current; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "already_current",
        "W11: decision must be 'already_current'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "W11: snapshot_written must be false on idempotent call"
    );
    assert!(
        !j["audit_event_id"].is_null(),
        "W11: audit_event_id must still be returned"
    );

    // Still exactly one audit event — no duplicate.
    assert_eq!(
        portfolio_snapshot_audit_count(&pool, run_id).await,
        1,
        "W11: still exactly one portfolio snapshot audit event after idempotent call"
    );

    // -----------------------------------------------------------------------
    // W12: Final DB invariant assertions.
    // -----------------------------------------------------------------------

    // Exactly one recovered inbox fill row for the run.
    assert_eq!(
        inbox_row_count(&pool, run_id, &stable_msg_id).await,
        1,
        "W12: exactly one recovered inbox fill row"
    );

    // The recovered inbox fill has applied_at_utc set.
    assert!(
        inbox_applied_at(&pool, run_id, &stable_msg_id)
            .await
            .is_some(),
        "W12: recovered inbox fill must have applied_at_utc set"
    );

    // Exactly one portfolio snapshot audit event for the run.
    assert_eq!(
        portfolio_snapshot_audit_count(&pool, run_id).await,
        1,
        "W12: exactly one portfolio snapshot audit event"
    );

    // No direct portfolio mutation table written.
    // (mqk_portfolio::PortfolioState is in-memory only; there is no separate
    //  portfolio positions table written by the repair route.)
    // Verified structurally: the repair route only writes to oms_inbox
    // (inbox_insert_deduped_with_identity + inbox_mark_applied) and audit_events
    // (portfolio_snapshot). Both are proven above.

    // Run status must still be HALTED — the workflow must not alter run lifecycle.
    assert_eq!(
        run_status(&pool, run_id).await,
        "HALTED",
        "W12: run status must still be HALTED after workflow"
    );

    // -----------------------------------------------------------------------
    // Cleanup
    // -----------------------------------------------------------------------
    cleanup(&pool, run_id, &internal_id, act_id).await;
}
