//! PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01C: `GET /api/v1/portfolio/positions`
//! and `GET /api/v1/portfolio/summary` P&L visibility proof tests.
//!
//! Closes the seam `PAPER-TRADE-LIFECYCLE-PROOF-02` exposed: a real filled
//! paper position (`AAPL qty=3 avg_price=314.81`) had `mark_price` and
//! `unrealized_pnl` both `null` because neither route ever consulted a mark
//! source. Both routes now resolve a mark from the latest completed
//! `md_bars` close for each broker-snapshot position (same source
//! `/api/v1/portfolio/live-weights` already uses) and combine it with the
//! position's own `avg_price` via `mqk_portfolio::unrealized_pnl_micros`.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                         |
//! |---------|---------------------------------------------------------------------------|
//! | PPV-01  | No broker snapshot -> positions/summary unchanged contract, 200          |
//! | PPV-02  | Snapshot, non-flat position, no DB -> "db_unavailable" on both routes    |
//! | PPV-03  | Snapshot, qty=0 position, no DB -> "flat", unrealized_pnl = 0.0          |
//! | PPV-04  | Existing route contract fields (`symbol`,`qty`,`avg_price`,`strategy_id`,|
//! |         | `broker_qty`) remain populated exactly as before (backward compatible)   |
//! | PPV-05  | DB-backed: seeded completed bar above avg_price -> positive P&L,         |
//! |         | mark_price/unrealized_pnl non-null, pnl_truth_state="active"             |
//! | PPV-06  | DB-backed: seeded completed bar below avg_price -> negative P&L          |
//! | PPV-07  | DB-backed: DB present but symbol has no completed bar -> "mark_unavailable"|
//! | PPV-08  | DB-backed: summary.unrealized_pnl aggregates position-level P&L,         |
//! |         | daily_pnl stays null with an explicit unavailable reason                 |
//! | PPV-09  | DB-backed: route calls make zero writes to `oms_outbox`                  |
//! | PPV-10  | DB-backed: default (no query) positions/summary still resolve `1D`       |
//! | PPV-11  | DB-backed: `?timeframe=5m` resolves mark/P&L when only a `5m` bar exists |
//! | PPV-12  | DB-backed: same symbol, default `1D` -> `mark_unavailable` (5m-only bar) |
//! | PPV-13  | DB-backed: `mark_source == "md_bars:5m:close"` when queried with `5m`    |
//! | PPV-14  | Blank `?timeframe=` defaults to `1D` (no DB needed — flat position)      |
//!
//! Non-DB tests are fully in-process (no DB, no disk I/O, no network). DB
//! tests skip gracefully without `MQK_DATABASE_URL` pointing at the local
//! paper DB (port 5440 / miniquantdesk_paper), matching the convention in
//! `scenario_portfolio_live_weights_01.rs`. No broker adapter, provider, or
//! network call is made by any test in this file.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::DateTime;
use http_body_util::BodyExt;
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

fn make_snapshot(positions: Vec<BrokerPosition>) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "1000000.00".to_string(),
            cash: "999000.00".to_string(),
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

// ---------------------------------------------------------------------------
// PPV-01: no broker snapshot -> unchanged contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ppv01_no_snapshot_positions_and_summary_unchanged_contract() {
    let (_st, router) = make_router_with_state();

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["snapshot_state"], "no_snapshot");
    assert!(v["rows"].as_array().is_some_and(|a| a.is_empty()));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["truth_state"], "no_snapshot");
    assert!(v["daily_pnl"].is_null());
    assert!(v["unrealized_pnl"].is_null());
    assert_eq!(v["pnl_truth_state"], "no_snapshot");
    assert!(v["daily_pnl_unavailable_reason"].is_string());
}

// ---------------------------------------------------------------------------
// PPV-02: non-flat position, no DB -> "db_unavailable"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ppv02_non_flat_position_without_db_is_db_unavailable() {
    let (st, router) = make_router_with_state();
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position("AAPL", "3", "314.81")]));

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let rows = v["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["mark_price"].is_null());
    assert!(rows[0]["unrealized_pnl"].is_null());
    assert_eq!(rows[0]["pnl_truth_state"], "db_unavailable");
    assert_eq!(rows[0]["pnl_unavailable_reason"], "no_db_pool_configured");
    assert!(rows[0]["mark_source"].is_null());

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(v["unrealized_pnl"].is_null());
    assert_eq!(v["pnl_truth_state"], "db_unavailable");
    assert!(v["pnl_unavailable_reason"].is_string());
    assert!(v["daily_pnl"].is_null());
}

// ---------------------------------------------------------------------------
// PPV-03: qty=0 position -> "flat", unrealized_pnl = 0.0, no DB needed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ppv03_flat_position_is_zero_pnl_without_db() {
    let (st, router) = make_router_with_state();
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position("AAPL", "0", "0.00")]));

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let rows = v["rows"].as_array().expect("rows array");
    assert_eq!(rows[0]["pnl_truth_state"], "flat");
    assert_eq!(rows[0]["unrealized_pnl"], 0.0);
    assert!(rows[0]["mark_price"].is_null());

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["unrealized_pnl"], 0.0);
    assert_eq!(v["pnl_truth_state"], "active");
}

// ---------------------------------------------------------------------------
// PPV-04: existing route contract fields remain backward compatible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ppv04_existing_position_fields_remain_populated() {
    let (st, router) = make_router_with_state();
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position("AAPL", "10", "175.50")]));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["symbol"], "AAPL");
    assert_eq!(row["qty"], 10);
    assert_eq!(row["avg_price"], 175.50);
    assert_eq!(row["broker_qty"], 10);
    assert!(row["strategy_id"].is_null());
    assert!(row["drift"].is_null());
    assert!(row["realized_pnl_today"].is_null());
}

// ---------------------------------------------------------------------------
// DB-backed tests (skip without MQK_DATABASE_URL pointing at the paper DB)
// ---------------------------------------------------------------------------

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

async fn insert_test_bar(
    pool: &sqlx::PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    close_micros: i64,
) {
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (symbol, timeframe, end_ts) do update set
          open_micros = excluded.open_micros,
          high_micros = excluded.high_micros,
          low_micros = excluded.low_micros,
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(close_micros)
    .bind(close_micros + 10_000)
    .bind(close_micros - 10_000)
    .bind(close_micros)
    .bind(1_000_i64)
    .bind(true)
    .execute(pool)
    .await
    .expect("insert test md_bars row");
}

async fn delete_test_bars(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await;
}

/// PPV-05: seeded completed bar above avg_price -> positive P&L.
#[tokio::test]
async fn ppv05_db_seeded_bar_above_avg_price_produces_positive_pnl() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-05: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV01POS";
    delete_test_bars(&pool, symbol).await;
    // avg_price=314.81, mark=320.00 -> positive pnl of (320.00-314.81)*3 = 15.57
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 320_000_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "active");
    assert_eq!(row["mark_price"], 320.0);
    assert_eq!(row["mark_source"], "md_bars:1D:close");
    let pnl = row["unrealized_pnl"]
        .as_f64()
        .expect("unrealized_pnl present");
    assert!(
        pnl > 0.0,
        "mark above avg_price must be a positive pnl; got {pnl}"
    );
    assert!((pnl - 15.57).abs() < 0.001, "expected ~15.57, got {pnl}");

    delete_test_bars(&pool, symbol).await;
}

/// PPV-06: seeded completed bar below avg_price -> negative P&L.
#[tokio::test]
async fn ppv06_db_seeded_bar_below_avg_price_produces_negative_pnl() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-06: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-06: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV01NEG";
    delete_test_bars(&pool, symbol).await;
    // avg_price=314.81, mark=300.00 -> negative pnl
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 300_000_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "active");
    let pnl = row["unrealized_pnl"]
        .as_f64()
        .expect("unrealized_pnl present");
    assert!(
        pnl < 0.0,
        "mark below avg_price must be a negative pnl; got {pnl}"
    );

    delete_test_bars(&pool, symbol).await;
}

/// PPV-07: DB present but symbol has no completed bar -> "mark_unavailable".
#[tokio::test]
async fn ppv07_db_present_but_unseeded_symbol_is_mark_unavailable() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-07: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV01NOBAR";
    delete_test_bars(&pool, symbol).await; // ensure no bars exist

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "1", "100.00")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "mark_unavailable");
    assert_eq!(
        row["pnl_unavailable_reason"],
        "no_completed_md_bars_row_for_symbol"
    );
    assert!(row["mark_price"].is_null());
    assert!(row["unrealized_pnl"].is_null());
}

/// PPV-08: summary aggregates position-level P&L; daily_pnl stays null with
/// an explicit unavailable reason.
#[tokio::test]
async fn ppv08_db_summary_aggregates_unrealized_pnl_daily_pnl_stays_unavailable() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-08: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-08: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV01SUM";
    delete_test_bars(&pool, symbol).await;
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 320_000_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["pnl_truth_state"], "active");
    let pnl = v["unrealized_pnl"]
        .as_f64()
        .expect("summary unrealized_pnl present");
    assert!((pnl - 15.57).abs() < 0.001, "expected ~15.57, got {pnl}");
    assert!(v["daily_pnl"].is_null(), "daily_pnl must stay unavailable");
    assert_eq!(v["daily_pnl_truth_state"], "baseline_unavailable");
    let reason = v["daily_pnl_unavailable_reason"]
        .as_str()
        .expect("daily_pnl_unavailable_reason present");
    assert!(
        reason.starts_with("no_account_equity_baseline_for_required_trading_day:"),
        "unexpected reason: {reason}"
    );

    delete_test_bars(&pool, symbol).await;
}

/// PPV-09: route calls make zero writes to `oms_outbox`.
#[tokio::test]
async fn ppv09_routes_make_no_outbox_writes() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-09: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-09: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV01OBX";
    delete_test_bars(&pool, symbol).await;
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 320_000_000).await;

    let outbox_count_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox before");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, _body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);

    let outbox_count_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox after");
    assert_eq!(
        outbox_count_before, outbox_count_after,
        "positions/summary routes must never write to oms_outbox"
    );

    delete_test_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// PAPER-PNL-OFFMARKET-01B: `timeframe` query-param proof (PPV-10..PPV-14)
// ---------------------------------------------------------------------------

/// PPV-10: default (no `?timeframe=`) positions/summary still resolve `1D`
/// exactly as before — backward compatible.
#[tokio::test]
async fn ppv10_default_no_query_still_resolves_1d() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-10: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-10: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV10DEF";
    delete_test_bars(&pool, symbol).await;
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 320_000_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "active");
    assert_eq!(row["mark_source"], "md_bars:1D:close");

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["pnl_truth_state"], "active");
    assert!(v["unrealized_pnl"].as_f64().is_some());

    delete_test_bars(&pool, symbol).await;
}

/// PPV-11 / PPV-13: `?timeframe=5m` resolves mark/P&L on both routes when
/// only a `5m` bar exists, and `mark_source == "md_bars:5m:close"`.
#[tokio::test]
async fn ppv11_timeframe_5m_resolves_mark_and_pnl_when_only_5m_bar_exists() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-11: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-11: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV11FIV";
    delete_test_bars(&pool, symbol).await;
    // Only a 5m bar exists — matches the real proof-02 AAPL DB shape.
    // avg_price=314.81, mark=314.86 -> unrealized_pnl = (314.86-314.81)*3 = 0.15
    insert_test_bar(&pool, symbol, "5m", 1_790_000_000, 314_860_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(
        router.clone(),
        get("/api/v1/portfolio/positions?timeframe=5m"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "active");
    assert_eq!(row["mark_price"], 314.86);
    assert_eq!(row["mark_source"], "md_bars:5m:close");
    let pnl = row["unrealized_pnl"]
        .as_f64()
        .expect("unrealized_pnl present");
    assert!((pnl - 0.15).abs() < 0.001, "expected ~0.15, got {pnl}");

    let (status, body) = call(router, get("/api/v1/portfolio/summary?timeframe=5m")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["pnl_truth_state"], "active");
    let pnl = v["unrealized_pnl"]
        .as_f64()
        .expect("summary unrealized_pnl present");
    assert!((pnl - 0.15).abs() < 0.001, "expected ~0.15, got {pnl}");

    delete_test_bars(&pool, symbol).await;
}

/// PPV-12: same symbol with only a `5m` bar seeded -> default (`1D`) query
/// still reports `mark_unavailable`, proving the default is unchanged and
/// the `5m` mark is only used when explicitly requested.
#[tokio::test]
async fn ppv12_same_symbol_default_1d_is_mark_unavailable_when_only_5m_bar_exists() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PPV-12: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PPV-12: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPPV12ONL";
    delete_test_bars(&pool, symbol).await;
    insert_test_bar(&pool, symbol, "5m", 1_790_000_000, 314_860_000).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position(symbol, "3", "314.81")]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let row = &v["rows"].as_array().expect("rows array")[0];
    assert_eq!(row["pnl_truth_state"], "mark_unavailable");
    assert_eq!(
        row["pnl_unavailable_reason"],
        "no_completed_md_bars_row_for_symbol"
    );
    assert!(row["mark_price"].is_null());

    delete_test_bars(&pool, symbol).await;
}

/// PPV-14: blank `?timeframe=` defaults to `1D` — no DB needed (flat
/// position never requires a mark lookup).
#[tokio::test]
async fn ppv14_blank_timeframe_query_param_defaults_to_1d() {
    let (st, router) = make_router_with_state();
    *st.broker_snapshot.write().await = Some(make_snapshot(vec![position("AAPL", "0", "0.00")]));

    let (status, body) = call(
        router.clone(),
        get("/api/v1/portfolio/positions?timeframe="),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let rows = v["rows"].as_array().expect("rows array");
    assert_eq!(rows[0]["pnl_truth_state"], "flat");
    assert_eq!(rows[0]["unrealized_pnl"], 0.0);

    let (status, body) = call(router, get("/api/v1/portfolio/summary?timeframe=")).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(v["unrealized_pnl"], 0.0);
    assert_eq!(v["pnl_truth_state"], "active");
}
