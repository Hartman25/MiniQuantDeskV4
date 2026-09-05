//! WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01
//!
//! Proves `GET /api/v1/strategy/performance`: deterministic exact
//! semantic-strategy performance analytics built on top of the P2
//! closed-trade authority (`resolve_authoritative_closed_trade_view`), which
//! this route shares with the Paper Journal `closed_trades_lane` rather than
//! re-deriving.
//!
//! All tests run against a real disposable Postgres database created and
//! torn down per-test via `mqk_db::run_isolated` (migrations applied
//! automatically). No `MQK_DATABASE_URL` / `--include-ignored` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_daemon::state::BrokerKind;
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

async fn fetch_performance(st: &Arc<state::AppState>, run_id: Uuid) -> serde_json::Value {
    let router = routes::build_router(Arc::clone(st));
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/performance?run_id={run_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "strategy/performance must return 200");
    serde_json::from_slice(&body).expect("body is not valid JSON")
}

fn perf_rows(perf: &serde_json::Value) -> &Vec<serde_json::Value> {
    perf["rows"].as_array().expect("rows must be an array")
}

fn coverage_buckets(perf: &serde_json::Value) -> &Vec<serde_json::Value> {
    perf["attribution_coverage"]
        .as_array()
        .expect("attribution_coverage must be an array")
}

fn find_row<'a>(perf: &'a serde_json::Value, strategy_id: &str, fingerprint: &str) -> Option<&'a serde_json::Value> {
    perf_rows(perf).iter().find(|r| {
        r["strategy_id"].as_str() == Some(strategy_id)
            && r["strategy_semantic_fingerprint"].as_str() == Some(fingerprint)
    })
}

fn find_bucket<'a>(perf: &'a serde_json::Value, attribution_state: &str) -> Option<&'a serde_json::Value> {
    coverage_buckets(perf)
        .iter()
        .find(|b| b["attribution_state"].as_str() == Some(attribution_state))
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn fingerprint(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap()
}

// ---------------------------------------------------------------------------
// DB fixtures (mirrors scenario_wave05_strategy_closed_trade_read_model_01.rs)
// ---------------------------------------------------------------------------

/// Seed a durable run discoverable via `fetch_latest_run_for_engine` /
/// explicit `run_id`. Unlike the Paper Journal scenario harness, this route
/// never reads in-memory active-run state, so no loop injection is needed.
async fn seed_run(st: &Arc<state::AppState>) -> Uuid {
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
            config_json: serde_json::json!({"source": "scenario_wave05_strategy_performance_analytics_01"}),
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

    run_id
}

enum StrategyShape<'a> {
    Manual,
    Full {
        strategy_id: &'a str,
        strategy_semantic_fingerprint: &'a str,
    },
    Legacy {
        strategy_id: &'a str,
    },
    MalformedStrategyId,
}

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

/// Seed a durable, accounting_epoch-classified accounting row via the real
/// DB authority path -- required for `truth_state == "active"`, the ONLY
/// state in which this route emits performance rows/coverage.
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
    assert!(matches!(
        insert_outcome,
        mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
    ));

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
    assert!(matches!(
        upsert_outcome,
        mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }
    ));
}

// ---------------------------------------------------------------------------
// P3-01 -- exact A/fingerprint-1 round trip produces one row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_01_exact_round_trip_produces_one_row() {
    mqk_db::run_isolated("p3_01_round_trip", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('a');

        let buy_id = unique_id("buy");
        let sell_id = unique_id("sell");
        place_and_fill(
            &pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;
        place_and_fill(
            &pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at(),
        ).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert_eq!(perf["pnl_basis"], "gross_realized_before_fees");
        assert_eq!(perf["fee_allocation_state"], "not_allocated_to_strategy_close_events");
        assert_eq!(perf_rows(&perf).len(), 1, "rows={:?}", perf_rows(&perf));
        let row = find_row(&perf, &sid, &fp).expect("row must exist for sid/fp");
        assert_eq!(row["attributed_fragment_count"], 1);
        assert_eq!(row["attributed_close_event_count"], 1);
        assert_eq!(row["attributed_closed_qty"], 10);
        assert_eq!(row["gross_realized_pnl_micros"], 100_000_000);
        assert_eq!(row["winning_close_event_count"], 1);
        assert_eq!(row["losing_close_event_count"], 0);
        assert_eq!(row["hit_rate"], 1.0);
        assert_eq!(row["profit_factor"], serde_json::Value::Null);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-02 -- A/fingerprint-1 and A/fingerprint-2 remain separate rows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_02_same_strategy_id_different_fingerprint_remain_separate_rows() {
    mqk_db::run_isolated("p3_02_fp_split", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp1 = fingerprint('b');
        let fp2 = fingerprint('c');

        // fp1 round trip on AAPL
        let buy1 = unique_id("buy1");
        let sell1 = unique_id("sell1");
        place_and_fill(&pool, run_id, &buy1, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp1 }, at()).await;
        place_and_fill(&pool, run_id, &sell1, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp1 }, at()).await;

        // fp2 round trip on MSFT
        let buy2 = unique_id("buy2");
        let sell2 = unique_id("sell2");
        place_and_fill(&pool, run_id, &buy2, "MSFT", Side::Buy, 5, 50_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp2 }, at()).await;
        place_and_fill(&pool, run_id, &sell2, "MSFT", Side::Sell, 5, 60_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp2 }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000 + 50_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert_eq!(perf_rows(&perf).len(), 2, "must never collapse two fingerprints under one strategy_id; rows={:?}", perf_rows(&perf));
        let row1 = find_row(&perf, &sid, &fp1).expect("fp1 row must exist");
        assert_eq!(row1["gross_realized_pnl_micros"], 100_000_000);
        let row2 = find_row(&perf, &sid, &fp2).expect("fp2 row must exist");
        assert_eq!(row2["gross_realized_pnl_micros"], 50_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-03 -- cross_strategy contributes to coverage but to neither strategy row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_03_cross_strategy_contributes_to_coverage_not_either_row() {
    mqk_db::run_isolated("p3_03_cross_strategy", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid_a = unique_id("strat_a");
        let sid_b = unique_id("strat_b");
        let fp_a = fingerprint('d');
        let fp_b = fingerprint('e');

        let buy_id = unique_id("buyxs");
        let sell_id = unique_id("sellxs");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid_a, strategy_semantic_fingerprint: &fp_a }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid_b, strategy_semantic_fingerprint: &fp_b }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert!(find_row(&perf, &sid_a, &fp_a).is_none(), "sid_a must have no row");
        assert!(find_row(&perf, &sid_b, &fp_b).is_none(), "sid_b must have no row");
        assert_eq!(perf_rows(&perf).len(), 0);
        let bucket = find_bucket(&perf, "cross_strategy").expect("cross_strategy bucket must exist");
        assert_eq!(bucket["fragment_count"], 1);
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-04 -- semantic_identity_changed contributes to coverage, not exact metrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_04_semantic_identity_changed_contributes_to_coverage_not_metrics() {
    mqk_db::run_isolated("p3_04_semantic_drift", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_drift");
        let fp_old = fingerprint('f');
        let fp_new = fingerprint('g');

        let buy_id = unique_id("buydrift");
        let sell_id = unique_id("selldrift");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp_old }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp_new }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert!(find_row(&perf, &sid, &fp_old).is_none());
        assert!(find_row(&perf, &sid, &fp_new).is_none());
        assert_eq!(perf_rows(&perf).len(), 0);
        let bucket = find_bucket(&perf, "semantic_identity_changed").expect("bucket must exist");
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-05 -- manual_or_mixed contributes to coverage, not exact metrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_05_manual_or_mixed_contributes_to_coverage_not_metrics() {
    mqk_db::run_isolated("p3_05_manual_mix", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_mix");
        let fp = fingerprint('h');

        let buy_id = unique_id("buymix");
        let sell_id = unique_id("sellmix");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Manual, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert!(find_row(&perf, &sid, &fp).is_none());
        assert_eq!(perf_rows(&perf).len(), 0);
        let bucket = find_bucket(&perf, "manual_or_mixed").expect("bucket must exist");
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-06 -- legacy fingerprint contributes to lineage_incomplete coverage, not a row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_06_legacy_fingerprint_contributes_to_lineage_incomplete_not_row() {
    mqk_db::run_isolated("p3_06_legacy_fp", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_legacy");
        let fp = fingerprint('i');

        let buy_id = unique_id("buylegacy");
        let sell_id = unique_id("selllegacy");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Legacy { strategy_id: &sid }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert!(find_row(&perf, &sid, &fp).is_none(), "legacy-incomplete closure must never receive a performance row");
        assert_eq!(perf_rows(&perf).len(), 0);
        let bucket = find_bucket(&perf, "lineage_incomplete").expect("bucket must exist");
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-07 -- malformed/missing lineage never becomes performance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_07_malformed_lineage_never_becomes_performance() {
    mqk_db::run_isolated("p3_07_malformed", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid_b = unique_id("strat_b");
        let fp_b = fingerprint('j');

        let buy_id = unique_id("buymalformed");
        let sell_id = unique_id("sellmalformed");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::MalformedStrategyId, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid_b, strategy_semantic_fingerprint: &fp_b }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert!(find_row(&perf, &sid_b, &fp_b).is_none());
        assert_eq!(perf_rows(&perf).len(), 0);
        let bucket = find_bucket(&perf, "lineage_invalid").expect("bucket must exist");
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-08 -- multi-lot single close: 2 fragments, 1 close event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_08_multi_lot_single_close_counts_one_event_two_fragments() {
    mqk_db::run_isolated("p3_08_multi_lot", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_multilot");
        let fp = fingerprint('k');

        let buy1 = unique_id("buy1ml");
        let buy2 = unique_id("buy2ml");
        let sell = unique_id("sellml");
        place_and_fill(&pool, run_id, &buy1, "AAPL", Side::Buy, 5, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &buy2, "AAPL", Side::Buy, 5, 120_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell, "AAPL", Side::Sell, 10, 130_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        // gross pnl = (130-100)*5 + (130-120)*5 = 150_000_000 + 50_000_000
        seed_accounting_state(&pool, run_id, 200_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["attributed_fragment_count"], 2, "two FIFO fragments closed this single sell");
        assert_eq!(row["attributed_close_event_count"], 1, "one economic close order = one close event");
        assert_eq!(row["attributed_closed_qty"], 10);
        assert_eq!(row["gross_realized_pnl_micros"], 200_000_000);
        assert_eq!(row["winning_close_event_count"], 1);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-09 -- hit-rate flats excluded from win/loss denominator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_09_hit_rate_excludes_flat_events_from_denominator() {
    mqk_db::run_isolated("p3_09_flat_excluded", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_flat");
        let fp = fingerprint('l');

        // Win: buy 1@100 sell 1@110 (+10_000_000)
        let win_buy = unique_id("winbuy");
        let win_sell = unique_id("winsell");
        place_and_fill(&pool, run_id, &win_buy, "AAPL", Side::Buy, 1, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &win_sell, "AAPL", Side::Sell, 1, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        // Flat: buy 1@100 sell 1@100 (0)
        let flat_buy = unique_id("flatbuy");
        let flat_sell = unique_id("flatsell");
        place_and_fill(&pool, run_id, &flat_buy, "AAPL", Side::Buy, 1, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &flat_sell, "AAPL", Side::Sell, 1, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        seed_accounting_state(&pool, run_id, 10_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["winning_close_event_count"], 1);
        assert_eq!(row["losing_close_event_count"], 0);
        assert_eq!(row["flat_close_event_count"], 1);
        assert_eq!(row["hit_rate"], 1.0, "flat event must be excluded from the denominator, not counted as a loss");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-10 -- no losses => profit_factor = null, never inf
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_10_no_losses_profit_factor_is_null_never_inf() {
    mqk_db::run_isolated("p3_10_no_losses", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_nolo");
        let fp = fingerprint('m');

        let buy_id = unique_id("buynolo");
        let sell_id = unique_id("sellnolo");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["gross_loss_abs_micros"], 0);
        assert_eq!(row["profit_factor"], serde_json::Value::Null, "must never be a fabricated infinity sentinel");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-11 -- drawdown: event pnl sequence +100, -40, -80, +20
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_11_drawdown_matches_deterministic_sequence() {
    mqk_db::run_isolated("p3_11_drawdown", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_dd");
        let fp = fingerprint('n');

        // Four independent round trips, each producing one close event with
        // an exact gross pnl of +100, -40, -80, +20 (qty=1, price diff IS
        // the pnl in micros). Trades happen in ascending inbox order.
        let deltas: [i64; 4] = [100, -40, -80, 20];
        let mut total: i64 = 0;
        for (i, d) in deltas.iter().enumerate() {
            let entry_px: i64 = 1_000_000;
            let exit_px: i64 = entry_px + d;
            let buy_id = unique_id(&format!("ddbuy{i}"));
            let sell_id = unique_id(&format!("ddsell{i}"));
            place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 1, entry_px,
                StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
            place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 1, exit_px,
                StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
            total += d;
        }

        seed_accounting_state(&pool, run_id, total, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["attributed_close_event_count"], 4);
        // cumulative: 100, 60, -20, 0 ; peak: 100,100,100,100 ; dd: 0,40,120,100 -> max 120
        assert_eq!(row["max_realized_pnl_drawdown_micros"], 120);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-12 -- attribution coverage P&L sum equals account closure P&L
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_12_attribution_coverage_sum_equals_account_closure_pnl() {
    mqk_db::run_isolated("p3_12_coverage_sum", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_cov");
        let fp = fingerprint('o');

        // Attributed: +100_000_000
        let buy1 = unique_id("covbuy1");
        let sell1 = unique_id("covsell1");
        place_and_fill(&pool, run_id, &buy1, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell1, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        // Manual: +30_000_000
        let buy2 = unique_id("covbuy2");
        let sell2 = unique_id("covsell2");
        place_and_fill(&pool, run_id, &buy2, "MSFT", Side::Buy, 5, 50_000_000, StrategyShape::Manual, at()).await;
        place_and_fill(&pool, run_id, &sell2, "MSFT", Side::Sell, 5, 56_000_000, StrategyShape::Manual, at()).await;

        let expected_total = 100_000_000 + 30_000_000;
        seed_accounting_state(&pool, run_id, expected_total, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let coverage_sum: i64 = coverage_buckets(&perf)
            .iter()
            .filter_map(|b| b["gross_realized_pnl_micros"].as_i64())
            .sum();
        assert_eq!(coverage_sum, expected_total, "coverage must reconcile exactly to canonical account closure P&L");
        assert_eq!(perf["total_gross_realized_pnl_micros"], expected_total);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-13 -- stale accounting snapshot: P2 authority non-active -> rows empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_13_stale_accounting_snapshot_yields_empty_rows() {
    mqk_db::run_isolated("p3_13_stale_snapshot", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_p313");
        let fp = fingerprint('p');

        let buy_id = unique_id("buyp313");
        let sell_id = unique_id("sellp313");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        let now = Utc::now();
        let snapshot_1 = Uuid::new_v4();
        mqk_db::insert_or_confirm_paper_portfolio_snapshot(
            &pool,
            mqk_db::NewPaperPortfolioSnapshot {
                snapshot_id: snapshot_1, captured_at_utc: now, deployment_mode: "paper".to_string(),
                source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
                equity_micros: 100_000_000_000, cash_micros: 100_000_000_000, currency: "USD".to_string(),
                truth_state: "active".to_string(), run_id: Some(run_id), operation_id: None, positions: vec![],
            },
        ).await.expect("insert S1 failed");

        let last_applied_inbox_id = max_applied_inbox_id(&pool, run_id).await;
        mqk_db::upsert_paper_portfolio_accounting_state(
            &pool,
            mqk_db::UpsertPaperPortfolioAccountingStateArgs {
                run_id, cash_micros: 100_000_000_000, realized_pnl_micros: 100_000_000, fees_micros: 0,
                last_applied_inbox_id, accounting_epoch: "complete".to_string(), accounting_epoch_reason: None,
                updated_at_utc: now, source_snapshot_id: snapshot_1,
            },
        ).await.expect("upsert accounting for S1 failed");

        // Newer snapshot -- accounting deliberately NOT refreshed to point at it.
        let snapshot_2 = Uuid::new_v4();
        mqk_db::insert_or_confirm_paper_portfolio_snapshot(
            &pool,
            mqk_db::NewPaperPortfolioSnapshot {
                snapshot_id: snapshot_2, captured_at_utc: now + chrono::Duration::seconds(5),
                deployment_mode: "paper".to_string(),
                source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
                equity_micros: 100_000_000_000, cash_micros: 100_000_000_000, currency: "USD".to_string(),
                truth_state: "active".to_string(), run_id: Some(run_id), operation_id: None, positions: vec![],
            },
        ).await.expect("insert S2 failed");

        let perf = fetch_performance(&st, run_id).await;
        assert_ne!(perf["truth_state"], "active", "a stale accounting row pointing at a superseded snapshot must never be treated as authoritative; perf={perf}");
        assert_eq!(perf_rows(&perf).len(), 0);
        assert_eq!(coverage_buckets(&perf).len(), 0);
        assert_eq!(perf["total_gross_realized_pnl_micros"], serde_json::Value::Null);
        assert_eq!(perf["accounting_provenance_state"], "accounting_snapshot_mismatch");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-14 -- stale accounting watermark: P2 authority non-active -> rows empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_14_stale_accounting_watermark_yields_empty_rows() {
    mqk_db::run_isolated("p3_14_stale_watermark", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat_p314");
        let fp = fingerprint('q');

        let buy_id = unique_id("buyp314");
        let sell_id = unique_id("sellp314");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000,
            StrategyShape::Full { strategy_id: &sid, strategy_semantic_fingerprint: &fp }, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        // Advance the canonical replay watermark AFTER accounting persisted,
        // with zero incremental realized P&L (new MSFT long open).
        let msft_buy_id = unique_id("msftbuyp314");
        place_and_fill(&pool, run_id, &msft_buy_id, "MSFT", Side::Buy, 5, 50_000_000, StrategyShape::Manual, at()).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_ne!(perf["truth_state"], "active", "P&L equality alone must never bypass a stale accounting watermark; perf={perf}");
        assert_eq!(perf_rows(&perf).len(), 0);
        assert_eq!(coverage_buckets(&perf).len(), 0);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P3-15 -- no attributed closures but authoritative P2: active + rows=[] is
// a valid authoritative zero
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_15_authoritative_active_with_no_attributed_closures_is_valid_zero() {
    mqk_db::run_isolated("p3_15_authoritative_zero", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;

        // Only a manual round trip -- zero attributed closures, but P2
        // authority can still be fully active.
        let buy_id = unique_id("buyzero");
        let sell_id = unique_id("sellzero");
        place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 10, 100_000_000, StrategyShape::Manual, at()).await;
        place_and_fill(&pool, run_id, &sell_id, "AAPL", Side::Sell, 10, 110_000_000, StrategyShape::Manual, at()).await;

        seed_accounting_state(&pool, run_id, 100_000_000, "complete", None).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        assert_eq!(perf_rows(&perf).len(), 0, "authoritative zero: no attributed closures exist");
        let bucket = find_bucket(&perf, "manual_or_mixed").expect("bucket must exist");
        assert_eq!(bucket["gross_realized_pnl_micros"], 100_000_000);
        assert_eq!(perf["total_gross_realized_pnl_micros"], 100_000_000);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Route resolution edge cases (P3.2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p3_invalid_run_id_is_bounded_invalid_request() {
    mqk_db::run_isolated("p3_invalid_run_id", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let router = routes::build_router(Arc::clone(&st));
        let (status, body) = call(router, get("/api/v1/strategy/performance?run_id=not-a-uuid")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(json["error"], "invalid_request");
        let detail = json["detail"].as_str().expect("detail present");
        assert!(!detail.contains("not-a-uuid"), "must never echo untrusted raw input; detail={detail}");
    })
    .await;
}

#[tokio::test]
async fn p3_no_db_is_db_unavailable() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ));
    let perf = fetch_performance(&st, Uuid::nil()).await;
    assert_eq!(perf["truth_state"], "db_unavailable");
    assert_eq!(perf_rows(&perf).len(), 0);
}

#[tokio::test]
async fn p3_not_found_run_is_not_found() {
    mqk_db::run_isolated("p3_not_found", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let perf = fetch_performance(&st, Uuid::new_v4()).await;
        assert_eq!(perf["truth_state"], "not_found");
        assert_eq!(perf_rows(&perf).len(), 0);
    })
    .await;
}
