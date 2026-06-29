//! ASSET-CORE-04F — `GET /api/v1/portfolio/economics/status` registry-v2
//! input seam proof tests.
//!
//! Continues the `ASSET-CORE-04` / crypto-local-marks lane after
//! `ASSET-CORE-04D-PORTFOLIO-ECONOMICS-READONLY-STATUS-01-COMBINED`,
//! `ASSET-CORE-04E-NON-EQUITY-MARK-DATA-SOURCE-DECISION-01-COMBINED`,
//! `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`, and
//! `CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01`. Those four patches
//! repeatedly documented the same structural blocker:
//! `portfolio_economics_status` only ever sourced its registry from
//! `st.instrument_registry_path` (v1) -> `convert_v1_registry_to_v2`, and
//! `convert_v1_registry_to_v2` unconditionally stamps an `Equity`/`Etf`
//! contract onto every row regardless of its `asset_class` string -- so no
//! v1-sourced row could ever carry a `CryptoPair` contract, and no real
//! crypto position could ever reach this HTTP route.
//!
//! This file proves the new `?registry_source=v2` seam
//! (`routes/portfolio.rs::resolve_portfolio_economics_registry`,
//! `AppState::portfolio_economics_registry_v2_path`, sourced only from
//! `MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH`) closes that blocker: with no
//! query param and no env var set, the route is byte-for-byte its
//! pre-existing v1-only behavior; with `?registry_source=v2` and a configured
//! path, the route loads a real registry-v2 document server-side (never a
//! client-supplied path) and can value the committed disabled `BTC/USD`
//! fixture (`config/instruments/instruments_v2.crypto_local_marks.example.json`)
//! through the actual HTTP route, reading its mark from the existing
//! `md_bars` completed-bar path. This is a route-input seam and route-level
//! proof only -- not a production registry-v2 cutover, not crypto trading
//! enablement, not a portfolio-ledger cutover.
//!
//! # Whole-unit quantity constraint (documented deviation from the mission
//! brief's illustrative "0.01 BTC" example)
//!
//! `mqk_runtime::observability::PositionSnapshot::net_qty` is a plain `i64`
//! ("positive = long, negative = short, 0 = flat" -- no fractional/micros
//! representation). `resolve_position_economics` in `routes/portfolio.rs`
//! converts this whole-unit quantity into `qty_micros` by multiplying by
//! `mqk_portfolio::MICROS_SCALE`, exactly as it already does for every
//! equity position. This route therefore cannot represent a fractional
//! `0.01 BTC` position -- doing so would require changing
//! `mqk-runtime::observability::PositionSnapshot` itself, which is out of
//! this patch's scope (`core-rs/crates/mqk-runtime/src/*` is forbidden). The
//! fractional-quantity case (`0.01 BTC` @ `$44,100.00` = `$441.00`) remains
//! proven at the pure-model layer by `CRYPTO-DATA-01A`/`01B`, which call
//! `mqk_portfolio::value_position_economics` directly with a hand-built
//! `signed_qty_micros`, bypassing `PositionSnapshot` entirely. The tests
//! below instead use a whole `1 BTC` position (`$44,100.00` notional at the
//! same documented fixture mark) -- the highest-fidelity whole-unit proof
//! this specific HTTP route can construct.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_portfolio_economics_v2_registry_seam_asset_core_04f
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_portfolio::MICROS_SCALE;
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/portfolio/economics/status";
const BTC_SYMBOL: &str = "BTC/USD";
/// Private timeframe tag for this file's DB-backed tests -- distinct from
/// `CRYPTO-DATA-01B`'s bare `"1D"` and `ASSET-CORE-04D`'s `"1D_AC04D_*"` tags,
/// so this file's `(BTC/USD, timeframe)` rows can never collide with another
/// integration-test binary running concurrently against the same shared
/// local paper DB.
const BTC_TIMEFRAME: &str = "1D_AC04F_BTC_ACTIVE";

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

/// The real, committed, disabled `BTC/USD` registry-v2 fixture
/// `CRYPTO-DATA-01A` added -- unmodified by this patch.
fn crypto_v2_fixture_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
        .canonicalize()
        .expect("crypto v2 fixture path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn write_temp_v2_file(content: &str, tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_ac04f_registry_v2_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp v2 registry fixture");
    path
}

/// Build a router whose `AppState` has no DB pool, the real v1 registry path,
/// and an optional configured ASSET-CORE-04F v2 path, returning the shared
/// state too so the caller can inject an execution snapshot before issuing
/// requests. Mirrors
/// `scenario_portfolio_economics_status_asset_core_04d.rs`'s
/// `make_router_with_registry_path`, extended with the new v2-path seam.
fn make_router(v1_path: String, v2_path: Option<String>) -> (Arc<state::AppState>, axum::Router) {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = v1_path;
    st.portfolio_economics_registry_v2_path = v2_path;
    let st = Arc::new(st);
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

fn make_router_with_db(
    pool: sqlx::PgPool,
    v1_path: String,
    v2_path: Option<String>,
) -> (Arc<state::AppState>, axum::Router) {
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    );
    st.instrument_registry_path = v1_path;
    st.portfolio_economics_registry_v2_path = v2_path;
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
// ac04f_01 / ac04f_02: default + explicit legacy match pre-existing 04D
// behavior, even when a v2 path is configured server-side.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_01_default_omitted_registry_source_uses_legacy() {
    let (st, router) = make_router(real_registry_path(), None);
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[]));

    let (status, body) = call_json(router, ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "no_positions", "body={body}");
    assert_eq!(body["registry_source_requested"], "legacy");
    assert_eq!(body["registry_source_used"], "legacy");
    assert_eq!(body["registry_v2_configured"], false);
    assert!(body["registry_v2_path"].is_null());
    assert!(
        body["registry_error_code"].is_null(),
        "no registry error on a successful legacy response: {body}"
    );
}

#[tokio::test]
async fn ac04f_02_explicit_legacy_uses_legacy_even_with_v2_configured() {
    let (st, router) = make_router(real_registry_path(), Some(crypto_v2_fixture_path()));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(500_000_000, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=legacy")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "no_positions", "body={body}");
    assert_eq!(body["registry_source_requested"], "legacy");
    assert_eq!(body["registry_source_used"], "legacy");
    assert_eq!(
        body["registry_v2_configured"], true,
        "v2 is configured server-side..."
    );
    assert_eq!(
        body["registry_v2_valid"], true,
        "...but explicit legacy must still use the v1 lane, body={body}"
    );
}

// ---------------------------------------------------------------------------
// ac04f_03 / ac04f_04 / ac04f_05: v2 requested but unavailable/malformed/
// invalid fails closed -- never a silent fallback to legacy.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_03_v2_requested_not_configured_fails_closed_no_fallback() {
    let (st, router) = make_router(real_registry_path(), None);
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=v2")).await;
    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "registry_unavailable", "body={body}");
    assert_eq!(body["reason_code"], "registry_v2_not_configured");
    assert_eq!(body["registry_error_code"], "registry_v2_not_configured");
    assert_eq!(body["registry_source_requested"], "v2");
    assert_eq!(
        body["registry_source_used"], "none",
        "must not silently fall back to legacy: {body}"
    );
    assert_eq!(body["registry_v2_configured"], false);
}

#[tokio::test]
async fn ac04f_04_v2_requested_configured_path_missing_fails_closed() {
    let missing_path = std::env::temp_dir()
        .join(format!(
            "mqk_missing_ac04f_registry_v2_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    let (st, router) = make_router(real_registry_path(), Some(missing_path));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=v2")).await;
    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "registry_unavailable", "body={body}");
    assert_eq!(body["reason_code"], "registry_v2_path_not_found");
    assert_eq!(body["registry_v2_configured"], true);
    assert_eq!(body["registry_source_used"], "none");
}

#[tokio::test]
async fn ac04f_05_v2_requested_malformed_file_fails_closed_no_panic() {
    let path = write_temp_v2_file("{not valid json", "malformed");
    let (st, router) = make_router(
        real_registry_path(),
        Some(path.to_string_lossy().to_string()),
    );
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=v2")).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(
        status,
        StatusCode::OK,
        "must still return 200, no panic: {body}"
    );
    assert_eq!(body["truth_state"], "registry_invalid", "body={body}");
    assert!(
        body["reason_code"]
            .as_str()
            .is_some_and(|s| s.starts_with("registry_v2_load_failed:")),
        "body={body}"
    );
    assert_eq!(body["registry_source_used"], "none");
}

#[tokio::test]
async fn ac04f_06_v2_requested_invalid_registry_fails_closed() {
    let path = write_temp_v2_file(r#"{"schema_version":999,"instruments":[]}"#, "bad_schema");
    let (st, router) = make_router(
        real_registry_path(),
        Some(path.to_string_lossy().to_string()),
    );
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=v2")).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(
        status,
        StatusCode::OK,
        "must still return 200, no panic: {body}"
    );
    assert_eq!(body["truth_state"], "registry_invalid", "body={body}");
    assert!(
        body["reason_code"]
            .as_str()
            .is_some_and(|s| s.starts_with("registry_v2_validation_failed:")),
        "body={body}"
    );
}

// ---------------------------------------------------------------------------
// ac04f_06b: v2 requested with the real committed BTC/USD fixture validates
// and is used -- proven without needing a DB (empty positions, no mark
// lookup required).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_06b_v2_requested_with_committed_fixture_validates_and_is_used() {
    let (st, router) = make_router(real_registry_path(), Some(crypto_v2_fixture_path()));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=v2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "no_positions", "body={body}");
    assert_eq!(body["registry_v2_valid"], true);
    assert_eq!(body["registry_source_requested"], "v2");
    assert_eq!(body["registry_source_used"], "v2");
    assert_eq!(body["registry_v2_configured"], true);
    assert!(body["registry_error_code"].is_null());
    assert_eq!(
        body["non_equity_enabled_count"], 0,
        "the fixture's BTC/USD row must remain disabled: {body}"
    );
}

// ---------------------------------------------------------------------------
// ac04f_14: unknown registry_source value fails closed, no panic, no silent
// legacy fallback.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_14_unknown_registry_source_fails_closed_no_panic() {
    let (st, router) = make_router(real_registry_path(), None);
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[]));

    let (status, body) = call_json(router, &format!("{ROUTE}?registry_source=bogus")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "must still return 200, no panic: {body}"
    );
    assert_eq!(
        body["truth_state"], "invalid_registry_source",
        "body={body}"
    );
    assert_eq!(body["reason_code"], "unknown_registry_source_value");
    assert_eq!(
        body["registry_source_requested"], "bogus",
        "the raw invalid value must be echoed verbatim, not silently coerced: {body}"
    );
    assert_eq!(body["registry_source_used"], "none");
}

// ---------------------------------------------------------------------------
// ac04f_07 - ac04f_13: DB-backed route-level BTC/USD valuation proof. Several
// proof points key off the same (BTC/USD, BTC_TIMEFRAME) row family and one
// inserts an extra probe row into that family, so -- mirroring
// CRYPTO-DATA-01B's `db_persistence_readback_and_model_chain_proof` and
// ASSET-CORE-04D's own per-test-private-timeframe convention -- they are
// composed sequentially in one test rather than several independent ones
// that could race within this binary's concurrently-run `#[tokio::test]`s.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_db_backed_route_level_btc_valuation_proof() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ASSET-CORE-04F: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, BTC_SYMBOL, BTC_TIMEFRAME).await;

    let outbox_count_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox before");

    // ---- ac04f_07: seed one completed BTC/USD bar and value it via the
    // real route, through the committed registry-v2 fixture. ----
    let btc_end_ts = 1_790_000_010_i64;
    insert_test_bar(
        &pool,
        BTC_SYMBOL,
        BTC_TIMEFRAME,
        btc_end_ts,
        44_100 * MICROS_SCALE,
        true,
    )
    .await;

    let (st, _router) = make_router_with_db(
        pool.clone(),
        real_registry_path(),
        Some(crypto_v2_fixture_path()),
    );
    // See module docs: 1 whole BTC, not the mission brief's illustrative
    // "0.01 BTC" -- PositionSnapshot::net_qty is a whole-unit i64.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(10_000 * MICROS_SCALE, &[(BTC_SYMBOL, 1)]));

    let router1 = routes::build_router(Arc::clone(&st));
    let (status, body) = call_json(
        router1,
        &format!("{ROUTE}?timeframe={BTC_TIMEFRAME}&symbol=BTC%2FUSD&registry_source=v2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["truth_state"], "active", "ac04f_07: body={body}");
    assert_eq!(body["registry_source_requested"], "v2");
    assert_eq!(body["registry_source_used"], "v2");
    assert_eq!(body["registry_v2_valid"], true);
    assert!(body["registry_error_code"].is_null());

    let row = row_by_symbol(&body, BTC_SYMBOL);
    // ac04f_09: crypto asset class + USD quote/currency on the route's own
    // response row.
    assert_eq!(row["asset_class"], "crypto", "ac04f_09: row={row}");
    assert_eq!(row["quote_currency"], "USD", "ac04f_09: row={row}");
    assert_eq!(row["signed_qty"], 1);
    assert_eq!(
        row["mark_price_micros"],
        44_100 * MICROS_SCALE,
        "ac04f_07: mark must come from the seeded md_bars row"
    );
    assert_eq!(row["mark_source"], format!("md_bars:{BTC_TIMEFRAME}:close"));
    assert_eq!(
        row["notional_micros"],
        44_100 * MICROS_SCALE,
        "ac04f_07: 1 BTC * $44,100.00 * 1.0x multiplier"
    );
    // ac04f_12: crypto row is model_only; the fixture itself stays disabled
    // (proven again below via non_equity_enabled_count == 0), and this
    // response never implies trading permission for any instrument.
    assert_eq!(row["model_only"], true, "ac04f_12: row={row}");
    assert_eq!(row["truth_state"], "active");

    // ac04f_10 / ac04f_11: portfolio aggregation includes crypto + USD
    // exposure, NAV includes cash + crypto notional.
    assert_eq!(body["cash_micros"], 10_000 * MICROS_SCALE);
    assert_eq!(
        body["nav_micros"],
        (10_000 + 44_100) * MICROS_SCALE,
        "ac04f_10/11: nav = cash + crypto notional: body={body}"
    );
    assert_eq!(body["gross_exposure_micros"], 44_100 * MICROS_SCALE);
    assert_eq!(body["non_equity_position_count"], 1);
    assert_eq!(
        body["non_equity_enabled_count"], 0,
        "BTC/USD remains disabled in the registry: body={body}"
    );

    let crypto_exposure = body["asset_class_exposures"]
        .as_array()
        .expect("asset_class_exposures array")
        .iter()
        .find(|r| r["key"] == "crypto")
        .unwrap_or_else(|| panic!("ac04f_10: crypto asset-class exposure row must exist: {body}"));
    assert_eq!(
        crypto_exposure["absolute_notional_micros"],
        44_100 * MICROS_SCALE
    );

    let usd_exposure = body["currency_exposures"]
        .as_array()
        .expect("currency_exposures array")
        .iter()
        .find(|r| r["key"] == "USD")
        .unwrap_or_else(|| panic!("ac04f_11: USD currency exposure row must exist: {body}"));
    assert_eq!(
        usd_exposure["absolute_notional_micros"],
        44_100 * MICROS_SCALE
    );

    // ac04f_12 (response-level): honesty flags remain exactly as documented
    // for a crypto-valued response too.
    assert_eq!(body["model_only"], true);
    assert_eq!(body["trading_uses_portfolio_economics"], false);
    assert_eq!(body["runtime_uses_portfolio_economics"], false);
    assert_eq!(body["risk_uses_portfolio_economics"], false);
    assert_eq!(body["order_path_uses_portfolio_economics"], false);

    // ---- ac04f_08: an incomplete newer row must not be read as the mark.
    let newer_incomplete_end_ts = btc_end_ts + 86_400;
    insert_test_bar(
        &pool,
        BTC_SYMBOL,
        BTC_TIMEFRAME,
        newer_incomplete_end_ts,
        50_000 * MICROS_SCALE,
        false,
    )
    .await;

    let router2 = routes::build_router(Arc::clone(&st));
    let (status2, body2) = call_json(
        router2,
        &format!("{ROUTE}?timeframe={BTC_TIMEFRAME}&symbol=BTC%2FUSD&registry_source=v2"),
    )
    .await;
    assert_eq!(status2, StatusCode::OK, "body={body2}");
    assert_eq!(
        body2["truth_state"], "active",
        "ac04f_08: an incomplete newer bar must not change the valued mark: body={body2}"
    );
    let row2 = row_by_symbol(&body2, BTC_SYMBOL);
    assert_eq!(
        row2["mark_price_micros"],
        44_100 * MICROS_SCALE,
        "ac04f_08: incomplete newer row must be ignored, mark unchanged: row={row2}"
    );

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2 and end_ts = $3")
        .bind(BTC_SYMBOL)
        .bind(BTC_TIMEFRAME)
        .bind(newer_incomplete_end_ts)
        .execute(&pool)
        .await
        .expect("cleanup: delete incomplete probe row");

    // ---- ac04f_13: this entire test must never write to oms_outbox. ----
    let outbox_count_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox after");
    assert_eq!(
        outbox_count_before, outbox_count_after,
        "ac04f_13: this test must never write to oms_outbox"
    );

    delete_test_bars(&pool, BTC_SYMBOL, BTC_TIMEFRAME).await;
}

// ---------------------------------------------------------------------------
// ac04f_15: existing ASSET-CORE-04D equity/legacy route behavior is
// unaffected by this patch's changes (full regression coverage is the
// dedicated `scenario_portfolio_economics_status_asset_core_04d.rs` suite;
// this is an in-file sanity check that the legacy lane through the
// refactored handler still produces the exact same shape of result).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04f_15_legacy_equity_route_behavior_unchanged() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ASSET-CORE-04F ac04f_15: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let timeframe = "1D_AC04F_EQUITY_REGRESSION";
    delete_test_bars(&pool, "AAPL", timeframe).await;
    insert_test_bar(&pool, "AAPL", timeframe, 1_790_000_020, 25_000_000, true).await;

    let (st, router) = make_router_with_db(pool.clone(), real_registry_path(), None);
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(0, &[("AAPL", 4)]));

    let (status, body) = call_json(router, &format!("{ROUTE}?timeframe={timeframe}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active", "body={body}");
    assert_eq!(body["nav_micros"], 100_000_000);
    assert_eq!(body["registry_source_requested"], "legacy");
    assert_eq!(body["registry_source_used"], "legacy");
    assert_eq!(body["registry_v2_configured"], false);

    let row = row_by_symbol(&body, "AAPL");
    assert_eq!(row["asset_class"], "equity");
    assert_eq!(row["quote_currency"], "USD");
    assert_eq!(row["model_only"], false);

    delete_test_bars(&pool, "AAPL", timeframe).await;
}
