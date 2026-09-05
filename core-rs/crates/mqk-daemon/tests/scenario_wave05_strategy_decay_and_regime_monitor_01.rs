//! WAVE05-STRATEGY-DECAY-AND-REGIME-MONITOR-01
//!
//! Proves the P4 additions to `GET /api/v1/strategy/performance`: a
//! conservative, deterministic forward Paper performance-decay monitor over
//! P3's exact `AttributedCloseEvent` series, plus observational
//! research-only current market-regime CONTEXT resolved from the EXACT
//! durable originating order (`oms_outbox.order_json`) -- never from current
//! config/registry state.
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

fn find_row<'a>(perf: &'a serde_json::Value, strategy_id: &str, fingerprint: &str) -> Option<&'a serde_json::Value> {
    perf["rows"]
        .as_array()
        .expect("rows must be an array")
        .iter()
        .find(|r| {
            r["strategy_id"].as_str() == Some(strategy_id)
                && r["strategy_semantic_fingerprint"].as_str() == Some(fingerprint)
        })
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
// DB fixtures
// ---------------------------------------------------------------------------

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
            config_json: serde_json::json!({"source": "scenario_wave05_strategy_decay_and_regime_monitor_01"}),
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

/// Exact strategy/context shape for one fixture order's `order_json`.
struct OrderCtx<'a> {
    strategy_id: Option<&'a str>,
    strategy_semantic_fingerprint: Option<&'a str>,
    timeframe_secs: Option<i64>,
}

impl<'a> OrderCtx<'a> {
    fn manual() -> Self {
        Self { strategy_id: None, strategy_semantic_fingerprint: None, timeframe_secs: None }
    }
    fn full(strategy_id: &'a str, fp: &'a str, timeframe_secs: Option<i64>) -> Self {
        Self { strategy_id: Some(strategy_id), strategy_semantic_fingerprint: Some(fp), timeframe_secs }
    }
}

async fn fixture_order(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    qty: i64,
    side_str: &str,
    ctx: &OrderCtx<'_>,
) {
    let mut json = serde_json::json!({"symbol": symbol, "qty": qty, "side": side_str});
    if let Some(sid) = ctx.strategy_id {
        json["strategy_id"] = serde_json::json!(sid);
        json["signal_source"] = serde_json::json!("internal_strategy_decision");
    }
    if let Some(fp) = ctx.strategy_semantic_fingerprint {
        json["strategy_semantic_fingerprint"] = serde_json::json!(fp);
    }
    if let Some(t) = ctx.timeframe_secs {
        json["timeframe_secs"] = serde_json::json!(t);
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
    internal_order_id: &str,
    broker_order_id: &str,
    symbol: &str,
    side: Side,
    qty: i64,
    price_micros: i64,
    at: DateTime<Utc>,
) {
    let ev = BrokerEvent::Fill {
        broker_message_id: broker_message_id.to_string(),
        broker_fill_id: None,
        internal_order_id: internal_order_id.to_string(),
        broker_order_id: Some(broker_order_id.to_string()),
        symbol: symbol.to_string(),
        side,
        delta_qty: qty,
        price_micros,
        fee_micros: 0,
    };
    let json = serde_json::to_value(&ev).expect("serialize BrokerEvent");
    mqk_db::inbox_insert_deduped_with_identity(
        pool, run_id, broker_message_id, None, internal_order_id, broker_order_id, "fill", &json, 0, at,
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
    ctx: &OrderCtx<'_>,
    at: DateTime<Utc>,
) {
    let side_str = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    fixture_order(pool, run_id, order_id, symbol, qty, side_str, ctx).await;
    let broker_message_id = format!("bm:{order_id}");
    let broker_order_id = format!("bo:{order_id}");
    fixture_applied_event(
        pool, run_id, &broker_message_id, order_id, &broker_order_id, symbol, side, qty, price_micros, at,
    )
    .await;
}

/// One attributed round-trip close event: buy 1 @ `entry_price_micros`, then
/// sell 1 @ `entry_price_micros + delta_micros` -- gross pnl == delta_micros
/// exactly (qty=1). The CLOSING (sell) order carries `timeframe_secs` when
/// given; the opening (buy) order never does (only the closing order's
/// context matters for P4.6 resolution).
#[allow(clippy::too_many_arguments)]
async fn round_trip(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    seq: usize,
    symbol: &str,
    strategy_id: &str,
    fp: &str,
    delta_micros: i64,
    close_timeframe_secs: Option<i64>,
    at: DateTime<Utc>,
) {
    const ENTRY_PRICE: i64 = 1_000_000;
    let buy_id = unique_id(&format!("rtbuy{seq}"));
    let sell_id = unique_id(&format!("rtsell{seq}"));
    place_and_fill(
        pool, run_id, &buy_id, symbol, Side::Buy, 1, ENTRY_PRICE,
        &OrderCtx::full(strategy_id, fp, None), at,
    ).await;
    place_and_fill(
        pool, run_id, &sell_id, symbol, Side::Sell, 1, ENTRY_PRICE + delta_micros,
        &OrderCtx::full(strategy_id, fp, close_timeframe_secs), at,
    ).await;
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

async fn seed_accounting_state(pool: &sqlx::PgPool, run_id: Uuid, realized_pnl_micros: i64) {
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
            accounting_epoch: "complete".to_string(),
            accounting_epoch_reason: None,
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

/// Seed one completed `md_bars` row directly (no ingest pipeline / provider
/// call -- read-only regime detection consumes whatever is already durable).
async fn seed_md_bar(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts: i64, close_micros: i64) {
    sqlx::query(
        r#"
        insert into md_bars (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete)
        values ($1, $2, $3, $4, $4, $4, $4, 1000, true)
        on conflict (symbol, timeframe, end_ts) do nothing
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(close_micros)
    .execute(pool)
    .await
    .expect("seed_md_bar insert failed");
}

// ---------------------------------------------------------------------------
// P4-01 -- 14 close events -> insufficient_data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_01_fourteen_events_is_insufficient_data() {
    mqk_db::run_isolated("p4_01_fourteen", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('a');

        let mut total = 0i64;
        for i in 0..14 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, 10, None, at()).await;
            total += 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["attributed_close_event_count"], 14);
        assert_eq!(row["decay_monitor"]["decay_state"], "insufficient_data");
        assert_eq!(row["decay_monitor"]["baseline"], serde_json::Value::Null);
        assert_eq!(row["decay_monitor"]["recent"], serde_json::Value::Null);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-02 -- exactly 15 -> baseline 10 / recent 5, no overlap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_02_exactly_fifteen_splits_baseline_ten_recent_five_no_overlap() {
    mqk_db::run_isolated("p4_02_fifteen", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('b');

        // deltas 1..=15 -> baseline (first 10) sum = 55, recent (last 5) sum = 65
        let mut total = 0i64;
        for i in 0..15 {
            let delta = (i as i64) + 1;
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, delta, None, at()).await;
            total += delta;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_ne!(row["decay_monitor"]["decay_state"], "insufficient_data", "row={row}");
        let baseline = &row["decay_monitor"]["baseline"];
        let recent = &row["decay_monitor"]["recent"];
        assert_eq!(baseline["event_count"], 10);
        assert_eq!(recent["event_count"], 5);
        assert_eq!(baseline["gross_realized_pnl_micros"], 55);
        assert_eq!(recent["gross_realized_pnl_micros"], 65);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-03/04/05/06 -- decay classifier sign-flip rules
// ---------------------------------------------------------------------------

async fn build_decay_scenario(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    sid: &str,
    fp: &str,
    baseline_delta: i64,
    recent_delta: i64,
) -> i64 {
    let mut total = 0i64;
    for i in 0..10 {
        round_trip(pool, run_id, i, "AAPL", sid, fp, baseline_delta, None, at()).await;
        total += baseline_delta;
    }
    for i in 10..15 {
        round_trip(pool, run_id, i, "AAPL", sid, fp, recent_delta, None, at()).await;
        total += recent_delta;
    }
    total
}

#[tokio::test]
async fn p4_03_baseline_positive_recent_negative_is_decay_observed() {
    mqk_db::run_isolated("p4_03_decay", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('c');
        let total = build_decay_scenario(&pool, run_id, &sid, &fp, 10, -10).await;
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "decay_observed", "row={row}");
    })
    .await;
}

#[tokio::test]
async fn p4_04_baseline_nonpositive_recent_positive_is_improvement_observed() {
    mqk_db::run_isolated("p4_04_improve", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('d');
        let total = build_decay_scenario(&pool, run_id, &sid, &fp, -10, 10).await;
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "improvement_observed", "row={row}");
    })
    .await;
}

#[tokio::test]
async fn p4_05_same_sign_positive_is_no_expectancy_sign_flip() {
    mqk_db::run_isolated("p4_05_same_pos", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('e');
        let total = build_decay_scenario(&pool, run_id, &sid, &fp, 10, 5).await;
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "no_expectancy_sign_flip", "row={row}");
    })
    .await;
}

#[tokio::test]
async fn p4_06_same_sign_negative_is_no_expectancy_sign_flip() {
    mqk_db::run_isolated("p4_06_same_neg", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('f');
        let total = build_decay_scenario(&pool, run_id, &sid, &fp, -10, -5).await;
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "no_expectancy_sign_flip", "row={row}");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-07 -- changing fingerprint splits samples
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_07_fingerprint_change_splits_samples() {
    mqk_db::run_isolated("p4_07_fp_split", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp_old = fingerprint('g');
        let fp_new = fingerprint('h');

        let mut total = 0i64;
        for i in 0..10 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp_old, 10, None, at()).await;
            total += 10;
        }
        for i in 10..15 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp_new, 10, None, at()).await;
            total += 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let new_row = find_row(&perf, &sid, &fp_new).expect("fp_new row must exist");
        assert_eq!(new_row["attributed_close_event_count"], 5);
        assert_eq!(
            new_row["decay_monitor"]["decay_state"], "insufficient_data",
            "fp_new's 5 events must never be topped up by fp_old's events; row={new_row}"
        );
        let old_row = find_row(&perf, &sid, &fp_old).expect("fp_old row must exist");
        assert_eq!(old_row["attributed_close_event_count"], 10);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-08 -- cross/manual/legacy closure events do not count toward sample size
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_08_manual_closures_do_not_count_toward_sample_size() {
    mqk_db::run_isolated("p4_08_manual_excluded", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('i');

        let mut total = 0i64;
        for i in 0..10 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, 10, None, at()).await;
            total += 10;
        }
        // 5 manual round trips on a different symbol -- real closures, but
        // never attributed to `sid`/`fp`.
        for i in 10..15 {
            let buy_id = unique_id(&format!("manbuy{i}"));
            let sell_id = unique_id(&format!("mansell{i}"));
            place_and_fill(&pool, run_id, &buy_id, "MSFT", Side::Buy, 1, 1_000_000, &OrderCtx::manual(), at()).await;
            place_and_fill(&pool, run_id, &sell_id, "MSFT", Side::Sell, 1, 1_000_010, &OrderCtx::manual(), at()).await;
            total += 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["attributed_close_event_count"], 10, "manual closures must never inflate the attributed sample size");
        assert_eq!(row["decay_monitor"]["decay_state"], "insufficient_data");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-09 -- exact durable timeframe context used even when current env/config differs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_09_exact_durable_timeframe_context_used() {
    mqk_db::run_isolated("p4_09_exact_context", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('j');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, Some(300), at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["symbol"], "AAPL");
        assert_eq!(row["regime_context"]["timeframe_secs"], 300);
        assert_eq!(row["regime_context"]["regime_authority"], "research_only_observational");
        assert_ne!(row["regime_context"]["regime_truth_state"], "context_unavailable", "row={row}");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-10 -- conflicting exact timeframes -> context_ambiguous
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_10_conflicting_exact_timeframes_is_context_ambiguous() {
    mqk_db::run_isolated("p4_10_ambiguous", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('k');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, Some(60), at()).await;
        round_trip(&pool, run_id, 1, "AAPL", &sid, &fp, 10, Some(300), at()).await;
        seed_accounting_state(&pool, run_id, 20).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["regime_truth_state"], "context_ambiguous", "row={row}");
        assert_eq!(row["regime_context"]["symbol"], serde_json::Value::Null);
        assert_eq!(row["regime_context"]["timeframe_secs"], serde_json::Value::Null);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-11 -- malformed/missing exact context -> context_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_11_missing_exact_context_is_context_unavailable() {
    mqk_db::run_isolated("p4_11_unavailable", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('l');
        // No timeframe_secs on the closing order.
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, None, at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["regime_truth_state"], "context_unavailable", "row={row}");
        assert_eq!(row["regime_context"]["regime_kind"], serde_json::Value::Null);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-12/13 -- completed md_bars only (no network); result marked research-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_12_13_completed_bars_only_and_marked_research_only_observational() {
    mqk_db::run_isolated("p4_12_13_bars", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('m');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, Some(60), at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        // 20 completed 1m bars -- comfortably above MarketRegimePolicy::conservative_defaults().min_bars (8).
        for i in 0..20 {
            seed_md_bar(&pool, "AAPL", "1m", 1_700_000_000 + i * 60, 1_000_000 + i * 100).await;
        }

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["regime_authority"], "research_only_observational");
        assert_eq!(row["regime_context"]["regime_truth_state"], "active_observational", "row={row}");
        assert!(row["regime_context"]["regime_kind"].is_string());
        assert_eq!(row["regime_context"]["input_bar_count"], 20);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-14 -- insufficient detector bars -> insufficient_data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_14_insufficient_detector_bars_is_insufficient_data() {
    mqk_db::run_isolated("p4_14_insufficient_bars", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('n');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, Some(60), at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        // Only 3 completed bars -- below min_bars (8).
        for i in 0..3 {
            seed_md_bar(&pool, "AAPL", "1m", 1_700_000_000 + i * 60, 1_000_000).await;
        }

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["regime_truth_state"], "insufficient_data", "row={row}");
        assert_eq!(row["regime_context"]["regime_kind"], "insufficient_data");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P4-15 -- regime observation does not alter outbox/inbox/suppression/promotion/run status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p4_15_route_call_is_zero_mutation() {
    mqk_db::run_isolated("p4_15_zero_mutation", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('o');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, Some(60), at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        async fn counts(pool: &sqlx::PgPool, run_id: Uuid) -> (i64, i64, String) {
            let outbox: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            let inbox: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            let status: String = sqlx::query_scalar("select status from runs where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            (outbox, inbox, status)
        }

        let before = counts(&pool, run_id).await;
        let _ = fetch_performance(&st, run_id).await;
        let after = counts(&pool, run_id).await;
        assert_eq!(before, after, "route call must mutate nothing");
    })
    .await;
}
