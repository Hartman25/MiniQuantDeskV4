//! PORTFOLIO-LIVE-WEIGHTS-01: `GET /api/v1/portfolio/live-weights` proof tests.
//!
//! Proves that:
//! - No execution snapshot → truth_state="no_snapshot", all financial fields null.
//! - A snapshot with cash and zero/flat positions is "active" with NAV == cash,
//!   even with no DB pool configured (a flat portfolio never needs a mark).
//! - A snapshot with a non-flat position and no DB pool → "db_unavailable",
//!   distinct from "missing_marks" (DB present but no bar for the symbol).
//! - `timeframe` query param is echoed; defaults to "1D" when absent.
//! - The route never mutates `execution_snapshot` or writes to `oms_outbox`.
//! - DB-backed: a seeded completed md_bars row produces a correct, truthful
//!   mark/NAV/weight; an unseeded symbol produces "missing_marks".
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                        |
//! |---------|------------------------------------------------------------------------|
//! | PLW-01  | No snapshot → 200, truth_state="no_snapshot", empty/null fields       |
//! | PLW-02  | Snapshot, no positions, no DB → "active", NAV == cash                 |
//! | PLW-03  | Snapshot, non-flat position, no DB → "db_unavailable"                 |
//! | PLW-04  | ?timeframe=1m is echoed in the response                               |
//! | PLW-05  | No timeframe param → default "1D" echoed                              |
//! | PLW-06  | execution_snapshot is unchanged after the call (read-only safety)     |
//! | PLW-07  | DB-backed: seeded bar → "active" with correct mark/NAV/weight;        |
//! |         | oms_outbox row count unchanged (no order/outbox writes)               |
//! | PLW-08  | DB-backed: unseeded symbol with DB present → "missing_marks"          |
//!
//! All non-DB tests are fully in-process (no DB, no disk I/O, no network).
//! DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing at the
//! local paper DB (port 5440 / miniquantdesk_paper), matching the convention
//! in `scenario_data_freshness_readiness_gate_01.rs` /
//! `scenario_backtest_db_bars_source_01.rs`. No broker adapter is ever called.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
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

fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("key '{}' must be a string in: {}", key, v))
}

fn get_live_weights(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn make_snapshot(cash_micros: i64, positions: &[(&str, i64)]) -> ExecutionSnapshot {
    ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros,
            realized_pnl_micros: 0,
            positions: positions
                .iter()
                .map(|(sym, qty)| PositionSnapshot {
                    symbol: sym.to_string(),
                    net_qty: *qty,
                })
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// PLW-01: no snapshot → "no_snapshot"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plw01_no_snapshot_returns_no_snapshot_truth_state() {
    let (_st, router) = make_router_with_state();
    let (status, body) = call(router, get_live_weights("/api/v1/portfolio/live-weights")).await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(json_str(&v, "truth_state"), "no_snapshot");
    assert!(v["nav_micros"].is_null());
    assert!(v["gross_exposure_micros"].is_null());
    assert_eq!(
        v["positions"].as_array().map(|a| a.len()).unwrap_or(1),
        0,
        "positions must be empty when there is no snapshot"
    );
    assert_eq!(json_str(&v, "timeframe"), "1D", "default timeframe is 1D");
}

// ---------------------------------------------------------------------------
// PLW-02: snapshot, no positions, no DB → "active", NAV == cash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plw02_no_positions_is_active_without_db() {
    let (st, router) = make_router_with_state();
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[]));

    let (status, body) = call(router, get_live_weights("/api/v1/portfolio/live-weights")).await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(
        json_str(&v, "truth_state"),
        "active",
        "a flat/no-position portfolio never needs a mark, even without a DB"
    );
    assert_eq!(v["nav_micros"], 500_000_000);
    assert_eq!(v["gross_exposure_micros"], 0);
    assert_eq!(v["cash_micros"], 500_000_000);
}

// ---------------------------------------------------------------------------
// PLW-03: snapshot, non-flat position, no DB → "db_unavailable"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plw03_non_flat_position_without_db_is_db_unavailable() {
    let (st, router) = make_router_with_state();
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("AAPL", 10)]));

    let (status, body) = call(router, get_live_weights("/api/v1/portfolio/live-weights")).await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(
        json_str(&v, "truth_state"),
        "db_unavailable",
        "a non-flat position with no DB pool cannot be marked at all"
    );
    assert!(v["nav_micros"].is_null());
    let missing = v["missing_mark_symbols"]
        .as_array()
        .expect("missing_mark_symbols must be an array");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].as_str(), Some("AAPL"));
}

// ---------------------------------------------------------------------------
// PLW-04 / PLW-05: timeframe echo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plw04_timeframe_param_is_echoed() {
    let (_st, router) = make_router_with_state();
    let (status, body) = call(
        router,
        get_live_weights("/api/v1/portfolio/live-weights?timeframe=1m"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(json_str(&v, "timeframe"), "1m");
}

#[tokio::test]
async fn plw05_missing_timeframe_param_defaults_to_1d() {
    let (_st, router) = make_router_with_state();
    let (status, body) = call(router, get_live_weights("/api/v1/portfolio/live-weights")).await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(json_str(&v, "timeframe"), "1D");
}

// ---------------------------------------------------------------------------
// PLW-06: execution_snapshot unaffected (read-only safety)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plw06_execution_snapshot_unchanged_after_call() {
    let (st, router) = make_router_with_state();
    let snapshot = make_snapshot(100_000_000, &[("AAPL", 10)]);
    st.execution_snapshot
        .write()
        .await
        .replace(snapshot.clone());

    let (status, _body) = call(router, get_live_weights("/api/v1/portfolio/live-weights")).await;
    assert_eq!(status, StatusCode::OK);

    let after = st.execution_snapshot.read().await.clone();
    assert_eq!(
        after.map(|s| s.portfolio.cash_micros),
        Some(snapshot.portfolio.cash_micros),
        "live-weights route must not mutate execution_snapshot"
    );
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

/// PLW-07: seeded completed bar → "active" with correct mark/NAV/weight, and
/// the route makes zero writes to `oms_outbox`.
#[tokio::test]
async fn plw07_db_seeded_bar_produces_correct_active_weights_and_no_outbox_writes() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PLW-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PLW-07: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPLW01TST";
    delete_test_bars(&pool, symbol).await;
    insert_test_bar(&pool, symbol, "1D", 1_790_000_000, 25_000_000).await; // $25/share close

    let outbox_count_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox before");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[(symbol, 4)])); // cash=0, qty=4 => NAV == market value exactly
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(
        router,
        get_live_weights("/api/v1/portfolio/live-weights?timeframe=1D"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let v = parse_json(body);
    assert_eq!(json_str(&v, "truth_state"), "active");
    assert_eq!(v["nav_micros"], 100_000_000); // 4 * 25_000_000
    assert_eq!(v["gross_exposure_micros"], 100_000_000);

    let rows = v["positions"].as_array().expect("positions array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["symbol"], symbol);
    assert_eq!(row["mark_price_micros"], 25_000_000);
    assert_eq!(row["market_value_micros"], 100_000_000);
    assert_eq!(row["weight_bps"], 10000); // single position == 100% of NAV
    assert_eq!(row["missing_mark"], false);
    assert_eq!(row["mark_source"], "md_bars:1D:close");

    let outbox_count_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox after");
    assert_eq!(
        outbox_count_before, outbox_count_after,
        "live-weights must never write to oms_outbox"
    );

    delete_test_bars(&pool, symbol).await;
}

/// PLW-08: DB present but the symbol has no completed bar → "missing_marks",
/// distinct from "db_unavailable" (PLW-03, no DB at all).
#[tokio::test]
async fn plw08_db_present_but_unseeded_symbol_is_missing_marks() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PLW-08: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PLW-08: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZPLW01NOBAR";
    delete_test_bars(&pool, symbol).await; // ensure no bars exist for this symbol

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[(symbol, 1)]));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(
        router,
        get_live_weights("/api/v1/portfolio/live-weights?timeframe=1D"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let v = parse_json(body);
    assert_eq!(
        json_str(&v, "truth_state"),
        "missing_marks",
        "DB is present but this symbol has zero completed bars — distinct from db_unavailable"
    );
    let missing = v["missing_mark_symbols"]
        .as_array()
        .expect("missing_mark_symbols must be an array");
    assert_eq!(missing[0].as_str(), Some(symbol));
}
