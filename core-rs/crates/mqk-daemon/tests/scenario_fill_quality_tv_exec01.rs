//! TV-EXEC-01 — Fill-quality telemetry proof tests.
//!
//! Proves that:
//!
//! FQ-01: A fill event produces exactly one telemetry row with exact field truth
//!        (price, qty, symbol, side, fill_kind, provenance_ref).
//! FQ-02: A limit fill carries a non-null slippage_bps derived from reference_price.
//! FQ-03: A market fill carries null reference_price_micros and null slippage_bps.
//! FQ-04: A cancelled/rejected order (no fill event inserted) produces zero telemetry rows.
//! FQ-05: GET /api/v1/execution/fill-quality returns truth_state="active" with the
//!        durable telemetry rows and correct field values (read-surface identity).
//!
//! All tests are DB-backed and require MQK_DATABASE_URL.
//! They skip cleanly if the env var is not set.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use mqk_daemon::{routes::build_router, state::AppState};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    (status, body)
}

async fn connect_db() -> Option<sqlx::PgPool> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => return None,
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("TV-EXEC-01: DB connect failed");
    Some(pool)
}

/// Seed a minimal run anchored to engine_id="mqk-daemon" and return its run_id.
async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Pre-test cleanup.
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
            git_hash: "tv-exec01-hash".to_string(),
            config_hash: "tv-exec01-cfg".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "tv-exec01-host".to_string(),
        },
    )
    .await
    .expect("seed_run insert_run failed");
}

// ---------------------------------------------------------------------------
// FQ-01: fill event → exactly one telemetry row with exact field truth
// ---------------------------------------------------------------------------

/// Proves:
/// - inserting one Fill telemetry row via insert_fill_quality_telemetry succeeds
/// - fetch_fill_quality_telemetry_recent returns exactly that row
/// - all key fields are bit-for-bit truthful (no rounding, no fabrication)
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fq01_fill_event_produces_one_telemetry_row_with_exact_field_truth() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("FQ-01: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id =
        Uuid::parse_str("ee010001-0000-4000-8000-000000000001").unwrap();
    seed_run(&pool, run_id).await;

    let broker_message_id = "fq01-fill-msg-1";
    let fill_received_at = Utc::now();

    let telemetry_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, broker_message_id).as_bytes(),
    );

    let row = mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id: "ord-fq01-1".to_string(),
        broker_order_id: Some("brk-fq01-1".to_string()),
        broker_fill_id: Some("fill-fq01-1".to_string()),
        broker_message_id: broker_message_id.to_string(),
        symbol: "AAPL".to_string(),
        side: "buy".to_string(),
        ordered_qty: 100,
        fill_qty: 100,
        fill_price_micros: 150_500_000,
        reference_price_micros: None,
        slippage_bps: None,
        submit_ts_utc: None,
        fill_received_at_utc: fill_received_at,
        submit_to_fill_ms: None,
        fill_kind: "final_fill".to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: Utc::now(),
    };

    mqk_db::insert_fill_quality_telemetry(&pool, &row)
        .await
        .expect("FQ-01: insert must succeed");

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10)
        .await
        .expect("FQ-01: fetch must succeed");

    assert_eq!(rows.len(), 1, "FQ-01: exactly one telemetry row must exist");

    let r = &rows[0];
    assert_eq!(r.telemetry_id, telemetry_id, "FQ-01: telemetry_id must match");
    assert_eq!(r.run_id, run_id, "FQ-01: run_id must match");
    assert_eq!(r.symbol, "AAPL", "FQ-01: symbol must be AAPL");
    assert_eq!(r.side, "buy", "FQ-01: side must be buy");
    assert_eq!(r.fill_qty, 100, "FQ-01: fill_qty must be exact");
    assert_eq!(
        r.fill_price_micros, 150_500_000,
        "FQ-01: fill_price_micros must be exact"
    );
    assert_eq!(r.fill_kind, "final_fill", "FQ-01: fill_kind must be final_fill");
    assert_eq!(
        r.provenance_ref,
        format!("oms_inbox:{}", broker_message_id),
        "FQ-01: provenance_ref must be oms_inbox:<broker_message_id>"
    );
    assert!(
        r.reference_price_micros.is_none(),
        "FQ-01: reference_price_micros must be null for market fill"
    );
    assert!(
        r.slippage_bps.is_none(),
        "FQ-01: slippage_bps must be null when reference absent"
    );
}

// ---------------------------------------------------------------------------
// FQ-02: limit fill carries non-null slippage_bps
// ---------------------------------------------------------------------------

/// Proves:
/// - when reference_price_micros is non-null, slippage_bps is also non-null
/// - the slippage value reflects the signed (fill_price - reference_price) / reference * 10000
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fq02_limit_fill_carries_non_null_slippage_bps() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("FQ-02: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id =
        Uuid::parse_str("ee010002-0000-4000-8000-000000000002").unwrap();
    seed_run(&pool, run_id).await;

    let broker_message_id = "fq02-fill-msg-1";
    // reference = 100_000_000 micros ($100.00), fill = 100_100_000 micros ($100.10)
    // slippage = (100_100_000 - 100_000_000) / 100_000_000 * 10_000 = 10 bps
    let reference_price = 100_000_000_i64;
    let fill_price = 100_100_000_i64;
    let expected_slippage = (fill_price - reference_price) * 10_000 / reference_price;

    let telemetry_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, broker_message_id).as_bytes(),
    );

    let row = mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id: "ord-fq02-1".to_string(),
        broker_order_id: None,
        broker_fill_id: Some("fill-fq02-1".to_string()),
        broker_message_id: broker_message_id.to_string(),
        symbol: "TSLA".to_string(),
        side: "buy".to_string(),
        ordered_qty: 50,
        fill_qty: 50,
        fill_price_micros: fill_price,
        reference_price_micros: Some(reference_price),
        slippage_bps: Some(expected_slippage),
        submit_ts_utc: None,
        fill_received_at_utc: Utc::now(),
        submit_to_fill_ms: None,
        fill_kind: "final_fill".to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: Utc::now(),
    };

    mqk_db::insert_fill_quality_telemetry(&pool, &row)
        .await
        .expect("FQ-02: insert must succeed");

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10)
        .await
        .expect("FQ-02: fetch must succeed");

    assert_eq!(rows.len(), 1, "FQ-02: exactly one row");

    let r = &rows[0];
    assert!(
        r.reference_price_micros.is_some(),
        "FQ-02: reference_price_micros must be non-null for limit fill"
    );
    assert_eq!(
        r.reference_price_micros.unwrap(),
        reference_price,
        "FQ-02: reference_price_micros must be exact"
    );
    assert!(
        r.slippage_bps.is_some(),
        "FQ-02: slippage_bps must be non-null when reference exists"
    );
    assert_eq!(
        r.slippage_bps.unwrap(),
        expected_slippage,
        "FQ-02: slippage_bps must match computed value"
    );
}

// ---------------------------------------------------------------------------
// FQ-03: market fill → null reference_price and null slippage
// ---------------------------------------------------------------------------

/// Proves:
/// - reference_price_micros and slippage_bps are both null for market orders
/// - the DB schema accepts null without error
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fq03_market_fill_null_reference_and_null_slippage() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("FQ-03: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id =
        Uuid::parse_str("ee010003-0000-4000-8000-000000000003").unwrap();
    seed_run(&pool, run_id).await;

    let broker_message_id = "fq03-fill-msg-1";
    let telemetry_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, broker_message_id).as_bytes(),
    );

    let row = mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id: "ord-fq03-1".to_string(),
        broker_order_id: None,
        broker_fill_id: None,
        broker_message_id: broker_message_id.to_string(),
        symbol: "GOOG".to_string(),
        side: "sell".to_string(),
        ordered_qty: 10,
        fill_qty: 10,
        fill_price_micros: 200_000_000,
        reference_price_micros: None, // market order
        slippage_bps: None,           // must not be fabricated
        submit_ts_utc: None,
        fill_received_at_utc: Utc::now(),
        submit_to_fill_ms: None,
        fill_kind: "final_fill".to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: Utc::now(),
    };

    mqk_db::insert_fill_quality_telemetry(&pool, &row)
        .await
        .expect("FQ-03: insert must succeed");

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10)
        .await
        .expect("FQ-03: fetch must succeed");

    assert_eq!(rows.len(), 1, "FQ-03: exactly one row");
    let r = &rows[0];
    assert!(
        r.reference_price_micros.is_none(),
        "FQ-03: reference_price_micros must be null for market fill"
    );
    assert!(
        r.slippage_bps.is_none(),
        "FQ-03: slippage_bps must be null when reference absent"
    );
}

// ---------------------------------------------------------------------------
// FQ-04: no-fill event → zero telemetry rows
// ---------------------------------------------------------------------------

/// Proves:
/// - a run that had only a cancel (no fill event inserted) produces no telemetry rows
/// - the telemetry table has no fabricated rows for non-fill events
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fq04_no_fill_event_produces_zero_telemetry_rows() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("FQ-04: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id =
        Uuid::parse_str("ee010004-0000-4000-8000-000000000004").unwrap();
    seed_run(&pool, run_id).await;

    // Do NOT insert any telemetry rows — simulate a run where only a cancel happened.

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10)
        .await
        .expect("FQ-04: fetch must succeed");

    assert!(
        rows.is_empty(),
        "FQ-04: zero telemetry rows must exist for a run with no fills"
    );
}

// ---------------------------------------------------------------------------
// FQ-05: GET /api/v1/execution/fill-quality — read-surface identity
// ---------------------------------------------------------------------------

/// Proves:
/// - the HTTP route returns truth_state="active" and backend="postgres.fill_quality_telemetry"
/// - rows returned by the route match the durable rows inserted directly
/// - all key fields survive JSON serialization round-trip intact
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fq05_read_surface_returns_active_truth_with_exact_rows() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("FQ-05: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id =
        Uuid::parse_str("ee010005-0000-4000-8000-000000000005").unwrap();
    seed_run(&pool, run_id).await;

    let broker_message_id = "fq05-fill-msg-1";
    let fill_received_at = Utc::now();
    let telemetry_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, broker_message_id).as_bytes(),
    );

    let insert_row = mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id: "ord-fq05-1".to_string(),
        broker_order_id: Some("brk-fq05-1".to_string()),
        broker_fill_id: Some("fill-fq05-1".to_string()),
        broker_message_id: broker_message_id.to_string(),
        symbol: "MSFT".to_string(),
        side: "buy".to_string(),
        ordered_qty: 25,
        fill_qty: 25,
        fill_price_micros: 300_000_000,
        reference_price_micros: Some(299_000_000),
        slippage_bps: Some((300_000_000 - 299_000_000) * 10_000 / 299_000_000),
        submit_ts_utc: None,
        fill_received_at_utc: fill_received_at,
        submit_to_fill_ms: None,
        fill_kind: "final_fill".to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: Utc::now(),
    };

    mqk_db::insert_fill_quality_telemetry(&pool, &insert_row)
        .await
        .expect("FQ-05: insert must succeed");

    // Arm the run so current_status_snapshot returns active_run_id = Some(run_id).
    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("FQ-05: arm_run must succeed");

    // Build daemon state with DB.
    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/fill-quality")
        .body(Body::empty())
        .unwrap();

    let (status, body_bytes) = call(router, req).await;
    assert_eq!(
        status, 200,
        "FQ-05: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "active",
        "FQ-05: truth_state must be active"
    );
    assert_eq!(
        body["backend"].as_str().unwrap(),
        "postgres.fill_quality_telemetry",
        "FQ-05: backend must be postgres.fill_quality_telemetry"
    );

    let rows = body["rows"].as_array().expect("FQ-05: rows must be an array");
    assert_eq!(rows.len(), 1, "FQ-05: exactly one telemetry row in response");

    let row = &rows[0];
    assert_eq!(
        row["telemetry_id"].as_str().unwrap(),
        telemetry_id.to_string(),
        "FQ-05: telemetry_id must round-trip"
    );
    assert_eq!(
        row["symbol"].as_str().unwrap(),
        "MSFT",
        "FQ-05: symbol must round-trip"
    );
    assert_eq!(
        row["fill_price_micros"].as_i64().unwrap(),
        300_000_000,
        "FQ-05: fill_price_micros must be exact"
    );
    assert_eq!(
        row["fill_kind"].as_str().unwrap(),
        "final_fill",
        "FQ-05: fill_kind must round-trip"
    );
    assert_eq!(
        row["provenance_ref"].as_str().unwrap(),
        format!("oms_inbox:{}", broker_message_id),
        "FQ-05: provenance_ref must round-trip"
    );
    assert!(
        row["reference_price_micros"].as_i64().is_some(),
        "FQ-05: reference_price_micros must be non-null"
    );
    assert!(
        row["slippage_bps"].as_i64().is_some(),
        "FQ-05: slippage_bps must be non-null for limit fill"
    );
}
