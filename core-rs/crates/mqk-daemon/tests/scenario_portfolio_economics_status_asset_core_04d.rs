//! ASSET-CORE-04D — `GET /api/v1/portfolio/economics/status` proof tests.
//!
//! Composes the ASSET-CORE-04B registry-v2 -> instrument-economics bridge
//! with the ASSET-CORE-04A/04C economics model against the live in-memory
//! execution snapshot (positions/cash) and the latest completed `md_bars`
//! mark per non-flat symbol -- the same position/cash/mark sources
//! `/api/v1/portfolio/live-weights` (`PORTFOLIO-LIVE-WEIGHTS-01`) already
//! uses. Read-only, diagnostic-only: no DB write, no provider/broker call,
//! no order/risk/runtime path is touched.
//!
//! Cross-currency (`currency_conversion_unsupported`) and a registry row
//! that exists but fails to bridge (e.g. a non-equity contract with a bad
//! multiplier) are NOT proven at the route level here: the v1 registry JSON
//! format this route loads only ever converts to `Equity`/`Etf` v2 contracts
//! (`convert_v1_registry_to_v2`), so neither case is constructible through a
//! v1 fixture file without editing committed config -- mirroring the same
//! limitation `scenario_instrument_economics_registry_bridge_asset_core_04b.rs`
//! documents for futures/options/crypto/forex. Both code paths exist in the
//! route (see `resolve_position_economics` in `routes/portfolio.rs`) and are
//! covered transitively by ASSET-CORE-04A/04B/04C's own pure-function tests.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_portfolio_economics_status_asset_core_04d
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/portfolio/economics/status";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

async fn call_json(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("body is valid JSON");
    (status, json)
}

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn custom_registry_json(symbols: &[&str]) -> String {
    let entries: Vec<String> = symbols
        .iter()
        .map(|s| {
            format!(
                r#"{{"instrument_id":"equity:US:{s}","symbol":"{s}","asset_class":"equity","provider":"twelvedata","provider_symbol":"{s}","venue":"NASDAQ","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"ASSET-CORE-04D test fixture"}}"#
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Build a router whose `AppState` has no DB pool and a real-or-custom
/// registry path already configured, returning the shared state too so the
/// caller can inject an execution snapshot before issuing requests.
fn make_router_with_registry_path(registry_path: String) -> (Arc<state::AppState>, axum::Router) {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    let st = Arc::new(st);
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

/// Write `content` to a uniquely-named temp file and build a router pointed
/// at it. Returns the path too so the caller can remove it afterward.
fn make_router_with_registry_content(
    content: &str,
    tag: &str,
) -> (axum::Router, Arc<state::AppState>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_portfolio_economics_status_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp registry fixture");
    let (st, router) = make_router_with_registry_path(path.to_string_lossy().to_string());
    (router, st, path)
}

fn make_router_with_db_and_real_registry(
    pool: sqlx::PgPool,
) -> (Arc<state::AppState>, axum::Router) {
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    );
    st.instrument_registry_path = real_registry_path();
    let st = Arc::new(st);
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

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
    is_complete: bool,
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
    .bind(is_complete)
    .execute(pool)
    .await
    .expect("insert test md_bars row");
}

async fn delete_test_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await;
}

fn row_by_symbol<'a>(v: &'a serde_json::Value, symbol: &str) -> &'a serde_json::Value {
    v["positions"]
        .as_array()
        .expect("positions must be an array")
        .iter()
        .find(|r| r["symbol"] == symbol)
        .unwrap_or_else(|| panic!("row for symbol {symbol} must be present in: {v}"))
}

// ---------------------------------------------------------------------------
// ACD-01: no execution snapshot
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_01_no_snapshot_returns_no_snapshot_truth_state_no_panic() {
    let (_st, router) = make_router_with_registry_path(real_registry_path());
    let (status, body) = call_json(router, ROUTE).await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "no_snapshot");
    assert_eq!(body["has_execution_snapshot"], false);
    assert_eq!(body["position_count"].as_u64(), Some(0));
    assert!(body["nav_micros"].is_null());
    assert!(body["cash_micros"].is_null());
    assert_eq!(
        body["positions"].as_array().map(|a| a.len()),
        Some(0),
        "positions must be empty when there is no snapshot"
    );
}

// ---------------------------------------------------------------------------
// ACD-02 / ACD-03: empty vs. flat portfolio
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_02_empty_positions_returns_no_positions_nav_equals_cash() {
    let (st, router) = make_router_with_registry_path(real_registry_path());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "no_positions", "body={body}");
    assert_eq!(body["nav_micros"], 500_000_000);
    assert_eq!(body["gross_exposure_micros"], 0);
    assert_eq!(body["net_exposure_micros"], 0);
    assert_eq!(body["position_count"].as_u64(), Some(0));
    assert_eq!(body["positions"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn ac04d_03_single_flat_position_returns_active_with_zero_exposure() {
    let (st, router) = make_router_with_registry_path(real_registry_path());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[("AAPL", 0)]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"], "active",
        "a flat position is valued as zero without needing a mark: body={body}"
    );
    assert_eq!(body["nav_micros"], 500_000_000);
    assert_eq!(body["gross_exposure_micros"], 0);
    assert_eq!(body["net_exposure_micros"], 0);
    assert_eq!(body["position_count"].as_u64(), Some(1));

    let row = row_by_symbol(&body, "AAPL");
    assert_eq!(row["truth_state"], "active");
    assert_eq!(row["notional_micros"], 0);
    assert_eq!(row["weight_bps"], 0);
    assert_eq!(row["mark_price_micros"], serde_json::Value::Null);
}

// ---------------------------------------------------------------------------
// ACD-04 / ACD-05: DB-backed active valuation (real registry symbols)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_04_db_seeded_mark_produces_active_nav_gross_net_weight() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ACD-04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("ACD-04: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let timeframe = "1D_AC04D_ACTIVE";
    delete_test_bars(&pool, "AAPL", timeframe).await;
    insert_test_bar(&pool, "AAPL", timeframe, 1_790_000_001, 25_000_000, true).await;

    let (st, router) = make_router_with_db_and_real_registry(pool.clone());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[("AAPL", 4)]));

    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe={timeframe}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active", "body={body}");
    assert_eq!(body["nav_micros"], 100_000_000);
    assert_eq!(body["gross_exposure_micros"], 100_000_000);
    assert_eq!(body["net_exposure_micros"], 100_000_000);
    assert_eq!(body["valued_position_count"].as_u64(), Some(1));
    assert_eq!(body["failed_position_count"].as_u64(), Some(0));

    let row = row_by_symbol(&body, "AAPL");
    assert_eq!(row["asset_class"], "equity");
    assert_eq!(row["quote_currency"], "USD");
    assert_eq!(row["signed_qty"], 4);
    assert_eq!(row["mark_price_micros"], 25_000_000);
    assert_eq!(row["mark_source"], format!("md_bars:{timeframe}:close"));
    assert_eq!(row["notional_micros"], 100_000_000);
    assert_eq!(row["weight_bps"], 10000);
    assert_eq!(row["model_only"], false);

    delete_test_bars(&pool, "AAPL", timeframe).await;
}

#[tokio::test]
async fn ac04d_05_long_and_short_positions_preserve_net_gross_signs() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ACD-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("ACD-05: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let timeframe = "1D_AC04D_LONGSHORT";
    delete_test_bars(&pool, "AAPL", timeframe).await;
    delete_test_bars(&pool, "MSFT", timeframe).await;
    insert_test_bar(&pool, "AAPL", timeframe, 1_790_000_002, 20_000_000, true).await; // $20
    insert_test_bar(&pool, "MSFT", timeframe, 1_790_000_002, 100_000_000, true).await; // $100

    let (st, router) = make_router_with_db_and_real_registry(pool.clone());
    st.execution_snapshot.write().await.replace(make_snapshot(
        1_000_000_000, // $1000 cash
        &[("AAPL", 10), ("MSFT", -5)],
    ));

    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe={timeframe}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active", "body={body}");
    assert_eq!(body["nav_micros"], 700_000_000); // 1_000_000_000 + 200_000_000 - 500_000_000
    assert_eq!(body["gross_exposure_micros"], 700_000_000); // 200_000_000 + 500_000_000
    assert_eq!(body["net_exposure_micros"], -300_000_000); // 200_000_000 - 500_000_000

    let aapl = row_by_symbol(&body, "AAPL");
    assert_eq!(
        aapl["notional_micros"], 200_000_000,
        "long notional must be positive"
    );
    let msft = row_by_symbol(&body, "MSFT");
    assert_eq!(
        msft["notional_micros"], -500_000_000,
        "short notional must be negative"
    );

    delete_test_bars(&pool, "AAPL", timeframe).await;
    delete_test_bars(&pool, "MSFT", timeframe).await;
}

// ---------------------------------------------------------------------------
// ACD-06 / ACD-07: missing / incomplete marks (DB present)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_06_missing_completed_mark_with_db_present_is_missing_marks() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ACD-06: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("ACD-06: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let timeframe = "1D_AC04D_NOBAR";
    delete_test_bars(&pool, "AAPL", timeframe).await; // ensure no bar exists

    let (st, router) = make_router_with_db_and_real_registry(pool.clone());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("AAPL", 3)]));

    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe={timeframe}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "missing_marks", "body={body}");
    assert!(body["nav_micros"].is_null());
    assert_eq!(body["missing_mark_count"].as_u64(), Some(1));

    let row = row_by_symbol(&body, "AAPL");
    assert_eq!(row["reason_code"], "missing_mark_for_non_flat_position");
    assert!(
        row["mark_price_micros"].is_null(),
        "no fabricated mark: {row}"
    );
    assert!(
        row["notional_micros"].is_null(),
        "no fabricated notional: {row}"
    );
}

#[tokio::test]
async fn ac04d_07_incomplete_bar_is_ignored_same_as_missing_marks() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ACD-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("ACD-07: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let timeframe = "1D_AC04D_INCOMPLETE";
    delete_test_bars(&pool, "AAPL", timeframe).await;
    insert_test_bar(&pool, "AAPL", timeframe, 1_790_000_003, 30_000_000, false).await; // is_complete=false

    let (st, router) = make_router_with_db_and_real_registry(pool.clone());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("AAPL", 2)]));

    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe={timeframe}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"], "missing_marks",
        "an incomplete-only bar must not be used as a mark: body={body}"
    );
    let row = row_by_symbol(&body, "AAPL");
    assert!(row["mark_price_micros"].is_null());

    delete_test_bars(&pool, "AAPL", timeframe).await;
}

// ---------------------------------------------------------------------------
// ACD-08: missing mark, no DB pool at all -> db_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_08_missing_mark_without_db_pool_is_db_unavailable() {
    let (st, router) = make_router_with_registry_path(real_registry_path());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("AAPL", 7)]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"], "db_unavailable",
        "no DB pool at all is distinct from DB-present-but-no-bar: body={body}"
    );
    assert_eq!(body["reason_code"], "no_db_pool_configured");
    assert!(body["nav_micros"].is_null());
}

// ---------------------------------------------------------------------------
// ACD-09: symbol absent from registry fails closed, no fabricated default
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_09_symbol_absent_from_registry_fails_closed_no_fabricated_default() {
    let (router, st, path) =
        make_router_with_registry_content(&custom_registry_json(&["ZZ04DKNOWN"]), "symbol_absent");
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("ZZ04DUNKNOWN", 5)]));

    let (status, body) = call_json(router, ROUTE).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"], "position_value_unavailable",
        "body={body}"
    );
    assert_eq!(body["valued_position_count"].as_u64(), Some(0));
    assert_eq!(body["failed_position_count"].as_u64(), Some(1));
    assert_eq!(body["non_equity_position_count"].as_u64(), Some(0));
    assert!(body["nav_micros"].is_null());

    let row = row_by_symbol(&body, "ZZ04DUNKNOWN");
    assert_eq!(row["reason_code"], "symbol_not_found_in_registry");
    assert_eq!(
        row["asset_class"], "",
        "unresolved symbol must not be fabricated as equity: {row}"
    );
    assert_eq!(
        row["quote_currency"], "",
        "unresolved symbol's currency must be reported as unknown, not USD: {row}"
    );
    assert!(
        row["notional_micros"].is_null(),
        "no fabricated notional: {row}"
    );
    assert!(
        row["absolute_notional_micros"].is_null(),
        "no fabricated notional: {row}"
    );
    assert!(row["weight_bps"].is_null(), "no fabricated weight: {row}");
}

// ---------------------------------------------------------------------------
// ACD-10 / ACD-11: registry missing / malformed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_10_missing_registry_file_returns_fail_closed_envelope_no_panic() {
    let missing_path = std::env::temp_dir()
        .join(format!(
            "mqk_missing_portfolio_economics_status_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    let (st, router) = make_router_with_registry_path(missing_path);
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "registry_unavailable");
    assert_eq!(body["registry_v2_valid"], false);
    assert_eq!(body["has_execution_snapshot"], true);
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
}

#[tokio::test]
async fn ac04d_11_malformed_registry_returns_fail_closed_envelope_no_panic() {
    let (router, st, path) = make_router_with_registry_content("{not valid json", "malformed");
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, ROUTE).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "registry_invalid");
    assert_eq!(body["registry_v2_valid"], false);
}

// ---------------------------------------------------------------------------
// ACD-12 / ACD-13: symbol / limit query params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_12_symbol_filter_returns_matching_row_counts_unaffected() {
    let (router, st, path) = make_router_with_registry_content(
        &custom_registry_json(&["ZZ04DAAA", "ZZ04DBBB"]),
        "symbol_filter",
    );
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[("ZZ04DAAA", 0), ("ZZ04DBBB", 0)]));

    let (status, body) = call_json(router, &format!("{ROUTE}?symbol=zz04daaa")).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK);
    let rows = body["positions"].as_array().expect("positions array");
    assert_eq!(rows.len(), 1, "rows={rows:?}");
    assert_eq!(rows[0]["symbol"], "ZZ04DAAA");
    assert_eq!(
        body["position_count"].as_u64(),
        Some(2),
        "position_count must stay full-portfolio, unaffected by symbol filter"
    );
    assert_eq!(body["symbol_filter"], "ZZ04DAAA");
}

#[tokio::test]
async fn ac04d_13_limit_param_truncates_rows_counts_unaffected() {
    let (router, st, path) = make_router_with_registry_content(
        &custom_registry_json(&["ZZ04DAAA", "ZZ04DBBB", "ZZ04DCCC"]),
        "limit",
    );
    st.execution_snapshot.write().await.replace(make_snapshot(
        0,
        &[("ZZ04DAAA", 0), ("ZZ04DBBB", 0), ("ZZ04DCCC", 0)],
    ));

    let (status, body) = call_json(router, &format!("{ROUTE}?limit=2")).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK);
    let rows = body["positions"].as_array().expect("positions array");
    assert_eq!(rows.len(), 2);
    assert_eq!(body["position_count"].as_u64(), Some(3));
    assert_eq!(body["valued_position_count"].as_u64(), Some(3));
}

// ---------------------------------------------------------------------------
// ACD-14: honesty flags always as documented
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_14_honesty_flags_always_as_documented() {
    fn assert_honesty_flags(body: &serde_json::Value) {
        assert_eq!(body["model_only"], true, "body={body}");
        assert_eq!(
            body["trading_uses_portfolio_economics"], false,
            "body={body}"
        );
        assert_eq!(
            body["runtime_uses_portfolio_economics"], false,
            "body={body}"
        );
        assert_eq!(body["risk_uses_portfolio_economics"], false, "body={body}");
        assert_eq!(
            body["order_path_uses_portfolio_economics"], false,
            "body={body}"
        );
    }

    let (_st, router) = make_router_with_registry_path(real_registry_path());
    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "no_snapshot");
    assert_honesty_flags(&body);

    let (st, router) = make_router_with_registry_path(real_registry_path());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[("AAPL", 0)]));
    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_honesty_flags(&body);
}

// ---------------------------------------------------------------------------
// ACD-15: real registry, held positions never enable non-equity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_15_real_registry_equity_positions_have_zero_non_equity_counts() {
    let (st, router) = make_router_with_registry_path(real_registry_path());
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(100_000_000, &[("AAPL", 0), ("MSFT", 0)]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active", "body={body}");
    assert_eq!(body["non_equity_position_count"].as_u64(), Some(0));
    assert_eq!(body["non_equity_enabled_count"].as_u64(), Some(0));

    for symbol in ["AAPL", "MSFT"] {
        let row = row_by_symbol(&body, symbol);
        assert_eq!(row["truth_state"], "active", "symbol={symbol} row={row}");
        assert_eq!(row["asset_class"], "equity", "symbol={symbol} row={row}");
        assert_eq!(row["model_only"], false, "symbol={symbol} row={row}");
    }
}

// ---------------------------------------------------------------------------
// ACD-16: timeframe echo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_16_timeframe_param_is_echoed_and_defaults_to_1d() {
    let (_st, router) = make_router_with_registry_path(real_registry_path());
    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe=1m")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["timeframe"], "1m");

    let (_st, router) = make_router_with_registry_path(real_registry_path());
    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["timeframe"], "1D");
}

// ---------------------------------------------------------------------------
// ACD-17: read-only safety
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04d_17_route_never_mutates_execution_snapshot() {
    let (st, router) = make_router_with_registry_path(real_registry_path());
    let snapshot = make_snapshot(100_000_000, &[("AAPL", 10)]);
    st.execution_snapshot
        .write()
        .await
        .replace(snapshot.clone());

    let (status, _body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);

    let after = st.execution_snapshot.read().await.clone();
    assert_eq!(
        after.map(|s| s.portfolio.cash_micros),
        Some(snapshot.portfolio.cash_micros),
        "economics-status route must not mutate execution_snapshot"
    );
}
