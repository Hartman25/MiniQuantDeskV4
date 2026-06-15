//! A5A: per-order execution timeline route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/timeline`.
//!
//! # Truth-state contract under test
//!
//! | Condition                                                       | truth_state                            |
//! |-------------------------------------------------------------------|-------------------------------------------|
//! | No DB pool                                                      | no_db                                   |
//! | DB pool + no active run                                         | no_order                                |
//! | DB pool + active run + order in OMS snapshot, filled_qty == 0   | no_fills_yet                            |
//! | DB pool + active run + fill quality rows (active or prior run)  | active                                  |
//! | DB pool + active run + order Filled, zero rows in any run       | filled_without_fill_quality_telemetry  |
//!
//! The DB-backed states (no_order, no_fills_yet, active,
//! filled_without_fill_quality_telemetry) all require MQK_DATABASE_URL and
//! skip gracefully in CI when it is absent.

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use std::sync::Arc;
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

// ---------------------------------------------------------------------------
// TL-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    // Wrapper shape contract
    assert!(
        v.get("canonical_route").is_some(),
        "canonical_route field required"
    );
    assert!(v.get("truth_state").is_some(), "truth_state field required");
    assert!(v.get("backend").is_some(), "backend field required");
    assert!(v.get("order_id").is_some(), "order_id field required");
    assert!(v.get("rows").is_some(), "rows field required");
    assert!(v["rows"].is_array(), "rows must be an array");
}

// ---------------------------------------------------------------------------
// TL-02: canonical_route includes the order_id in the path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl02_canonical_route_includes_order_id() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-order-xyz/timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"]
        .as_str()
        .expect("canonical_route is a string");
    assert!(
        route.contains("my-order-xyz"),
        "canonical_route must include the resolved order_id; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// TL-03: no_db truth_state when DB pool is absent (pure in-process).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl03_no_db_truth_state_without_db_pool() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/timeline")
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
    let rows = v["rows"].as_array().expect("rows is array");
    assert!(
        rows.is_empty(),
        "rows must be empty when truth_state is no_db"
    );
}

// ---------------------------------------------------------------------------
// TL-04: rows array is empty for no_db state (not authoritative zero).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl04_rows_empty_not_authoritative_for_no_db() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-99/timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // Explicit check: empty rows under no_db must NOT be treated as "no fills exist".
    // The test documents the contract; the renderer must check truth_state first.
    assert_eq!(v["truth_state"].as_str(), Some("no_db"));
    assert_eq!(v["rows"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// TL-05: blank order_id returns 400 (not a valid order detail request).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl05_blank_order_id_returns_400() {
    // Axum path routing won't match "/api/v1/execution/orders//timeline"
    // (empty segment), so this test validates the trim+check in the handler
    // via a URL-encoded space that decodes to blank after trim.
    // NOTE: Axum will 404 on a truly empty path segment before reaching the handler.
    // We instead test that the route exists for a non-blank id (already covered by TL-01).
    // Document: blank order_id is rejected at the path routing layer (404 or 400).
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/timeline") // URL-encoded space → " " → trim → ""
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _body) = call(router, req).await;
    // After trim, the handler should return 400. Allow 404 if routing rejects first.
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// TL-06: order_id is reflected in the response body.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl06_order_id_reflected_in_response() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/reflected-order-id/timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(
        v["order_id"].as_str(),
        Some("reflected-order-id"),
        "order_id must be echoed back in the response"
    );
}

// ---------------------------------------------------------------------------
// TL-07: nullable identity fields are present (may be null without snapshot).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tl07_nullable_identity_fields_present_in_schema() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/test-order/timeline")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // These fields must be present in the response (even if null).
    for field in &[
        "broker_order_id",
        "symbol",
        "requested_qty",
        "filled_qty",
        "current_status",
        "current_stage",
        "last_event_at",
    ] {
        assert!(
            v.get(field).is_some(),
            "response must contain field '{field}'"
        );
    }
}

// ---------------------------------------------------------------------------
// DB-backed tests (FILL-QUALITY-TELEMETRY-PROOF-01)
//
// | Condition                                                            | truth_state                            |
// |------------------------------------------------------------------------|-----------------------------------------|
// | Fill recorded under a prior run; active run has no rows for this order | active (cross-run fallback)            |
// | Order Filled in OMS snapshot; zero telemetry rows in any run           | filled_without_fill_quality_telemetry  |
//
// Both require MQK_DATABASE_URL and skip gracefully in CI when absent.
// ---------------------------------------------------------------------------

async fn connect_db() -> Option<sqlx::PgPool> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => return None,
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("A5A DB-backed tests: DB connect failed");
    Some(pool)
}

/// Seed a minimal CREATED run anchored to engine_id="mqk-daemon", mode="PAPER",
/// at the given `started_at_utc`. Used to control which run is "latest" for
/// `fetch_latest_run_for_engine`.
async fn seed_run(pool: &sqlx::PgPool, run_id: uuid::Uuid, started_at: chrono::DateTime<chrono::Utc>) {
    sqlx::query("delete from fill_quality_telemetry where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test fill_quality_telemetry cleanup failed");
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test audit_events cleanup failed");
    sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test oms_inbox cleanup failed");
    sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test oms_outbox cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test runs cleanup failed");

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "a5a-proof-hash".to_string(),
            config_hash: "a5a-proof-cfg".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "a5a-proof-host".to_string(),
        },
    )
    .await
    .expect("seed_run insert_run failed");
}

fn exec_snapshot_with_filled_order(
    run_id: uuid::Uuid,
    order_id: &str,
) -> mqk_runtime::observability::ExecutionSnapshot {
    mqk_runtime::observability::ExecutionSnapshot {
        run_id: Some(run_id),
        active_orders: vec![mqk_runtime::observability::OrderSnapshot {
            order_id: order_id.to_string(),
            // Mirrors the f9f8c1f7 evidence: broker_order_id is deregistered
            // from in-memory state on terminal fill (BRK terminal-fill cleanup).
            broker_order_id: None,
            symbol: "AAPL".to_string(),
            total_qty: 5,
            filled_qty: 5,
            status: "Filled".to_string(),
        }],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros: 100_000_000_000,
            realized_pnl_micros: 0,
            positions: vec![],
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// TL-08: cross-run fallback — a fill recorded under a prior run is returned as
// truth_state="active" when the now-active run has no telemetry rows for this
// order. Also proves broker_order_id is not invented from the telemetry row.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn tl08_cross_run_fallback_returns_active_with_prior_run_fill() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("TL-08: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let prior_run_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000a108").unwrap();
    let active_run_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000b108").unwrap();

    let t0 = chrono::DateTime::parse_from_rfc3339("2020-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let t1 = chrono::DateTime::parse_from_rfc3339("2020-01-01T11:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    seed_run(&pool, prior_run_id, t0).await;
    seed_run(&pool, active_run_id, t1).await;

    let internal_order_id = "ord-tl08-cross-run";
    let broker_message_id = "tl08-fill-msg-1";
    let telemetry_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", prior_run_id, broker_message_id).as_bytes(),
    );

    let row = mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id: prior_run_id,
        internal_order_id: internal_order_id.to_string(),
        broker_order_id: Some("brk-tl08-prior".to_string()),
        broker_fill_id: Some("fill-tl08-1".to_string()),
        broker_message_id: broker_message_id.to_string(),
        symbol: "AAPL".to_string(),
        side: "buy".to_string(),
        ordered_qty: 5,
        fill_qty: 5,
        fill_price_micros: 298_266_000,
        reference_price_micros: None,
        slippage_bps: None,
        submit_ts_utc: None,
        fill_received_at_utc: chrono::Utc::now(),
        submit_to_fill_ms: None,
        fill_kind: "final_fill".to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: chrono::Utc::now(),
    };
    mqk_db::insert_fill_quality_telemetry(&pool, &row)
        .await
        .expect("TL-08: insert telemetry under prior run must succeed");

    // Arm the NEWER run so it becomes the active run — the telemetry row above
    // is recorded under `prior_run_id`, not `active_run_id`.
    mqk_db::arm_run(&pool, active_run_id)
        .await
        .expect("TL-08: arm_run on active_run_id must succeed");

    let st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    );
    {
        let mut snap = st.execution_snapshot.write().await;
        *snap = Some(exec_snapshot_with_filled_order(active_run_id, internal_order_id));
    }

    let router = routes::build_router(Arc::new(st));
    let req = Request::builder()
        .uri(format!("/api/v1/execution/orders/{internal_order_id}/timeline"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["truth_state"].as_str(),
        Some("active"),
        "TL-08: cross-run fallback must report truth_state=active, got {v}"
    );
    assert_eq!(
        v["backend"].as_str(),
        Some("postgres.fill_quality_telemetry")
    );
    let rows = v["rows"].as_array().expect("rows is array");
    assert_eq!(rows.len(), 1, "TL-08: exactly one fill row from prior run");
    assert_eq!(
        rows[0]["event_id"].as_str(),
        Some(telemetry_id.to_string().as_str()),
        "TL-08: returned row must be the one recorded under the prior run"
    );

    // broker_order_id must not be invented from the telemetry row (which
    // carries broker_order_id="brk-tl08-prior"). The top-level
    // broker_order_id reflects only the in-memory OMS snapshot, which has it
    // as null.
    assert!(
        v["broker_order_id"].is_null(),
        "TL-08: broker_order_id must not be backfilled from fill_quality_telemetry; got {v}"
    );
}

// ---------------------------------------------------------------------------
// TL-09: filled order with zero telemetry rows in any run reports the honest
// residual truth_state, not "no_fills_yet".
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn tl09_filled_without_telemetry_returns_honest_truth_state() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("TL-09: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let active_run_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000c109").unwrap();
    let t0 = chrono::DateTime::parse_from_rfc3339("2020-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    seed_run(&pool, active_run_id, t0).await;

    let internal_order_id = "ord-tl09-no-telemetry";

    // Precondition: no telemetry rows exist anywhere for this order_id.
    let existing =
        mqk_db::fetch_fill_quality_telemetry_for_order_any_run(&pool, internal_order_id)
            .await
            .expect("TL-09: any-run fetch must succeed even when empty");
    assert!(
        existing.is_empty(),
        "TL-09: precondition — no telemetry rows must exist for this order_id"
    );

    mqk_db::arm_run(&pool, active_run_id)
        .await
        .expect("TL-09: arm_run must succeed");

    let st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    );
    {
        let mut snap = st.execution_snapshot.write().await;
        *snap = Some(exec_snapshot_with_filled_order(active_run_id, internal_order_id));
    }

    let router = routes::build_router(Arc::new(st));
    let req = Request::builder()
        .uri(format!("/api/v1/execution/orders/{internal_order_id}/timeline"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["truth_state"].as_str(),
        Some("filled_without_fill_quality_telemetry"),
        "TL-09: filled order with zero telemetry anywhere must report the honest residual state, got {v}"
    );
    assert_eq!(
        v["backend"].as_str(),
        Some("postgres.fill_quality_telemetry"),
        "TL-09: backend must still name the source table (not 'unavailable')"
    );
    assert_eq!(
        v["rows"].as_array().unwrap().len(),
        0,
        "TL-09: rows must be empty"
    );
    assert_eq!(v["filled_qty"].as_i64(), Some(5));
}
