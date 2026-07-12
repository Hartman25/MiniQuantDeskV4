//! PAPER-DAILY-PNL-CAPTURE-01B: `POST /api/v1/ops/action
//! {"action_key":"capture-account-equity-baseline"}` — explicit,
//! operator-controlled capture of a `sys_account_equity_baseline` row from
//! the daemon's real `broker_snapshot`.
//!
//! The capture -> `GET /api/v1/portfolio/summary` read-side loop proof is
//! added in PAPER-DAILY-PNL-CAPTURE-01C (a later phase of this same patch
//! group), appended to this file.
//!
//! # Proof matrix
//!
//! | Test     | What it proves                                                     |
//! |----------|----------------------------------------------------------------------|
//! | PDBC-01  | Unauthorized (TokenRequired, no/wrong bearer) -> 401, no row written  |
//! | PDBC-02  | No DB pool -> `db_unavailable`, 503                                  |
//! | PDBC-03  | DB present, no broker snapshot -> `no_broker_snapshot`, 503           |
//! | PDBC-04  | Blank/missing `reason` -> `missing_reason`, 400                      |
//! | PDBC-05  | Missing `trading_date` -> `missing_trading_date`, 400                |
//! | PDBC-06  | Malformed `trading_date` -> `invalid_trading_date`, 400               |
//! | PDBC-07  | `trading_date` is a real weekend day -> `non_trading_day`, 403        |
//! | PDBC-08  | Valid capture -> 200 `applied`, row written, provenance correct       |
//! | PDBC-09  | Repeated capture, same `trading_date` -> still exactly one row       |
//! | PDBC-10  | A successful capture call writes zero `oms_outbox`/`oms_inbox` rows   |
//! | PDBC-11  | Same inputs twice -> identical deterministic `audit_event_id`        |
//!
//! All DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing
//! at the local paper DB (port 5440 / miniquantdesk_paper), matching every
//! prior test file in this patch lineage. No provider, broker, or network
//! call in any test. No order/outbox/inbox row is ever written by this
//! action.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use http_body_util::BodyExt;
use mqk_daemon::state::{MarketCalendarProvider, NyseWeekdaysProvider};
use mqk_daemon::{routes, state};
use mqk_schemas::{BrokerAccount, BrokerPosition, BrokerSnapshot};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router_with_state() -> (Arc<state::AppState>, axum::Router) {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body)
}

fn parse_json(body: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&body).expect("response body must be valid JSON")
}

fn get(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body_json: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body_json.to_string()))
        .unwrap()
}

fn post_json_auth(uri: &str, body_json: &str, bearer: Option<&str>) -> Request<axum::body::Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder
        .body(axum::body::Body::from(body_json.to_string()))
        .unwrap()
}

fn make_snapshot(equity: &str, cash: &str, positions: Vec<BrokerPosition>) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: equity.to_string(),
            cash: cash.to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions,
    }
}

fn position(symbol: &str, qty: &str, avg_price: &str) -> BrokerPosition {
    BrokerPosition {
        symbol: symbol.to_string(),
        qty: qty.to_string(),
        avg_price: avg_price.to_string(),
    }
}

/// Independent reimplementation of the read-side's
/// `most_recent_trading_day_before`, mirroring
/// `scenario_paper_daily_pnl_baseline_01.rs`'s
/// `expected_required_trading_day` exactly, so tests can compute a real
/// trading day to capture without depending on the route's private helper.
fn expected_required_trading_day(now_utc: DateTime<Utc>) -> NaiveDate {
    let mut candidate = now_utc.date_naive().pred_opt().expect("pred date");
    for _ in 0..10 {
        let probe = candidate
            .and_hms_opt(18, 0, 0)
            .expect("valid time")
            .and_utc();
        if NyseWeekdaysProvider.session_for(probe).is_trading_day {
            return candidate;
        }
        candidate = candidate.pred_opt().expect("pred date");
    }
    panic!("no trading day found in 10-day walk-back window");
}

/// A definitely-a-Saturday date, verified independent of any holiday
/// table (weekend detection is pure weekday arithmetic).
fn a_known_saturday() -> NaiveDate {
    let d = NaiveDate::from_ymd_opt(2024, 1, 6).expect("valid date");
    assert_eq!(d.weekday(), chrono::Weekday::Sat, "fixture must be a Saturday");
    d
}

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

async fn connected_pool(db_url: &str) -> sqlx::PgPool {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .expect("pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");
    pool
}

async fn delete_baseline(pool: &sqlx::PgPool, trading_date: NaiveDate) {
    let _ = sqlx::query("delete from sys_account_equity_baseline where trading_date = $1")
        .bind(trading_date)
        .execute(pool)
        .await;
}

async fn fetch_baseline_row_count(pool: &sqlx::PgPool, trading_date: NaiveDate) -> i64 {
    sqlx::query_scalar(
        "select count(*) from sys_account_equity_baseline where trading_date = $1",
    )
    .bind(trading_date)
    .fetch_one(pool)
    .await
    .expect("row count query should succeed")
}

// ---------------------------------------------------------------------------
// PDBC-01: unauthorized refusal (no DB required)
// ---------------------------------------------------------------------------

/// PDBC-01: `capture-account-equity-baseline` under `TokenRequired` auth
/// mode, without a valid bearer token, is refused with 401 before any gate
/// in the handler runs.
#[tokio::test]
async fn pdbc01_unauthorized_capture_is_refused() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired("real-secret".to_string()),
    ));
    let router = routes::build_router(Arc::clone(&st));

    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"test","trading_date":"2026-07-10"}"#;

    let (status, _) = call(router.clone(), post_json_auth("/api/v1/ops/action", body, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "PDBC-01: missing bearer must be 401");

    let (status, _) = call(
        router,
        post_json_auth("/api/v1/ops/action", body, Some("wrong-token")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "PDBC-01: wrong bearer must be 401");
}

// ---------------------------------------------------------------------------
// PDBC-02 / PDBC-03: no-DB / no-snapshot refusals (no DB connection required)
// ---------------------------------------------------------------------------

/// PDBC-02: no DB pool configured -> `db_unavailable`, 503, no row written
/// (nothing to check for a row since there is no pool at all).
#[tokio::test]
async fn pdbc02_no_db_is_refused_db_unavailable() {
    let (_st, router) = make_router_with_state();
    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"test","trading_date":"2026-07-10"}"#;
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let v = parse_json(resp_body);
    assert_eq!(v["accepted"], false);
    assert_eq!(v["disposition"], "db_unavailable");
    assert!(v["captured_baseline"].is_null());
}

/// PDBC-03: DB present, no broker snapshot -> `no_broker_snapshot`, 503.
#[tokio::test]
async fn pdbc03_no_broker_snapshot_is_refused() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-03: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"test","trading_date":"2026-07-10"}"#;
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let v = parse_json(resp_body);
    assert_eq!(v["accepted"], false);
    assert_eq!(v["disposition"], "no_broker_snapshot");
}

// ---------------------------------------------------------------------------
// PDBC-04..07: request-shape / calendar refusals (DB-backed, snapshot set)
// ---------------------------------------------------------------------------

async fn make_db_state_with_snapshot(pool: sqlx::PgPool) -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "101500.00",
        "50000.00",
        vec![position("ZZPDBC", "0", "0.00")],
    ));
    st
}

/// PDBC-04: blank `reason` -> `missing_reason`, 400.
#[tokio::test]
async fn pdbc04_blank_reason_is_refused() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let st = make_db_state_with_snapshot(pool).await;
    let router = routes::build_router(Arc::clone(&st));

    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"   ","trading_date":"2026-07-10"}"#;
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(parse_json(resp_body)["disposition"], "missing_reason");
}

/// PDBC-05: missing `trading_date` -> `missing_trading_date`, 400.
#[tokio::test]
async fn pdbc05_missing_trading_date_is_refused() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let st = make_db_state_with_snapshot(pool).await;
    let router = routes::build_router(Arc::clone(&st));

    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"test"}"#;
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(parse_json(resp_body)["disposition"], "missing_trading_date");
}

/// PDBC-06: malformed `trading_date` -> `invalid_trading_date`, 400.
#[tokio::test]
async fn pdbc06_malformed_trading_date_is_refused() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-06: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let st = make_db_state_with_snapshot(pool).await;
    let router = routes::build_router(Arc::clone(&st));

    let body = r#"{"action_key":"capture-account-equity-baseline","reason":"test","trading_date":"not-a-date"}"#;
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(parse_json(resp_body)["disposition"], "invalid_trading_date");
}

/// PDBC-07: `trading_date` is a real Saturday -> `non_trading_day`, 403.
#[tokio::test]
async fn pdbc07_weekend_trading_date_is_refused() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let st = make_db_state_with_snapshot(pool).await;
    let router = routes::build_router(Arc::clone(&st));

    let saturday = a_known_saturday();
    let body = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"test","trading_date":"{saturday}"}}"#
    );
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", &body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(parse_json(resp_body)["disposition"], "non_trading_day");
}

// ---------------------------------------------------------------------------
// PDBC-08..11: successful capture, idempotency, zero-order-writes, determinism
// ---------------------------------------------------------------------------

/// PDBC-08: a fully valid capture request writes exactly one row with
/// correct provenance and returns `applied` / `captured_baseline`.
#[tokio::test]
async fn pdbc08_valid_capture_writes_row_and_returns_provenance() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-08: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let trading_date = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, trading_date).await;

    let st = make_db_state_with_snapshot(pool.clone()).await;
    let router = routes::build_router(Arc::clone(&st));

    let body = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"PDBC-08 capture","trading_date":"{trading_date}"}}"#
    );
    let (status, resp_body) = call(router, post_json("/api/v1/ops/action", &body)).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(resp_body);
    assert_eq!(v["accepted"], true);
    assert_eq!(v["disposition"], "applied");
    let cb = &v["captured_baseline"];
    assert_eq!(cb["trading_date"], trading_date.to_string());
    assert_eq!(cb["equity"].as_f64().expect("equity present"), 101_500.0);
    assert_eq!(cb["cash"].as_f64().expect("cash present"), 50_000.0);
    assert_eq!(cb["currency"], "USD");
    assert_eq!(cb["captured_by"], "operator:capture-account-equity-baseline");
    assert!(cb["broker_snapshot_source"].is_string());
    assert!(cb["audit_event_id"].is_string());
    assert_eq!(
        v["audit"]["durable_targets"][0],
        "sys_account_equity_baseline"
    );
    assert_eq!(v["audit"]["durable_db_write"], true);

    let row_count = fetch_baseline_row_count(&pool, trading_date).await;
    assert_eq!(row_count, 1, "PDBC-08: exactly one row for trading_date");

    delete_baseline(&pool, trading_date).await;
}

/// PDBC-09: repeated capture for the same `trading_date` still yields
/// exactly one row (idempotent upsert), even when equity differs.
#[tokio::test]
async fn pdbc09_repeated_capture_same_date_is_idempotent() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-09: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let trading_date = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, trading_date).await;

    let st = make_db_state_with_snapshot(pool.clone()).await;
    let router = routes::build_router(Arc::clone(&st));

    let body1 = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"first","trading_date":"{trading_date}"}}"#
    );
    let (status1, _) = call(router.clone(), post_json("/api/v1/ops/action", &body1)).await;
    assert_eq!(status1, StatusCode::OK);

    *st.broker_snapshot.write().await = Some(make_snapshot(
        "202500.00",
        "60000.00",
        vec![position("ZZPDBC09", "0", "0.00")],
    ));
    let body2 = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"second","trading_date":"{trading_date}"}}"#
    );
    let (status2, resp_body2) = call(router, post_json("/api/v1/ops/action", &body2)).await;
    assert_eq!(status2, StatusCode::OK);
    let v2 = parse_json(resp_body2);
    assert_eq!(v2["captured_baseline"]["equity"].as_f64().unwrap(), 202_500.0);

    let row_count = fetch_baseline_row_count(&pool, trading_date).await;
    assert_eq!(row_count, 1, "PDBC-09: still exactly one row after re-capture");

    delete_baseline(&pool, trading_date).await;
}

/// PDBC-10: a successful capture call writes zero `oms_outbox`/`oms_inbox`
/// rows -- this action never touches order-lifecycle tables.
#[tokio::test]
async fn pdbc10_capture_writes_zero_outbox_inbox_rows() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-10: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let trading_date = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, trading_date).await;

    let outbox_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("outbox count before");
    let inbox_before: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(&pool)
        .await
        .expect("inbox count before");

    let st = make_db_state_with_snapshot(pool.clone()).await;
    let router = routes::build_router(Arc::clone(&st));
    let body = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"PDBC-10","trading_date":"{trading_date}"}}"#
    );
    let (status, _) = call(router, post_json("/api/v1/ops/action", &body)).await;
    assert_eq!(status, StatusCode::OK);

    let outbox_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("outbox count after");
    let inbox_after: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(&pool)
        .await
        .expect("inbox count after");

    assert_eq!(outbox_before, outbox_after, "PDBC-10: oms_outbox row count must be unchanged");
    assert_eq!(inbox_before, inbox_after, "PDBC-10: oms_inbox row count must be unchanged");

    delete_baseline(&pool, trading_date).await;
}

/// PDBC-11: identical inputs (same trading_date, same broker snapshot
/// values) produce the same deterministic `audit_event_id` across two
/// independent captures of two different dates seeded with identical
/// equity/cash -- proven indirectly by capturing the *same* date twice in
/// a row with the *same* snapshot each time and asserting the ID is
/// unchanged (a true no-op re-run of the same logical event).
#[tokio::test]
async fn pdbc11_deterministic_audit_event_id_reproducible() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDBC-11: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;
    let trading_date = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, trading_date).await;

    let st = make_db_state_with_snapshot(pool.clone()).await;
    let router = routes::build_router(Arc::clone(&st));

    let body = format!(
        r#"{{"action_key":"capture-account-equity-baseline","reason":"PDBC-11","trading_date":"{trading_date}"}}"#
    );
    let (status1, resp1) = call(router.clone(), post_json("/api/v1/ops/action", &body)).await;
    assert_eq!(status1, StatusCode::OK);
    let id1 = parse_json(resp1)["captured_baseline"]["audit_event_id"]
        .as_str()
        .expect("id1 present")
        .to_string();

    let (status2, resp2) = call(router, post_json("/api/v1/ops/action", &body)).await;
    assert_eq!(status2, StatusCode::OK);
    let id2 = parse_json(resp2)["captured_baseline"]["audit_event_id"]
        .as_str()
        .expect("id2 present")
        .to_string();

    assert_eq!(id1, id2, "PDBC-11: identical inputs must reproduce the same audit_event_id");

    delete_baseline(&pool, trading_date).await;
}
