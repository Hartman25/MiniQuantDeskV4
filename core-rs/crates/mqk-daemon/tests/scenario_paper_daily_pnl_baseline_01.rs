//! PAPER-DAILY-PNL-BASELINE-01C: `GET /api/v1/portfolio/summary.daily_pnl`
//! previous-session-close baseline read-side proof.
//!
//! Proves `daily_pnl` becomes computable only when a real
//! `sys_account_equity_baseline` row exists for the required prior trading
//! day, and stays honestly unavailable (with an explicit
//! `daily_pnl_truth_state` + reason) when no snapshot, no DB, or no
//! baseline row exists — never fabricated, never inferred from marks.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                    |
//! |---------|--------------------------------------------------------------------|
//! | PDB-01  | No broker snapshot -> `daily_pnl_truth_state = "no_snapshot"`     |
//! | PDB-02  | Snapshot, no DB -> `daily_pnl_truth_state = "db_unavailable"`     |
//! | PDB-03  | DB present, no baseline row -> `"baseline_unavailable"`           |
//! | PDB-04  | Baseline row for required prior trading day, equity above it ->   |
//! |         | positive `daily_pnl`, `"active"`, baseline provenance populated    |
//! | PDB-05  | Baseline row present, equity below it -> negative `daily_pnl`     |
//! | PDB-06  | Baseline row present, equity equal to it -> `daily_pnl == 0.0`    |
//! | PDB-07  | A baseline row older than the required prior trading day ->       |
//! |         | `"stale_baseline"`, `daily_pnl` stays null                         |
//! | PDB-08  | Route calls never insert/update/delete any baseline row           |
//! | PDB-09  | `unrealized_pnl` / `pnl_truth_state` behavior is unchanged when a  |
//! |         | baseline row is also present (fields are independent)             |
//! | PDB-10  | Default (no query) and `?timeframe=5m` daily_pnl behavior is      |
//! |         | unaffected by the `timeframe` query param (`daily_pnl` only        |
//! |         | depends on account equity + baseline, not on position marks)       |
//!
//! All DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing
//! at the local paper DB (port 5440 / miniquantdesk_paper), matching the
//! convention in `scenario_paper_pnl_operator_visibility_01.rs`. No
//! provider, broker, or network call in any test. No order submission.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::state::{MarketCalendarProvider, NyseWeekdaysProvider};
use mqk_daemon::{routes, state};
use mqk_db::{upsert_account_equity_baseline, UpsertAccountEquityBaselineArgs};
use mqk_schemas::{BrokerAccount, BrokerPosition, BrokerSnapshot};
use tower::ServiceExt;
use uuid::Uuid;

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

/// Independent reimplementation of the route's `most_recent_trading_day_before`
/// using the same public `NyseWeekdaysProvider` seam, so tests can compute
/// which `trading_date` to seed without depending on the route's private
/// helper. Mirrors `core-rs/crates/mqk-daemon/src/routes/portfolio.rs`.
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

async fn seed_baseline(pool: &sqlx::PgPool, trading_date: NaiveDate, equity_micros: i64, tag: &str) {
    let audit_event_id = Uuid::new_v4();
    upsert_account_equity_baseline(
        pool,
        UpsertAccountEquityBaselineArgs {
            trading_date,
            equity_micros,
            cash_micros: equity_micros / 2,
            currency: "USD".to_string(),
            captured_at_utc: trading_date
                .and_hms_opt(21, 0, 0)
                .expect("valid time")
                .and_utc(),
            captured_by: format!("test:{tag}"),
            broker_snapshot_source: "synthetic".to_string(),
            audit_event_id,
        },
    )
    .await
    .expect("seed baseline upsert should succeed");
}

async fn delete_baseline(pool: &sqlx::PgPool, trading_date: NaiveDate) {
    let _ = sqlx::query("delete from sys_account_equity_baseline where trading_date = $1")
        .bind(trading_date)
        .execute(pool)
        .await;
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

// ---------------------------------------------------------------------------
// PDB-01 / PDB-02: no-snapshot / no-DB (no DB connection required)
// ---------------------------------------------------------------------------

/// PDB-01: no broker snapshot -> `daily_pnl_truth_state = "no_snapshot"`.
#[tokio::test]
async fn pdb01_no_snapshot_is_no_snapshot_truth_state() {
    let (_st, router) = make_router_with_state();
    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(v["daily_pnl"].is_null());
    assert_eq!(v["daily_pnl_truth_state"], "no_snapshot");
    assert_eq!(v["daily_pnl_unavailable_reason"], "no_broker_snapshot");
    assert!(v["daily_pnl_baseline_trading_date"].is_null());
    assert!(v["daily_pnl_baseline_equity"].is_null());
}

/// PDB-02: snapshot present, no DB pool configured -> `"db_unavailable"`.
#[tokio::test]
async fn pdb02_snapshot_no_db_is_db_unavailable_truth_state() {
    let (st, router) = make_router_with_state();
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "100000.00",
        "50000.00",
        vec![position("ZZPDB02", "0", "0.00")],
    ));
    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(v["daily_pnl"].is_null());
    assert_eq!(v["daily_pnl_truth_state"], "db_unavailable");
    assert_eq!(v["daily_pnl_unavailable_reason"], "no_db_pool_configured");
}

// ---------------------------------------------------------------------------
// DB-backed tests (skip without MQK_DATABASE_URL pointing at the paper DB)
// ---------------------------------------------------------------------------

/// PDB-03: DB present, no baseline row for the required prior trading day
/// (and none older either) -> `"baseline_unavailable"`.
#[tokio::test]
async fn pdb03_db_present_no_baseline_row_is_baseline_unavailable() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-03: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    // Make sure no residual baseline row exists in the lookback window.
    let required = expected_required_trading_day(Utc::now());
    let mut probe = required;
    for _ in 0..31 {
        delete_baseline(&pool, probe).await;
        let Some(p) = probe.pred_opt() else { break };
        probe = p;
    }

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "100000.00",
        "50000.00",
        vec![position("ZZPDB03", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(v["daily_pnl"].is_null());
    assert_eq!(v["daily_pnl_truth_state"], "baseline_unavailable");
    let reason = v["daily_pnl_unavailable_reason"]
        .as_str()
        .expect("reason present");
    assert!(reason.starts_with("no_account_equity_baseline_for_required_trading_day:"));
}

/// PDB-04: baseline row for the required prior trading day, current equity
/// above it -> positive `daily_pnl`, `"active"`, baseline fields populated.
#[tokio::test]
async fn pdb04_baseline_present_equity_above_is_positive_daily_pnl() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 100_000_000_000, "pdb04").await; // $100,000.00

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "101500.00",
        "50000.00",
        vec![position("ZZPDB04", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["daily_pnl_truth_state"], "active");
    assert!(v["daily_pnl_unavailable_reason"].is_null());
    let daily_pnl = v["daily_pnl"].as_f64().expect("daily_pnl present");
    assert!((daily_pnl - 1500.0).abs() < 0.001, "expected 1500.0, got {daily_pnl}");
    assert_eq!(v["daily_pnl_baseline_trading_date"], required.to_string());
    assert_eq!(
        v["daily_pnl_baseline_equity"].as_f64().expect("baseline equity present"),
        100_000.0
    );
    assert_eq!(v["daily_pnl_baseline_source"], "test:pdb04");
    assert!(v["daily_pnl_baseline_captured_at_utc"].is_string());

    delete_baseline(&pool, required).await;
}

/// PDB-05: baseline row present, current equity below it -> negative
/// `daily_pnl`.
#[tokio::test]
async fn pdb05_baseline_present_equity_below_is_negative_daily_pnl() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 100_000_000_000, "pdb05").await; // $100,000.00

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "98200.00",
        "50000.00",
        vec![position("ZZPDB05", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["daily_pnl_truth_state"], "active");
    let daily_pnl = v["daily_pnl"].as_f64().expect("daily_pnl present");
    assert!(
        (daily_pnl - (-1800.0)).abs() < 0.001,
        "expected -1800.0, got {daily_pnl}"
    );

    delete_baseline(&pool, required).await;
}

/// PDB-06: baseline row present, current equity exactly equal to it ->
/// `daily_pnl == 0.0` (not null, not "unavailable").
#[tokio::test]
async fn pdb06_baseline_present_equity_equal_is_zero_daily_pnl() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-06: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 100_000_000_000, "pdb06").await; // $100,000.00

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "100000.00",
        "50000.00",
        vec![position("ZZPDB06", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["daily_pnl_truth_state"], "active");
    let daily_pnl = v["daily_pnl"].as_f64().expect("daily_pnl present, not null");
    assert!(daily_pnl.abs() < 0.001, "expected 0.0, got {daily_pnl}");

    delete_baseline(&pool, required).await;
}

/// PDB-07: a baseline row exists, but only for a date strictly older than
/// the required prior trading day -> `"stale_baseline"`, `daily_pnl` stays
/// null (never silently used as if it were the correct baseline).
#[tokio::test]
async fn pdb07_older_baseline_only_is_stale_baseline() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;

    // Seed a row 5 calendar days before the required date instead — this is
    // never itself a required date, so its trading_date-ness doesn't matter
    // for this proof, only that it predates `required`.
    let stale_date = required - chrono::Duration::days(5);
    delete_baseline(&pool, stale_date).await;
    seed_baseline(&pool, stale_date, 90_000_000_000, "pdb07_stale").await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "95000.00",
        "50000.00",
        vec![position("ZZPDB07", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(v["daily_pnl"].is_null(), "stale baseline must never be silently used");
    assert_eq!(v["daily_pnl_truth_state"], "stale_baseline");
    assert_eq!(v["daily_pnl_baseline_trading_date"], stale_date.to_string());
    let reason = v["daily_pnl_unavailable_reason"]
        .as_str()
        .expect("reason present");
    assert!(reason.contains(&stale_date.to_string()));
    assert!(reason.contains(&required.to_string()));

    delete_baseline(&pool, stale_date).await;
}

/// PDB-08: repeated route calls never insert/update/delete any baseline
/// row — read-only lookup, proven by an unchanged row count and unchanged
/// row content across two calls.
#[tokio::test]
async fn pdb08_route_calls_never_write_baseline_rows() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-08: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 77_000_000_000, "pdb08").await;

    let count_before: i64 = sqlx::query_scalar("select count(*) from sys_account_equity_baseline")
        .fetch_one(&pool)
        .await
        .expect("count before");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "78000.00",
        "40000.00",
        vec![position("ZZPDB08", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, _body) = call(router.clone(), get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);

    let count_after: i64 = sqlx::query_scalar("select count(*) from sys_account_equity_baseline")
        .fetch_one(&pool)
        .await
        .expect("count after");
    assert_eq!(count_before, count_after, "route must never write a baseline row");

    let row_captured_by: String = sqlx::query_scalar(
        "select captured_by from sys_account_equity_baseline where trading_date = $1",
    )
    .bind(required)
    .fetch_one(&pool)
    .await
    .expect("row must be unchanged");
    assert_eq!(row_captured_by, "test:pdb08", "row content must be unchanged");

    delete_baseline(&pool, required).await;
}

/// PDB-09: `unrealized_pnl` / `pnl_truth_state` are unaffected by
/// `daily_pnl` baseline presence — the two P&L surfaces are independent.
#[tokio::test]
async fn pdb09_unrealized_pnl_unaffected_by_daily_pnl_baseline() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-09: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 100_000_000_000, "pdb09").await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Flat position: unrealized_pnl is always exactly 0.0/"flat"/"active"
    // regardless of any baseline row — proves the two surfaces don't leak
    // into each other.
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "101000.00",
        "50000.00",
        vec![position("ZZPDB09", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["pnl_truth_state"], "active");
    assert_eq!(v["unrealized_pnl"].as_f64().unwrap(), 0.0);
    assert_eq!(v["daily_pnl_truth_state"], "active");
    assert!(v["daily_pnl"].as_f64().is_some());

    delete_baseline(&pool, required).await;
}

/// PDB-10: `daily_pnl` behavior is identical for the default timeframe and
/// `?timeframe=5m` — it depends only on account equity + baseline, never
/// on the position-mark timeframe.
#[tokio::test]
async fn pdb10_daily_pnl_unaffected_by_timeframe_query_param() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PDB-10: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connected_pool(&db_url).await;

    let required = expected_required_trading_day(Utc::now());
    delete_baseline(&pool, required).await;
    seed_baseline(&pool, required, 100_000_000_000, "pdb10").await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(
        "100250.00",
        "50000.00",
        vec![position("ZZPDB10", "0", "0.00")],
    ));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let default_v = parse_json(body);

    let (status, body) = call(router, get("/api/v1/portfolio/summary?timeframe=5m")).await;
    assert_eq!(status, StatusCode::OK);
    let tf5m_v = parse_json(body);

    assert_eq!(default_v["daily_pnl"], tf5m_v["daily_pnl"]);
    assert_eq!(
        default_v["daily_pnl_truth_state"],
        tf5m_v["daily_pnl_truth_state"]
    );
    assert_eq!(default_v["daily_pnl_truth_state"], "active");

    delete_baseline(&pool, required).await;
}

// ---------------------------------------------------------------------------
// Weekend previous-trading-day sanity (uses the real current date, so this
// only exercises the weekend-skip path when the test happens to run on a
// day whose required prior trading day is a Friday; it always at least
// proves `expected_required_trading_day` never returns a Saturday/Sunday).
// ---------------------------------------------------------------------------

#[test]
fn weekend_skip_helper_never_returns_a_weekend_date() {
    let now = Utc.with_ymd_and_hms(2026, 7, 13, 12, 0, 0).unwrap(); // Monday
    let required = expected_required_trading_day(now);
    assert_eq!(
        required.weekday().to_string(),
        "Fri",
        "the trading day before Monday must be the preceding Friday"
    );

    let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(); // Tuesday
    let required = expected_required_trading_day(now);
    assert_eq!(
        required.weekday().to_string(),
        "Mon",
        "the trading day before Tuesday must be Monday"
    );
}
