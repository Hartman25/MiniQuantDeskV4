//! WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01
//!
//! Proves `GET /api/v1/paper/journal`'s `closed_trades_lane`: a deterministic,
//! read-only FIFO closed-trade attribution projection built from the SAME
//! canonical effective-fill replay that feeds `mqk_portfolio`'s FIFO
//! accounting, joined with P1's durable strategy-lineage resolver.
//!
//! All tests run against a real disposable Postgres database created and
//! torn down per-test via `mqk_db::run_isolated` (migrations applied
//! automatically). No `MQK_DATABASE_URL` / `--include-ignored` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_execution::{BrokerEvent, Side};
use tower::ServiceExt;
use uuid::Uuid;

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

fn get(path: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn fetch_journal(st: &Arc<state::AppState>) -> serde_json::Value {
    let router = routes::build_router(Arc::clone(st));
    let (status, body) = call(router, get("/api/v1/paper/journal")).await;
    assert_eq!(status, StatusCode::OK, "paper/journal must return 200");
    serde_json::from_slice(&body).expect("body is not valid JSON")
}

fn closed_trade_rows(journal: &serde_json::Value) -> &Vec<serde_json::Value> {
    journal["closed_trades_lane"]["rows"]
        .as_array()
        .expect("closed_trades_lane.rows must be an array")
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn fingerprint(byte: char) -> String {
    byte.to_string().repeat(64)
}

// ---------------------------------------------------------------------------
// DB fixtures
// ---------------------------------------------------------------------------

/// Seed a RUNNING run in the DB and wire up the local loop handle so
/// `st.current_status_snapshot().await.active_run_id` resolves to it.
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
            config_json: serde_json::json!({"source": "scenario_wave05_strategy_closed_trade_read_model_01"}),
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

    st.inject_running_loop_for_test(run_id).await;
    run_id
}

/// Durable strategy shape for one fixture order's `order_json`.
enum StrategyShape<'a> {
    /// No `strategy_id` key at all -- a genuine manual/operator order.
    Manual,
    /// Fully-bound strategy identity.
    Full {
        strategy_id: &'a str,
        strategy_semantic_fingerprint: &'a str,
    },
    /// `strategy_id` present, `strategy_semantic_fingerprint` key absent
    /// entirely -- a legacy order persisted before fingerprint capture.
    Legacy { strategy_id: &'a str },
    /// `strategy_id` present but blank -- malformed/contradictory durable
    /// attribution field.
    MalformedStrategyId,
}

/// Enqueue a SENT outbox order carrying `order_json` shaped by `shape` --
/// the prerequisite `recover_oms_and_portfolio_traced` requires to construct
/// an `OmsOrder` for `order_id`.
async fn fixture_order(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    qty: i64,
    side_str: &str,
    shape: StrategyShape<'_>,
) {
    let mut json = serde_json::json!({"symbol": symbol, "qty": qty, "side": side_str});
    match shape {
        StrategyShape::Manual => {}
        StrategyShape::Full {
            strategy_id,
            strategy_semantic_fingerprint,
        } => {
            json["strategy_id"] = serde_json::json!(strategy_id);
            json["strategy_semantic_fingerprint"] = serde_json::json!(strategy_semantic_fingerprint);
            json["signal_source"] = serde_json::json!("internal_strategy_decision");
        }
        StrategyShape::Legacy { strategy_id } => {
            json["strategy_id"] = serde_json::json!(strategy_id);
            json["signal_source"] = serde_json::json!("internal_strategy_decision");
        }
        StrategyShape::MalformedStrategyId => {
            json["strategy_id"] = serde_json::json!("");
            json["signal_source"] = serde_json::json!("internal_strategy_decision");
        }
    }
    mqk_db::outbox_enqueue(pool, run_id, order_id, json)
        .await
        .expect("outbox_enqueue should succeed");
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(order_id)
        .execute(pool)
        .await
        .expect("mark outbox SENT should succeed");
}

/// Insert one already-applied inbox row carrying `event`.
#[allow(clippy::too_many_arguments)]
async fn fixture_applied_event(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    broker_fill_id: Option<&str>,
    internal_order_id: &str,
    broker_order_id: &str,
    event_kind: &str,
    event: &BrokerEvent,
    at: DateTime<Utc>,
) {
    let json = serde_json::to_value(event).expect("serialize BrokerEvent");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        broker_message_id,
        broker_fill_id,
        internal_order_id,
        broker_order_id,
        event_kind,
        &json,
        0,
        at,
    )
    .await
    .expect("inbox insert should succeed");
    mqk_db::inbox_mark_applied(pool, run_id, broker_message_id, at)
        .await
        .expect("inbox mark applied should succeed");
}

/// A single-shot order: SENT outbox row for `qty`, immediately filled in one
/// terminal `Fill` event at `price_micros`.
#[allow(clippy::too_many_arguments)]
async fn place_and_fill(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    side: Side,
    qty: i64,
    price_micros: i64,
    shape: StrategyShape<'_>,
    at: DateTime<Utc>,
) {
    let side_str = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    fixture_order(pool, run_id, order_id, symbol, qty, side_str, shape).await;

    let broker_message_id = format!("bm:{order_id}");
    let broker_order_id = format!("bo:{order_id}");
    let ev = BrokerEvent::Fill {
        broker_message_id: broker_message_id.clone(),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: Some(broker_order_id.clone()),
        symbol: symbol.to_string(),
        side,
        delta_qty: qty,
        price_micros,
        fee_micros: 0,
    };
    fixture_applied_event(
        pool,
        run_id,
        &broker_message_id,
        None,
        order_id,
        &broker_order_id,
        "fill",
        &ev,
        at,
    )
    .await;
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap()
}

/// Find the closed-trade row closing exactly `close_internal_order_id` with
/// `qty`, or panic with the full lane dumped for diagnosis.
fn find_closure<'a>(
    journal: &'a serde_json::Value,
    close_internal_order_id: &str,
    qty: i64,
) -> &'a serde_json::Value {
    closed_trade_rows(journal)
        .iter()
        .find(|r| {
            r["close_internal_order_id"].as_str() == Some(close_internal_order_id)
                && r["qty"].as_i64() == Some(qty)
        })
        .unwrap_or_else(|| {
            panic!(
                "no closure row with close_internal_order_id={close_internal_order_id} qty={qty}; \
                 rows={:?}",
                closed_trade_rows(journal)
            )
        })
}

async fn max_applied_inbox_id(pool: &sqlx::PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, Option<i64>>(
        "select max(inbox_id) from oms_inbox where run_id = $1 and applied_at_utc is not null",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .expect("max inbox_id query failed")
    .unwrap_or(0)
}

/// Seed a durable, `accounting_epoch`-classified `sys_paper_portfolio_accounting_state`
/// row for `run_id` via the real DB authority path (a confirmed
/// `external_alpaca` snapshot backing the accounting write) -- never a raw
/// unchecked INSERT.
async fn seed_accounting_state(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    realized_pnl_micros: i64,
    accounting_epoch: &str,
    accounting_epoch_reason: Option<&str>,
) {
    let snapshot_id = Uuid::new_v4();
    let now = Utc::now();
    let insert_outcome = mqk_db::insert_or_confirm_paper_portfolio_snapshot(
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
    .expect("insert_or_confirm_paper_portfolio_snapshot failed");
    assert!(
        matches!(
            insert_outcome,
            mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
        ),
        "expected a fresh snapshot insert: {insert_outcome:?}"
    );

    let last_applied_inbox_id = max_applied_inbox_id(pool, run_id).await;
    let upsert_outcome = mqk_db::upsert_paper_portfolio_accounting_state(
        pool,
        mqk_db::UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 100_000_000_000,
            realized_pnl_micros,
            fees_micros: 0,
            last_applied_inbox_id,
            accounting_epoch: accounting_epoch.to_string(),
            accounting_epoch_reason: accounting_epoch_reason.map(|s| s.to_string()),
            updated_at_utc: now,
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("upsert_paper_portfolio_accounting_state failed");
    assert!(
        matches!(
            upsert_outcome,
            mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }
        ),
        "expected a fresh accounting-state insert: {upsert_outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// CT01 -- simple long round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct01_simple_long_round_trip_attributed() {
    mqk_db::run_isolated("ct01_simple_round_trip", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;

        let sid = unique_id("strat");
        let fp = fingerprint('a');
        let buy_id = unique_id("buy");
        let sell_id = unique_id("sell");

        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp },
            at(),
        )
        .await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp },
            at(),
        )
        .await;

        let journal = fetch_journal(&st).await;
        assert_ne!(
            journal["closed_trades_lane"]["truth_state"], "parity_failed",
            "projection must not fail closed on a clean single round trip; journal={journal}"
        );
        let rows = closed_trade_rows(&journal);
        assert_eq!(rows.len(), 1, "expected exactly one closure fragment; rows={rows:?}");
        let row = &rows[0];
        assert_eq!(row["symbol"], "AAPL");
        assert_eq!(row["qty"], 10);
        assert_eq!(row["direction"], "long");
        assert_eq!(row["entry_price_micros"], 100_000_000);
        assert_eq!(row["exit_price_micros"], 110_000_000);
        assert_eq!(row["gross_realized_pnl_micros"], 100_000_000);
        assert_eq!(row["attribution_state"], "attributed");
        assert_eq!(row["open_strategy_id"].as_str(), Some(sid.as_str()));
        assert_eq!(row["close_strategy_id"].as_str(), Some(sid.as_str()));
        assert_eq!(row["open_strategy_semantic_fingerprint"].as_str(), Some(fp.as_str()));
        assert_eq!(row["close_strategy_semantic_fingerprint"].as_str(), Some(fp.as_str()));
        assert_eq!(
            journal["closed_trades_lane"]["sum_gross_realized_pnl_micros"],
            100_000_000
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT02 -- partial close
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct02_partial_close_produces_two_fifo_fragments() {
    mqk_db::run_isolated("ct02_partial_close", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('b');

        let buy_id = unique_id("buy");
        let sell1_id = unique_id("sell1");
        let sell2_id = unique_id("sell2");

        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell1_id, "AAPL", Side::Sell, 4, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell2_id, "AAPL", Side::Sell, 6, 105_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let rows = closed_trade_rows(&journal);
        assert_eq!(rows.len(), 2, "expected two FIFO closure fragments; rows={rows:?}");

        let f1 = find_closure(&journal, &sell1_id, 4);
        assert_eq!(f1["entry_price_micros"], 100_000_000);
        assert_eq!(f1["exit_price_micros"], 110_000_000);
        assert_eq!(f1["gross_realized_pnl_micros"], 40_000_000);
        assert_eq!(f1["open_internal_order_id"], buy_id);

        let f2 = find_closure(&journal, &sell2_id, 6);
        assert_eq!(f2["entry_price_micros"], 100_000_000);
        assert_eq!(f2["exit_price_micros"], 105_000_000);
        assert_eq!(f2["gross_realized_pnl_micros"], 30_000_000);
        assert_eq!(f2["open_internal_order_id"], buy_id);

        assert_eq!(
            journal["closed_trades_lane"]["sum_gross_realized_pnl_micros"],
            70_000_000
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT03 -- multiple opening lots FIFO
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct03_multiple_opening_lots_close_in_fifo_order() {
    mqk_db::run_isolated("ct03_multi_lot_fifo", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('c');

        let buy1_id = unique_id("buy1");
        let buy2_id = unique_id("buy2");
        let sell_id = unique_id("sell");

        place_and_fill(
            &pool, run_id, &buy1_id, "AAPL", Side::Buy, 5, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &buy2_id, "AAPL", Side::Buy, 5, 120_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 7, 130_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let rows = closed_trade_rows(&journal);
        assert_eq!(rows.len(), 2, "expected two fragments (one per opening lot); rows={rows:?}");

        let f1 = find_closure(&journal, &sell_id, 5);
        assert_eq!(f1["open_internal_order_id"], buy1_id, "first 5 must close against the 100 lot (oldest first)");
        assert_eq!(f1["entry_price_micros"], 100_000_000);
        assert_eq!(f1["gross_realized_pnl_micros"], 150_000_000);

        let f2 = find_closure(&journal, &sell_id, 2);
        assert_eq!(f2["open_internal_order_id"], buy2_id, "remaining 2 must close against the 120 lot");
        assert_eq!(f2["entry_price_micros"], 120_000_000);
        assert_eq!(f2["gross_realized_pnl_micros"], 20_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT04 -- terminal raw-overfill correction
// ---------------------------------------------------------------------------

/// Reproduces the proven production terminal-overfill case: order total=3,
/// partial effective 2, terminal RAW delta_qty=2 (Alpaca paper-WS quirk)
/// which the OMS caps to a true effective remainder of 1. The projection
/// must see economic qty 3 total when later closed, NOT 4 -- proving it
/// consumes the SAME canonical effective-fill replay
/// `recover_oms_and_portfolio` uses, never raw `BrokerEvent.delta_qty`.
#[tokio::test]
async fn ct04_terminal_overfill_correction_reflected_in_closure() {
    mqk_db::run_isolated("ct04_terminal_overfill", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('d');

        let buy_id = unique_id("buyof");
        fixture_order(
            &pool, run_id, &buy_id, "AAPL", 3, "buy",
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp },
        ).await;

        let partial = BrokerEvent::PartialFill {
            broker_message_id: format!("bm:{buy_id}:partial"),
            broker_fill_id: None,
            internal_order_id: buy_id.clone(),
            broker_order_id: Some(format!("bo:{buy_id}")),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 2,
            price_micros: 100_000_000,
            fee_micros: 0,
            cum_qty_after: Some(2),
        };
        fixture_applied_event(
            &pool, run_id, &format!("bm:{buy_id}:partial"), None, &buy_id,
            &format!("bo:{buy_id}"), "partial_fill", &partial, at(),
        ).await;

        // Terminal event carries RAW delta_qty=2 (would overstate to 4
        // total if used directly); true effective remainder is 1.
        let terminal = BrokerEvent::Fill {
            broker_message_id: format!("bm:{buy_id}:term"),
            broker_fill_id: None,
            internal_order_id: buy_id.clone(),
            broker_order_id: Some(format!("bo:{buy_id}")),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 2,
            price_micros: 102_000_000,
            fee_micros: 0,
        };
        fixture_applied_event(
            &pool, run_id, &format!("bm:{buy_id}:term"), None, &buy_id,
            &format!("bo:{buy_id}"), "fill", &terminal, at(),
        ).await;

        // Sell 4 -- one MORE than the true economic total of 3. If the
        // projection wrongly totaled the position at 4 (raw delta double
        // count: 2+2), this fully closes both corrupted lots (qty=4
        // closed). If it correctly totals 3, only 3 close and the 4th
        // share opens a new (uncredited-here) short lot -- so a buggy
        // model and a correct model produce OBSERVABLY DIFFERENT closed
        // qty/pnl here, unlike selling exactly 3 (which both models would
        // satisfy identically).
        let sell_id = unique_id("sellof");
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 4, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        assert_ne!(
            journal["closed_trades_lane"]["truth_state"], "parity_failed",
            "projection must not fail closed on a correct terminal-overfill replay; journal={journal}"
        );
        let rows: Vec<&serde_json::Value> = closed_trade_rows(&journal)
            .iter()
            .filter(|r| r["close_internal_order_id"] == sell_id)
            .collect();
        let total_qty: i64 = rows.iter().filter_map(|r| r["qty"].as_i64()).sum();
        assert_eq!(
            total_qty, 3,
            "economic qty closed must be exactly 3 (2 effective + 1 effective), NOT 4 from raw \
             delta_qty double count; rows={rows:?}"
        );
        let total_pnl: i64 = rows
            .iter()
            .filter_map(|r| r["gross_realized_pnl_micros"].as_i64())
            .sum();
        // 2@100 closed at 110 (+20_000_000) + 1@102 closed at 110 (+8_000_000)
        assert_eq!(total_pnl, 28_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT05 -- duplicate WS/REST fill
// ---------------------------------------------------------------------------

/// The repo's known cross-lane duplicate pattern: a WS partial and a REST
/// recovery delivery of the SAME physical execution (different
/// `broker_message_id`, REST carries a `broker_fill_id`, both carry the same
/// `cum_qty_after` watermark). Must collapse to ONE economic effect and
/// exactly one closure fragment, never two.
#[tokio::test]
async fn ct05_cross_lane_duplicate_fill_produces_one_closure_not_two() {
    mqk_db::run_isolated("ct05_cross_lane_dup", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('e');

        let buy_id = unique_id("buydup");
        fixture_order(
            &pool, run_id, &buy_id, "AAPL", 10, "buy",
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp },
        ).await;

        let ws_ev = BrokerEvent::PartialFill {
            broker_message_id: format!("alpaca:{buy_id}:partial_fill:ws-10"),
            broker_fill_id: None,
            internal_order_id: buy_id.clone(),
            broker_order_id: Some(format!("bo:{buy_id}")),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 10,
            price_micros: 100_000_000,
            fee_micros: 0,
            cum_qty_after: Some(10),
        };
        fixture_applied_event(
            &pool, run_id, "alpaca:ws-10", None, &buy_id, &format!("bo:{buy_id}"),
            "partial_fill", &ws_ev, at(),
        ).await;

        let rest_ev = BrokerEvent::PartialFill {
            broker_message_id: "alpaca-rest-recovery:activity-ct05".to_string(),
            broker_fill_id: Some("activity-ct05".to_string()),
            internal_order_id: buy_id.clone(),
            broker_order_id: Some(format!("bo:{buy_id}")),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 10,
            price_micros: 100_000_000,
            fee_micros: 0,
            cum_qty_after: Some(10),
        };
        fixture_applied_event(
            &pool, run_id, "alpaca-rest-recovery:activity-ct05", Some("activity-ct05"),
            &buy_id, &format!("bo:{buy_id}"), "partial_fill", &rest_ev, at(),
        ).await;

        // Sell 20 -- double the true physical execution size. If the
        // duplicate collapsed correctly (true long = 10), only 10 close
        // here and the other 10 open a new short (no fragment for it). If
        // the duplicate was NOT caught (phantom long = 20), all 20 would
        // close -- an observably different qty/pnl than selling exactly 10
        // (which both a correct and a buggy model would satisfy alike).
        let sell_id = unique_id("selldup");
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 20, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        assert_ne!(journal["closed_trades_lane"]["truth_state"], "parity_failed");
        let rows: Vec<&serde_json::Value> = closed_trade_rows(&journal)
            .iter()
            .filter(|r| r["close_internal_order_id"] == sell_id)
            .collect();
        assert_eq!(
            rows.len(), 1,
            "one physical 10-share execution delivered on two transport lanes must produce \
             exactly one closure fragment, not two; rows={rows:?}"
        );
        assert_eq!(rows[0]["qty"], 10);
        assert_eq!(rows[0]["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT06 -- two strategies, same symbol: cross_strategy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct06_two_strategies_same_symbol_is_cross_strategy_never_fabricated() {
    mqk_db::run_isolated("ct06_cross_strategy", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid_a = unique_id("strat_a");
        let sid_b = unique_id("strat_b");
        let fp_a = fingerprint('f');
        let fp_b = fingerprint('g');

        let buy_id = unique_id("buyxs");
        let sell_id = unique_id("sellxs");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid_a, strategy_semantic_fingerprint: &fp_a }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid_b, strategy_semantic_fingerprint: &fp_b }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let row = find_closure(&journal, &sell_id, 10);
        assert_eq!(row["attribution_state"], "cross_strategy");
        assert_eq!(row["open_strategy_id"].as_str(), Some(sid_a.as_str()));
        assert_eq!(row["close_strategy_id"].as_str(), Some(sid_b.as_str()));
        assert_ne!(row["open_strategy_id"], row["close_strategy_id"]);
        assert_eq!(
            row["gross_realized_pnl_micros"], 100_000_000,
            "gross account closure P&L must still be shown even though it is not attributable"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT07 -- same strategy_id, different semantic fingerprint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct07_same_strategy_id_different_fingerprint_is_semantic_identity_changed() {
    mqk_db::run_isolated("ct07_semantic_drift", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_drift");
        let fp_old = fingerprint('h');
        let fp_new = fingerprint('i');

        let buy_id = unique_id("buydrift");
        let sell_id = unique_id("selldrift");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp_old }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp_new }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let row = find_closure(&journal, &sell_id, 10);
        assert_eq!(row["attribution_state"], "semantic_identity_changed");
        assert_eq!(row["open_strategy_id"], row["close_strategy_id"], "same strategy_id on both sides");
        assert_ne!(
            row["open_strategy_semantic_fingerprint"], row["close_strategy_semantic_fingerprint"],
            "must never collapse a same-id/different-fingerprint pair into attributed"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT08 -- manual/strategy mixing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct08_strategy_opens_manual_closes_is_manual_or_mixed() {
    mqk_db::run_isolated("ct08_manual_mix", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_mix");
        let fp = fingerprint('j');

        let buy_id = unique_id("buymix");
        let sell_id = unique_id("sellmix");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Manual, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let row = find_closure(&journal, &sell_id, 10);
        assert_eq!(row["attribution_state"], "manual_or_mixed");
        assert_eq!(row["open_strategy_id"].as_str(), Some(sid.as_str()));
        assert!(row["close_strategy_id"].is_null(), "manual closing side must have no strategy_id");
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT09 -- legacy fingerprint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct09_legacy_missing_fingerprint_is_lineage_incomplete_never_invented() {
    mqk_db::run_isolated("ct09_legacy_fp", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_legacy");
        let fp = fingerprint('k');

        let buy_id = unique_id("buylegacy");
        let sell_id = unique_id("selllegacy");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Legacy { strategy_id: &sid }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let row = find_closure(&journal, &sell_id, 10);
        assert_eq!(row["attribution_state"], "lineage_incomplete");
        assert_eq!(row["gross_realized_pnl_micros"], 100_000_000, "gross math stays visible");
        assert_eq!(row["open_strategy_id"].as_str(), Some(sid.as_str()));
        assert!(
            row["open_strategy_semantic_fingerprint"].is_null(),
            "a legacy order's missing fingerprint must never be invented/reconstructed"
        );
        assert_eq!(row["close_strategy_semantic_fingerprint"].as_str(), Some(fp.as_str()));
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT10 -- malformed P1 lineage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct10_malformed_lineage_never_upgraded_to_authoritative_attribution() {
    mqk_db::run_isolated("ct10_malformed_lineage", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid_b = unique_id("strat_b");
        let fp_b = fingerprint('m');

        let buy_id = unique_id("buymalformed");
        let sell_id = unique_id("sellmalformed");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::MalformedStrategyId, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid_b, strategy_semantic_fingerprint: &fp_b }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        let row = find_closure(&journal, &sell_id, 10);
        assert_eq!(row["attribution_state"], "lineage_invalid");
        assert!(
            row["open_strategy_id"].is_null(),
            "malformed lineage must never surface a trusted strategy_id; row={row}"
        );
        assert_ne!(
            row["attribution_state"], "attributed",
            "malformed lineage on one side must never be upgraded into an attributed closure"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT11 -- account parity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct11_summed_gross_pnl_matches_canonical_and_durable_accounting_truth() {
    mqk_db::run_isolated("ct11_account_parity", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_parity");
        let fp = fingerprint('n');

        // AAPL: buy10@100, sell10@110 -> +100_000_000
        let aapl_buy = unique_id("aaplbuy");
        let aapl_sell = unique_id("aaplsell");
        place_and_fill(
            &pool, run_id, &aapl_buy, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &aapl_sell, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        // MSFT: buy5@50, sell3@60 (+30_000_000), sell2@55 (+10_000_000)
        let msft_buy = unique_id("msftbuy");
        let msft_sell1 = unique_id("msftsell1");
        let msft_sell2 = unique_id("msftsell2");
        place_and_fill(
            &pool, run_id, &msft_buy, "MSFT", Side::Buy, 5, 50_000_000, StrategyShape::Manual, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &msft_sell1, "MSFT", Side::Sell, 3, 60_000_000, StrategyShape::Manual, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &msft_sell2, "MSFT", Side::Sell, 2, 55_000_000, StrategyShape::Manual, at(),
        ).await;

        let expected_total = 100_000_000 + 30_000_000 + 10_000_000;

        seed_accounting_state(&pool, run_id, expected_total, "complete", None).await;

        let journal = fetch_journal(&st).await;
        assert_eq!(
            journal["closed_trades_lane"]["truth_state"], "active",
            "journal={journal}"
        );
        assert_eq!(
            journal["closed_trades_lane"]["sum_gross_realized_pnl_micros"],
            expected_total
        );
        assert_eq!(journal["closed_trades_lane"]["accounting_epoch"], "complete");

        let rows = closed_trade_rows(&journal);
        let summed: i64 = rows
            .iter()
            .filter_map(|r| r["gross_realized_pnl_micros"].as_i64())
            .sum();
        assert_eq!(
            summed, expected_total,
            "sum across ALL closure fragments (including any cross-strategy/mixed) must equal \
             canonical account replay AND durable accounting truth"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT12 -- incomplete accounting epoch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct12_incomplete_accounting_epoch_never_claims_authoritative_complete_pnl() {
    mqk_db::run_isolated("ct12_incomplete_epoch", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_incomplete");
        let fp = fingerprint('o');

        let buy_id = unique_id("buyincomplete");
        let sell_id = unique_id("sellincomplete");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        // A broker position exists at the broker that this run's fill
        // history cannot explain -- classic inherited/adopted-position
        // incompleteness.
        seed_accounting_state(
            &pool, run_id, 100_000_000, "incomplete",
            Some("broker_position_missing_fill_history:MSFT"),
        ).await;

        let journal = fetch_journal(&st).await;
        assert_eq!(journal["closed_trades_lane"]["truth_state"], "incomplete");
        assert_eq!(journal["closed_trades_lane"]["accounting_epoch"], "incomplete");
        assert_eq!(
            journal["closed_trades_lane"]["accounting_epoch_reason"],
            "broker_position_missing_fill_history:MSFT"
        );
        // Observed fills are still visible -- incompleteness is surfaced
        // via truth_state, not by hiding real closure evidence.
        let rows = closed_trade_rows(&journal);
        assert_eq!(rows.len(), 1, "observed closure fragments must still be visible; rows={rows:?}");
        assert_eq!(rows[0]["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT13 -- short lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ct13_short_open_then_cover_produces_short_closure_with_correct_pnl() {
    mqk_db::run_isolated("ct13_short_lifecycle", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_short");
        let fp = fingerprint('p');

        let short_open_id = unique_id("shortopen");
        let cover_id = unique_id("cover");
        place_and_fill(
            &pool, run_id, &short_open_id, "AAPL", Side::Sell, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &cover_id, "AAPL", Side::Buy, 10, 90_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        let journal = fetch_journal(&st).await;
        assert_ne!(journal["closed_trades_lane"]["truth_state"], "parity_failed");
        let row = find_closure(&journal, &cover_id, 10);
        assert_eq!(row["direction"], "short");
        assert_eq!(row["entry_price_micros"], 100_000_000, "short lot entry is the sell-to-open price");
        assert_eq!(row["exit_price_micros"], 90_000_000, "exit is the covering buy price");
        assert_eq!(
            row["gross_realized_pnl_micros"], 100_000_000,
            "short profit = (entry_short - cover_price) * qty"
        );
        assert_eq!(row["attribution_state"], "attributed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT14 -- stale accounting snapshot must not appear active
// (WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01-REPAIR-01)
// ---------------------------------------------------------------------------

/// Reopens exactly the Bundle-4 defect class this repair closes: a durable
/// accounting row that is `accounting_epoch == "complete"` with realized P&L
/// exactly matching the projection must still NOT be reported active once a
/// NEWER run-scoped snapshot exists that the accounting row's
/// `source_snapshot_id` no longer points at. The shared portfolio-provenance
/// classifier (`classify_portfolio_provenance`) must be the sole authority
/// deciding this -- proven here via the real `GET /api/v1/paper/journal`
/// route, not a unit test of the classifier in isolation.
#[tokio::test]
async fn ct14_stale_accounting_snapshot_must_not_appear_active() {
    mqk_db::run_isolated("ct14_stale_snapshot", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_ct14");
        let fp = fingerprint('q');

        let buy_id = unique_id("buyct14");
        let sell_id = unique_id("sellct14");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        let expected_realized_pnl = 100_000_000;

        // Durable snapshot S1 + accounting row pointing at it, realized P&L
        // exactly matching the projection, epoch complete.
        let now = Utc::now();
        let snapshot_1 = Uuid::new_v4();
        let insert_1 = mqk_db::insert_or_confirm_paper_portfolio_snapshot(
            &pool,
            mqk_db::NewPaperPortfolioSnapshot {
                snapshot_id: snapshot_1,
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
        .expect("insert S1 failed");
        assert!(matches!(
            insert_1,
            mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
        ));

        let last_applied_inbox_id = max_applied_inbox_id(&pool, run_id).await;
        mqk_db::upsert_paper_portfolio_accounting_state(
            &pool,
            mqk_db::UpsertPaperPortfolioAccountingStateArgs {
                run_id,
                cash_micros: 100_000_000_000,
                realized_pnl_micros: expected_realized_pnl,
                fees_micros: 0,
                last_applied_inbox_id,
                accounting_epoch: "complete".to_string(),
                accounting_epoch_reason: None,
                updated_at_utc: now,
                source_snapshot_id: snapshot_1,
            },
        )
        .await
        .expect("upsert accounting for S1 failed");

        // A NEWER authoritative run-scoped snapshot S2 -- accounting is
        // deliberately NOT refreshed to point at it.
        let snapshot_2 = Uuid::new_v4();
        let insert_2 = mqk_db::insert_or_confirm_paper_portfolio_snapshot(
            &pool,
            mqk_db::NewPaperPortfolioSnapshot {
                snapshot_id: snapshot_2,
                captured_at_utc: now + chrono::Duration::seconds(5),
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
        .expect("insert S2 failed");
        assert!(matches!(
            insert_2,
            mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
        ));

        let journal = fetch_journal(&st).await;
        assert_ne!(
            journal["closed_trades_lane"]["truth_state"], "active",
            "a stale accounting row pointing at a superseded snapshot must never appear active \
             even though realized P&L matches exactly; journal={journal}"
        );
        assert_eq!(
            journal["closed_trades_lane"]["accounting_provenance_state"],
            "accounting_snapshot_mismatch",
            "the exact shared-classifier defect must be named, not collapsed into a generic \
             label; journal={journal}"
        );
        // Observed closure fragments may still be visible (non-authoritative).
        let rows = closed_trade_rows(&journal);
        assert_eq!(rows.len(), 1, "rows={rows:?}");
        assert_eq!(
            journal["closed_trades_lane"]["sum_gross_realized_pnl_micros"],
            expected_realized_pnl,
            "matching realized-P&L numbers must not be read as proof of currency"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// CT15 -- stale accounting watermark with unchanged realized P&L
// (WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01-REPAIR-01)
// ---------------------------------------------------------------------------

/// The counterexample realized-P&L equality alone cannot catch: durable
/// accounting is proven current (same snapshot, epoch complete) against the
/// canonical projection's realized P&L -- but AFTER accounting was persisted,
/// another canonical applied fill advanced the replay watermark (opening a
/// new position, realizing zero P&L). The durable accounting row's
/// `last_applied_inbox_id` is now stale relative to the canonical replay
/// even though every P&L number still matches exactly.
#[tokio::test]
async fn ct15_stale_accounting_watermark_with_unchanged_realized_pnl() {
    mqk_db::run_isolated("ct15_stale_watermark", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;
        let sid = unique_id("strat_ct15");
        let fp = fingerprint('r');

        // Buy 10 @ 100, sell 10 @ 110 -> realized P&L = +100_000_000.
        let buy_id = unique_id("buyct15");
        let sell_id = unique_id("sellct15");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        let expected_realized_pnl = 100_000_000;

        let watermark_n = max_applied_inbox_id(&pool, run_id).await;

        // Valid current snapshot/accounting at watermark N.
        seed_accounting_state(&pool, run_id, expected_realized_pnl, "complete", None).await;
        assert_eq!(
            max_applied_inbox_id(&pool, run_id).await,
            watermark_n,
            "seeding accounting must not itself advance the replay watermark"
        );

        // AFTER accounting persistence: one more canonical applied BUY fill
        // opening a new MSFT position -- advances the watermark to N+1 but
        // realizes zero P&L, so the projection's summed realized P&L is
        // untouched.
        let msft_buy_id = unique_id("msftbuyct15");
        place_and_fill(
            &pool, run_id, &msft_buy_id, "MSFT", Side::Buy, 5, 50_000_000,
            StrategyShape::Manual, at(),
        ).await;
        let watermark_n_plus_1 = max_applied_inbox_id(&pool, run_id).await;
        assert_eq!(watermark_n_plus_1, watermark_n + 1);

        let journal = fetch_journal(&st).await;
        assert_eq!(
            journal["closed_trades_lane"]["sum_gross_realized_pnl_micros"],
            expected_realized_pnl,
            "the new BUY must not change summed realized P&L; journal={journal}"
        );
        assert_eq!(
            journal["closed_trades_lane"]["canonical_last_applied_inbox_id"],
            watermark_n_plus_1
        );
        assert_eq!(
            journal["closed_trades_lane"]["accounting_last_applied_inbox_id"],
            watermark_n
        );
        assert_ne!(
            journal["closed_trades_lane"]["truth_state"], "active",
            "P&L equality alone must never bypass a stale accounting watermark; journal={journal}"
        );
        assert_ne!(
            journal["closed_trades_lane"]["truth_state"], "parity_failed",
            "a stale watermark with unchanged realized P&L is not a P&L contradiction; journal={journal}"
        );
        assert_eq!(
            journal["closed_trades_lane"]["accounting_watermark_state"],
            "accounting_watermark_mismatch"
        );
    })
    .await;
}
