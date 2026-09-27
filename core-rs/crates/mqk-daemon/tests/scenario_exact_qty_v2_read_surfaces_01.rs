//! Exact-quantity V2 read surfaces (`qty_micros_v1`) for live-weights,
//! paper-journal closed trades and strategy-performance.
//!
//! Invariant: a real fractional position/closure is never truncated, rounded,
//! nulled or dropped. V1 (whole-unit) refuses it with a 409 that names the V2
//! route; V2 carries the exact raw `QtyMicros`; whole-unit V1 output is
//! unchanged.
//!
//! | Test  | Claim                                                                    |
//! |-------|--------------------------------------------------------------------------|
//! | EQ01  | live-weights: whole book V1 unchanged, V2 exact + version tag           |
//! | EQ02  | live-weights: fractional long+short V1 409, V2 exact and signed         |
//! | EQ03  | V2 envelopes carry the version tag even when the source is unavailable  |
//! | EQ04  | journal closed trades: fractional long+short V1 409, V2 exact           |
//! | EQ05  | journal closed trades: whole-unit V1 unchanged, V2 exact                |
//! | EQ06  | strategy-performance: fractional V1 409, V2 exact total                 |
//! | EQ07  | strategy-performance: whole-unit V1 unchanged, V2 exact                 |
//! | EQ08  | journal admissions: a non-integer admission qty is skipped, never `0`   |
//! | EQ09  | execution orders: V2 exact; V1 keeps its documented fractional null     |
//! | EQ10  | execution orders V2: no snapshot is 503, not an empty list              |
//!
//! EQ01-EQ03 are in-process. EQ04-EQ08 run against a real disposable Postgres
//! database (`mqk_db::run_isolated`).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_execution::{BrokerEvent, QtyMicros, Side};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
use tower::ServiceExt;
use uuid::Uuid;

const V2_VERSION: &str = "qty_micros_v1";

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&body).expect("JSON body"))
}

fn no_db_router() -> (Arc<state::AppState>, axum::Router) {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

fn snapshot(positions: &[(&str, &str)]) -> ExecutionSnapshot {
    ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros: 500_000_000,
            realized_pnl_micros: 0,
            positions: positions
                .iter()
                .map(|(sym, qty)| PositionSnapshot {
                    symbol: sym.to_string(),
                    net_qty: qty.parse::<QtyMicros>().expect("qty"),
                })
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: Utc::now(),
    }
}

fn position<'a>(v: &'a serde_json::Value, symbol: &str) -> &'a serde_json::Value {
    v["positions"]
        .as_array()
        .expect("positions array")
        .iter()
        .find(|p| p["symbol"] == symbol)
        .unwrap_or_else(|| panic!("no position {symbol} in {v}"))
}

// ---------------------------------------------------------------------------
// live-weights
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq01_live_weights_whole_book_v1_unchanged_v2_exact() {
    let (st, router) = no_db_router();
    st.execution_snapshot
        .write()
        .await
        .replace(snapshot(&[("AAPL", "10"), ("TSLA", "-3")]));

    let (status, v1) = call(router.clone(), "/api/v1/portfolio/live-weights").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(position(&v1, "AAPL")["signed_qty"], 10);
    assert_eq!(position(&v1, "TSLA")["signed_qty"], -3);
    assert!(
        v1.get("quantity_schema_version").is_none(),
        "V1 wire shape must not gain a key: {v1}"
    );

    let (status, v2) = call(router, "/api/v2/portfolio/live-weights").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v2["quantity_schema_version"], V2_VERSION);
    assert_eq!(position(&v2, "AAPL")["signed_qty_micros"], 10_000_000);
    assert_eq!(position(&v2, "TSLA")["signed_qty_micros"], -3_000_000);
    assert!(position(&v2, "AAPL").get("signed_qty").is_none());
    assert_eq!(v1["truth_state"], v2["truth_state"]);
}

#[tokio::test]
async fn eq02_live_weights_fractional_long_and_short_v1_409_v2_exact() {
    let (st, router) = no_db_router();
    st.execution_snapshot
        .write()
        .await
        .replace(snapshot(&[("BTC/USD", "0.5"), ("ETH/USD", "-1.500001")]));

    let (status, v1) = call(router.clone(), "/api/v1/portfolio/live-weights").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v1["error"], "quantity_not_representable_in_v1");
    assert!(v1["detail"]
        .as_str()
        .unwrap()
        .contains("/api/v2/portfolio/live-weights"));
    assert!(v1.get("positions").is_none(), "no partial V1 body: {v1}");

    let (status, v2) = call(router, "/api/v2/portfolio/live-weights").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(position(&v2, "BTC/USD")["signed_qty_micros"], 500_000);
    assert_eq!(position(&v2, "ETH/USD")["signed_qty_micros"], -1_500_001);
}

#[tokio::test]
async fn eq03_v2_envelopes_carry_the_version_tag_when_the_source_is_unavailable() {
    let (_st, router) = no_db_router();

    let (_, lw) = call(router.clone(), "/api/v2/portfolio/live-weights").await;
    assert_eq!(lw["truth_state"], "no_snapshot");
    assert_eq!(lw["quantity_schema_version"], V2_VERSION);

    let (status, j) = call(router.clone(), "/api/v2/paper/journal").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(j["quantity_schema_version"], V2_VERSION);
    assert_eq!(j["canonical_route"], "/api/v2/paper/journal");
    assert_eq!(j["closed_trades_lane"]["truth_state"], "no_db");
    let (_, j1) = call(router.clone(), "/api/v1/paper/journal").await;
    assert!(j1.get("quantity_schema_version").is_none());

    let (status, p) = call(router.clone(), "/api/v2/strategy/performance").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["quantity_schema_version"], V2_VERSION);
    assert_eq!(p["canonical_route"], "/api/v2/strategy/performance");
    assert_eq!(p["truth_state"], "db_unavailable");
    let (_, p1) = call(router.clone(), "/api/v1/strategy/performance").await;
    assert!(p1.get("quantity_schema_version").is_none());

    let (_, d) = call(router, "/api/v2/portfolio/durable-positions").await;
    assert_eq!(d["truth_state"], "db_unavailable");
    assert_eq!(d["quantity_schema_version"], V2_VERSION);
}

// ---------------------------------------------------------------------------
// DB fixtures: closed trades
// ---------------------------------------------------------------------------

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap()
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_exact_qty_v2_read_surfaces_01"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");
    st.inject_running_loop_for_test(mqk_daemon::state::ExecutionDomain::EquityNyse, run_id).await;
    run_id
}

/// One single-shot strategy-attributed order for the exact decimal `qty`,
/// filled in full at `price_micros`. `qty` is the wire decimal (`"0.5"`, `"10"`).
#[allow(clippy::too_many_arguments)]
async fn place_and_fill(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    side: Side,
    qty: &str,
    price_micros: i64,
    strategy_id: &str,
) {
    let side_str = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let order_json = serde_json::json!({
        "symbol": symbol,
        "qty": qty,
        "side": side_str,
        "strategy_id": strategy_id,
        "strategy_semantic_fingerprint": "a".repeat(64),
        "signal_source": "internal_strategy_decision",
    });
    mqk_db::outbox_enqueue(pool, run_id, order_id, order_json)
        .await
        .expect("outbox_enqueue");
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(order_id)
        .execute(pool)
        .await
        .expect("mark SENT");

    let broker_message_id = format!("bm:{order_id}");
    let broker_order_id = format!("bo:{order_id}");
    let ev = BrokerEvent::Fill {
        broker_message_id: broker_message_id.clone(),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: Some(broker_order_id.clone()),
        symbol: symbol.to_string(),
        side,
        delta_qty: qty.parse::<QtyMicros>().expect("qty"),
        price_micros,
        fee_micros: 0,
    };
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        &broker_message_id,
        None,
        order_id,
        &broker_order_id,
        "fill",
        &serde_json::to_value(&ev).expect("event json"),
        0,
        at(),
    )
    .await
    .expect("inbox insert");
    mqk_db::inbox_mark_applied(pool, run_id, &broker_message_id, at())
        .await
        .expect("inbox apply");
}

/// Durable accounting row (via the real authority path) so the closed-trade
/// authority can classify the run `active`.
async fn seed_accounting_state(pool: &sqlx::PgPool, run_id: Uuid, realized_pnl_micros: i64) {
    let snapshot_id = Uuid::new_v4();
    let now = Utc::now();
    mqk_db::insert_or_confirm_paper_portfolio_snapshot(
        pool,
        mqk_db::NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: now,
            deployment_mode: "paper".to_string(),
            source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("snapshot insert");
    let last_applied_inbox_id: i64 = sqlx::query_scalar::<_, Option<i64>>(
        "select max(inbox_id) from oms_inbox where run_id = $1 and applied_at_utc is not null",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .expect("max inbox id")
    .unwrap_or(0);
    mqk_db::upsert_paper_portfolio_accounting_state(
        pool,
        mqk_db::UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 100_000_000_000,
            realized_pnl_micros,
            fees_micros: 0,
            last_applied_inbox_id,
            accounting_epoch: "complete".to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: now,
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("accounting upsert");
}

fn db_state(pool: sqlx::PgPool) -> Arc<state::AppState> {
    Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

fn closure<'a>(v: &'a serde_json::Value, symbol: &str) -> &'a serde_json::Value {
    v["closed_trades_lane"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["symbol"] == symbol)
        .unwrap_or_else(|| panic!("no closure for {symbol}: {v}"))
}

// ---------------------------------------------------------------------------
// paper journal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq04_journal_fractional_long_and_short_closures_v1_409_v2_exact() {
    mqk_db::run_isolated("eq04_journal_fractional", |pool| async move {
        let st = db_state(pool.clone());
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");

        // Long: buy 0.5 @100, sell 0.5 @110 -> +5.0 (5_000_000 micros).
        place_and_fill(
            &pool,
            run_id,
            &unique_id("b"),
            "BTC/USD",
            Side::Buy,
            "0.5",
            100_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "BTC/USD",
            Side::Sell,
            "0.5",
            110_000_000,
            &sid,
        )
        .await;
        // Short: sell 1.500001 @200, buy 1.500001 @190 -> +15.000010.
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "ETH/USD",
            Side::Sell,
            "1.500001",
            200_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("b"),
            "ETH/USD",
            Side::Buy,
            "1.500001",
            190_000_000,
            &sid,
        )
        .await;

        let router = routes::build_router(Arc::clone(&st));
        let (status, v1) = call(router.clone(), "/api/v1/paper/journal").await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "V1 must refuse, not round: {v1}"
        );
        assert_eq!(v1["error"], "quantity_not_representable_in_v1");
        assert!(v1["detail"]
            .as_str()
            .unwrap()
            .contains("/api/v2/paper/journal"));

        let (status, v2) = call(router, "/api/v2/paper/journal").await;
        assert_eq!(status, StatusCode::OK, "{v2}");
        assert_eq!(v2["quantity_schema_version"], V2_VERSION);
        assert_eq!(
            v2["closed_trades_lane"]["rows"].as_array().unwrap().len(),
            2
        );
        let btc = closure(&v2, "BTC/USD");
        assert_eq!(btc["direction"], "long");
        assert_eq!(btc["qty_micros"], 500_000);
        assert_eq!(btc["gross_realized_pnl_micros"], 5_000_000);
        assert!(
            btc.get("qty").is_none(),
            "V2 carries qty_micros only: {btc}"
        );
        let eth = closure(&v2, "ETH/USD");
        assert_eq!(eth["direction"], "short");
        assert_eq!(eth["qty_micros"], 1_500_001);
        assert_eq!(eth["gross_realized_pnl_micros"], 15_000_010);
    })
    .await;
}

#[tokio::test]
async fn eq05_journal_whole_unit_closure_v1_unchanged_v2_exact() {
    mqk_db::run_isolated("eq05_journal_whole", |pool| async move {
        let st = db_state(pool.clone());
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        place_and_fill(
            &pool,
            run_id,
            &unique_id("b"),
            "AAPL",
            Side::Buy,
            "10",
            100_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "AAPL",
            Side::Sell,
            "10",
            110_000_000,
            &sid,
        )
        .await;

        let router = routes::build_router(Arc::clone(&st));
        let (status, v1) = call(router.clone(), "/api/v1/paper/journal").await;
        assert_eq!(status, StatusCode::OK, "{v1}");
        assert_eq!(closure(&v1, "AAPL")["qty"], 10);
        assert!(v1.get("quantity_schema_version").is_none());

        let (status, v2) = call(router, "/api/v2/paper/journal").await;
        assert_eq!(status, StatusCode::OK, "{v2}");
        assert_eq!(closure(&v2, "AAPL")["qty_micros"], 10_000_000);
        assert_eq!(
            closure(&v1, "AAPL")["gross_realized_pnl_micros"],
            closure(&v2, "AAPL")["gross_realized_pnl_micros"]
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// strategy performance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq06_strategy_performance_fractional_v1_409_v2_exact() {
    mqk_db::run_isolated("eq06_perf_fractional", |pool| async move {
        let st = db_state(pool.clone());
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        // buy 0.5 @100, sell 0.25 @110, sell 0.25 @120: two close events.
        place_and_fill(
            &pool,
            run_id,
            &unique_id("b"),
            "BTC/USD",
            Side::Buy,
            "0.5",
            100_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "BTC/USD",
            Side::Sell,
            "0.25",
            110_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "BTC/USD",
            Side::Sell,
            "0.25",
            120_000_000,
            &sid,
        )
        .await;
        // pnl = 0.25*10 + 0.25*20 = 7.5
        seed_accounting_state(&pool, run_id, 7_500_000).await;

        let router = routes::build_router(Arc::clone(&st));
        let q = format!("?run_id={run_id}");
        let (status, v1) = call(router.clone(), &format!("/api/v1/strategy/performance{q}")).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "V1 must refuse, not round: {v1}"
        );
        assert!(v1["detail"]
            .as_str()
            .unwrap()
            .contains("/api/v2/strategy/performance"));

        let (status, v2) = call(router, &format!("/api/v2/strategy/performance{q}")).await;
        assert_eq!(status, StatusCode::OK, "{v2}");
        assert_eq!(v2["truth_state"], "active");
        assert_eq!(v2["quantity_schema_version"], V2_VERSION);
        let row = &v2["rows"][0];
        assert_eq!(row["attributed_closed_qty_micros"], 500_000);
        assert_eq!(row["attributed_close_event_count"], 2);
        assert_eq!(row["gross_realized_pnl_micros"], 7_500_000);
        assert!(row.get("attributed_closed_qty").is_none());
    })
    .await;
}

#[tokio::test]
async fn eq07_strategy_performance_whole_unit_v1_unchanged_v2_exact() {
    mqk_db::run_isolated("eq07_perf_whole", |pool| async move {
        let st = db_state(pool.clone());
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        place_and_fill(
            &pool,
            run_id,
            &unique_id("b"),
            "AAPL",
            Side::Buy,
            "10",
            100_000_000,
            &sid,
        )
        .await;
        place_and_fill(
            &pool,
            run_id,
            &unique_id("s"),
            "AAPL",
            Side::Sell,
            "10",
            110_000_000,
            &sid,
        )
        .await;
        seed_accounting_state(&pool, run_id, 100_000_000).await;

        let router = routes::build_router(Arc::clone(&st));
        let q = format!("?run_id={run_id}");
        let (status, v1) = call(router.clone(), &format!("/api/v1/strategy/performance{q}")).await;
        assert_eq!(status, StatusCode::OK, "{v1}");
        assert_eq!(v1["truth_state"], "active");
        assert_eq!(v1["rows"][0]["attributed_closed_qty"], 10);
        assert!(v1.get("quantity_schema_version").is_none());

        let (status, v2) = call(router, &format!("/api/v2/strategy/performance{q}")).await;
        assert_eq!(status, StatusCode::OK, "{v2}");
        assert_eq!(v2["rows"][0]["attributed_closed_qty_micros"], 10_000_000);
        assert_eq!(
            v1["rows"][0]["gross_realized_pnl_micros"],
            v2["rows"][0]["gross_realized_pnl_micros"]
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// journal admissions lane: a non-integer admission quantity is never a `0` row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq08_journal_admission_with_a_non_integer_qty_is_skipped_never_reported_as_zero() {
    mqk_db::run_isolated("eq08_admission_qty", |pool| async move {
        let st = db_state(pool.clone());
        let run_id = seed_active_run(&st).await;
        for (i, qty) in [serde_json::json!(3), serde_json::json!("0.5")]
            .into_iter()
            .enumerate()
        {
            mqk_db::insert_audit_event(
                &pool,
                &mqk_db::NewAuditEvent {
                    event_id: Uuid::new_v4(),
                    run_id,
                    ts_utc: Utc::now(),
                    topic: "signal_ingestion".to_string(),
                    event_type: "signal.admitted".to_string(),
                    payload: serde_json::json!({
                        "signal_id": format!("sig{i}"),
                        "strategy_id": "s",
                        "symbol": "AAPL",
                        "side": "buy",
                        "qty": qty,
                    }),
                    hash_prev: None,
                    hash_self: None,
                },
            )
            .await
            .expect("audit insert");
        }

        let router = routes::build_router(Arc::clone(&st));
        let (status, v1) = call(router, "/api/v1/paper/journal").await;
        assert_eq!(status, StatusCode::OK, "{v1}");
        let rows = v1["admissions_lane"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "the unparseable row is skipped: {rows:?}");
        assert_eq!(rows[0]["qty"], 3);
        assert_eq!(rows[0]["signal_id"], "sig0");
    })
    .await;
}

// ---------------------------------------------------------------------------
// execution orders (in-memory OMS): exact V2 alongside the documented V1 null
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq09_execution_orders_v2_is_exact_and_v1_keeps_its_documented_null() {
    let (st, router) = no_db_router();
    let mut snap = snapshot(&[]);
    let order = |id: &str, total: &str, filled: &str| mqk_runtime::observability::OrderSnapshot {
        order_id: id.to_string(),
        broker_order_id: None,
        symbol: format!("SYM{id}"),
        total_qty: total.parse::<QtyMicros>().unwrap(),
        filled_qty: filled.parse::<QtyMicros>().unwrap(),
        status: "PartiallyFilled".to_string(),
    };
    snap.active_orders = vec![order("w", "10", "4"), order("f", "0.5", "0.125")];
    st.execution_snapshot.write().await.replace(snap);

    let (status, v1) = call(router.clone(), "/api/v1/execution/orders").await;
    assert_eq!(status, StatusCode::OK);
    let by_id = |v: &serde_json::Value, id: &str| {
        v.as_array()
            .expect("V1 is a bare array")
            .iter()
            .find(|r| r["internal_order_id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id(&v1, "w")["requested_qty"], 10);
    assert_eq!(by_id(&v1, "w")["filled_qty"], 4);
    assert!(
        by_id(&v1, "f")["requested_qty"].is_null(),
        "documented V1 contract: a fractional quantity is null, never truncated or zero"
    );

    let (status, v2) = call(router, "/api/v2/execution/orders").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v2["quantity_schema_version"], V2_VERSION);
    let rows = v2["rows"].as_array().unwrap();
    let get = |id: &str| rows.iter().find(|r| r["internal_order_id"] == id).unwrap();
    assert_eq!(get("w")["requested_qty_micros"], 10_000_000);
    assert_eq!(get("w")["filled_qty_micros"], 4_000_000);
    assert_eq!(get("f")["requested_qty_micros"], 500_000);
    assert_eq!(get("f")["filled_qty_micros"], 125_000);
    assert!(get("f").get("requested_qty").is_none());
}

#[tokio::test]
async fn eq10_execution_orders_v2_without_a_snapshot_is_unavailable_not_an_empty_list() {
    let (_st, router) = no_db_router();
    let (status, v) = call(router, "/api/v2/execution/orders").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"], "no_execution_snapshot");
}
