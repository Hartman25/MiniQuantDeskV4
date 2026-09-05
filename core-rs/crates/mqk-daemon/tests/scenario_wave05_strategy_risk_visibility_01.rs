//! WAVE05-STRATEGY-RISK-VISIBILITY-01
//!
//! Proves the P5 additions to `GET /api/v1/strategy/performance`:
//! deterministic, VISIBILITY-ONLY strategy-level risk visibility built from
//! P3/P4 plus the existing durable strategy-suppression READ seam
//! (`sys_strategy_suppressions` / `fetch_active_suppression_for_strategy`).
//! No automated suppression, no automated clearing, no order/promotion/
//! accounting mutation anywhere in this route.
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
// DB fixtures (mirrors scenario_wave05_strategy_decay_and_regime_monitor_01.rs)
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
            config_json: serde_json::json!({"source": "scenario_wave05_strategy_risk_visibility_01"}),
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

struct OrderCtx<'a> {
    strategy_id: Option<&'a str>,
    strategy_semantic_fingerprint: Option<&'a str>,
}

impl<'a> OrderCtx<'a> {
    fn manual() -> Self {
        Self { strategy_id: None, strategy_semantic_fingerprint: None }
    }
    fn full(strategy_id: &'a str, fp: &'a str) -> Self {
        Self { strategy_id: Some(strategy_id), strategy_semantic_fingerprint: Some(fp) }
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

#[allow(clippy::too_many_arguments)]
async fn round_trip(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    seq: usize,
    symbol: &str,
    strategy_id: &str,
    fp: &str,
    delta_micros: i64,
    at: DateTime<Utc>,
) {
    const ENTRY_PRICE: i64 = 1_000_000;
    let buy_id = unique_id(&format!("rtbuy{seq}"));
    let sell_id = unique_id(&format!("rtsell{seq}"));
    place_and_fill(pool, run_id, &buy_id, symbol, Side::Buy, 1, ENTRY_PRICE, &OrderCtx::full(strategy_id, fp), at).await;
    place_and_fill(pool, run_id, &sell_id, symbol, Side::Sell, 1, ENTRY_PRICE + delta_micros, &OrderCtx::full(strategy_id, fp), at).await;
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
    assert!(matches!(insert_outcome, mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }));

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
    assert!(matches!(upsert_outcome, mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }));
}

async fn seed_suppression(pool: &sqlx::PgPool, strategy_id: &str) -> Uuid {
    let suppression_id = Uuid::new_v4();
    mqk_db::insert_strategy_suppression(
        pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id,
            strategy_id: strategy_id.to_string(),
            trigger_domain: "risk".to_string(),
            trigger_reason: "operator flagged elevated drawdown".to_string(),
            started_at_utc: Utc::now(),
            note: "".to_string(),
        },
    )
    .await
    .expect("insert_strategy_suppression failed");
    suppression_id
}

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
// P5-01 -- active suppression -> state=suppressed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_01_active_suppression_is_suppressed() {
    mqk_db::run_isolated("p5_01_suppressed", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('a');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, at()).await;
        seed_accounting_state(&pool, run_id, 10).await;
        let suppression_id = seed_suppression(&pool, &sid).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["risk_visibility"]["risk_visibility_state"], "suppressed", "row={row}");
        assert_eq!(row["risk_visibility"]["active_strategy_suppression"], true);
        assert_eq!(row["risk_visibility"]["active_suppression_id"], suppression_id.to_string());
        assert_eq!(row["risk_visibility"]["active_suppression_trigger_domain"], "risk");
        assert_eq!(row["risk_visibility"]["recommended_operator_action"], "already_suppressed");
        assert!(row["risk_visibility"]["risk_flags"]
            .as_array().unwrap().iter().any(|f| f == "active_strategy_suppression"));
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-02 -- suppression keyed strategy_id applies to both fingerprints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_02_suppression_applies_to_both_fingerprints_of_same_strategy() {
    mqk_db::run_isolated("p5_02_both_fp", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp1 = fingerprint('b');
        let fp2 = fingerprint('c');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp1, 10, at()).await;
        round_trip(&pool, run_id, 1, "MSFT", &sid, &fp2, 10, at()).await;
        seed_accounting_state(&pool, run_id, 20).await;
        seed_suppression(&pool, &sid).await;

        let perf = fetch_performance(&st, run_id).await;
        let row1 = find_row(&perf, &sid, &fp1).expect("fp1 row must exist");
        let row2 = find_row(&perf, &sid, &fp2).expect("fp2 row must exist");
        assert_eq!(row1["risk_visibility"]["risk_visibility_state"], "suppressed", "row1={row1}");
        assert_eq!(row2["risk_visibility"]["risk_visibility_state"], "suppressed", "row2={row2}");
        assert_eq!(row1["risk_visibility"]["active_strategy_suppression"], true);
        assert_eq!(row2["risk_visibility"]["active_strategy_suppression"], true);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-03 -- decay_observed without suppression -> watch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_03_decay_observed_without_suppression_is_watch() {
    mqk_db::run_isolated("p5_03_watch", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('d');
        let mut total = 0i64;
        for i in 0..10 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, 10, at()).await;
            total += 10;
        }
        for i in 10..15 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, -10, at()).await;
            total -= 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "decay_observed", "row={row}");
        assert_eq!(row["risk_visibility"]["risk_visibility_state"], "watch");
        assert_eq!(row["risk_visibility"]["recommended_operator_action"], "review");
        assert!(row["risk_visibility"]["risk_flags"]
            .as_array().unwrap().iter().any(|f| f == "gross_expectancy_sign_flip_negative"));
        assert_eq!(row["risk_visibility"]["active_strategy_suppression"], false);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-04 -- insufficient_data -> insufficient_data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_04_insufficient_data_propagates() {
    mqk_db::run_isolated("p5_04_insufficient", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('e');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, at()).await;
        seed_accounting_state(&pool, run_id, 10).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "insufficient_data");
        assert_eq!(row["risk_visibility"]["risk_visibility_state"], "insufficient_data", "row={row}");
        assert_eq!(row["risk_visibility"]["recommended_operator_action"], "insufficient_evidence");
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-05 -- no sign-flip + no suppression -> normal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_05_no_sign_flip_no_suppression_is_normal() {
    mqk_db::run_isolated("p5_05_normal", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('f');
        let mut total = 0i64;
        for i in 0..15 {
            round_trip(&pool, run_id, i, "AAPL", &sid, &fp, 10, at()).await;
            total += 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["decay_monitor"]["decay_state"], "no_expectancy_sign_flip");
        assert_eq!(row["risk_visibility"]["risk_visibility_state"], "normal", "row={row}");
        assert_eq!(row["risk_visibility"]["recommended_operator_action"], "none");
        assert_eq!(row["risk_visibility"]["risk_flags"], serde_json::json!([]));
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-06 -- high_volatility observational regime alone does NOT force watch/suppressed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_06_high_volatility_context_alone_does_not_escalate_risk_state() {
    mqk_db::run_isolated("p5_06_high_vol", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('g');
        // 15 flat/normal-sign events (no_expectancy_sign_flip -> normal) with
        // exact durable timeframe context so regime can resolve.
        let mut total = 0i64;
        for i in 0..15 {
            let entry_px = 1_000_000i64;
            let buy_id = unique_id(&format!("hvbuy{i}"));
            let sell_id = unique_id(&format!("hvsell{i}"));
            place_and_fill(&pool, run_id, &buy_id, "AAPL", Side::Buy, 1, entry_px, &OrderCtx::full(&sid, &fp), at()).await;
            fixture_order(&pool, run_id, &sell_id, "AAPL", 1, "sell", &OrderCtx::full(&sid, &fp)).await;
            // Attach timeframe_secs directly on the closing order.
            sqlx::query("update oms_outbox set order_json = order_json || '{\"timeframe_secs\": 60}'::jsonb where idempotency_key = $1")
                .bind(&sell_id).execute(&pool).await.unwrap();
            let bm = format!("bm:{sell_id}");
            let bo = format!("bo:{sell_id}");
            fixture_applied_event(&pool, run_id, &bm, &sell_id, &bo, "AAPL", Side::Sell, 1, entry_px + 10, at()).await;
            total += 10;
        }
        seed_accounting_state(&pool, run_id, total).await;

        // Wildly oscillating closes -> high realized volatility.
        for i in 0..20 {
            let close = if i % 2 == 0 { 1_000_000 } else { 2_000_000 };
            seed_md_bar(&pool, "AAPL", "1m", 1_700_000_000 + i * 60, close).await;
        }

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        assert_eq!(row["regime_context"]["regime_kind"], "high_volatility", "row={row}");
        assert_eq!(row["decay_monitor"]["decay_state"], "no_expectancy_sign_flip");
        assert_eq!(
            row["risk_visibility"]["risk_visibility_state"], "normal",
            "high_volatility observational context alone must never escalate risk state; row={row}"
        );
        assert!(row["risk_visibility"]["risk_flags"]
            .as_array().unwrap().iter().any(|f| f == "observational_high_volatility_context"));
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-07/08/09 -- coverage flags surface without being assigned to strategy metrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_07_08_09_coverage_flags_surface_on_every_row() {
    mqk_db::run_isolated("p5_07_08_09_flags", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('h');
        // Normal attributed row for sid/fp.
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, at()).await;

        // semantic_identity_changed: same sid, different fingerprints across open/close.
        let sid2 = unique_id("strat2");
        let fp_old = fingerprint('i');
        let fp_new = fingerprint('j');
        place_and_fill(&pool, run_id, &unique_id("sicbuy"), "MSFT", Side::Buy, 1, 1_000_000, &OrderCtx::full(&sid2, &fp_old), at()).await;
        place_and_fill(&pool, run_id, &unique_id("sicsell"), "MSFT", Side::Sell, 1, 1_000_010, &OrderCtx::full(&sid2, &fp_new), at()).await;

        // cross_strategy: two different strategies.
        let sid3 = unique_id("strat3");
        let sid4 = unique_id("strat4");
        place_and_fill(&pool, run_id, &unique_id("xsbuy"), "GOOG", Side::Buy, 1, 1_000_000, &OrderCtx::full(&sid3, &fingerprint('k')), at()).await;
        place_and_fill(&pool, run_id, &unique_id("xssell"), "GOOG", Side::Sell, 1, 1_000_010, &OrderCtx::full(&sid4, &fingerprint('l')), at()).await;

        // manual_or_mixed:
        place_and_fill(&pool, run_id, &unique_id("manbuy"), "TSLA", Side::Buy, 1, 1_000_000, &OrderCtx::manual(), at()).await;
        place_and_fill(&pool, run_id, &unique_id("mansell"), "TSLA", Side::Sell, 1, 1_000_010, &OrderCtx::manual(), at()).await;

        seed_accounting_state(&pool, run_id, 10 + 10 + 10 + 10).await;

        let perf = fetch_performance(&st, run_id).await;
        let row = find_row(&perf, &sid, &fp).expect("row must exist");
        let flags: Vec<String> = row["risk_visibility"]["risk_flags"]
            .as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert!(flags.contains(&"semantic_identity_change_excluded_pnl".to_string()), "flags={flags:?}");
        assert!(flags.contains(&"cross_strategy_closure_pnl".to_string()), "flags={flags:?}");
        assert!(flags.contains(&"manual_mixed_closure_pnl".to_string()), "flags={flags:?}");
        // These closures must never be folded into sid/fp's own exact metrics.
        assert_eq!(row["attributed_close_event_count"], 1);
    })
    .await;
}

// ---------------------------------------------------------------------------
// P5-11 -- recommended action mapping exact (pure, via unit test in lib)
// ---------------------------------------------------------------------------
// See routes::strategy_performance::tests::recommended_action_mapping_is_exact
// in-crate for the exhaustive pure mapping proof.

// ---------------------------------------------------------------------------
// P5-12 -- route call produces zero mutation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p5_12_route_call_is_zero_mutation() {
    mqk_db::run_isolated("p5_12_zero_mutation", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(), state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_run(&st).await;
        let sid = unique_id("strat");
        let fp = fingerprint('m');
        round_trip(&pool, run_id, 0, "AAPL", &sid, &fp, 10, at()).await;
        seed_accounting_state(&pool, run_id, 10).await;
        seed_suppression(&pool, &sid).await;

        async fn counts(pool: &sqlx::PgPool, run_id: Uuid, strategy_id: &str) -> (i64, i64, i64, i64, String) {
            let outbox: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            let inbox: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            let suppressions: i64 = sqlx::query_scalar("select count(*) from sys_strategy_suppressions where strategy_id = $1")
                .bind(strategy_id).fetch_one(pool).await.unwrap();
            let promotions: i64 = sqlx::query_scalar("select count(*) from sys_strategy_promotion_transitions")
                .fetch_one(pool).await.unwrap();
            let status: String = sqlx::query_scalar("select status from runs where run_id = $1")
                .bind(run_id).fetch_one(pool).await.unwrap();
            (outbox, inbox, suppressions, promotions, status)
        }

        let before = counts(&pool, run_id, &sid).await;
        let _ = fetch_performance(&st, run_id).await;
        let after = counts(&pool, run_id, &sid).await;
        assert_eq!(before, after, "P5 route call must mutate nothing");
    })
    .await;
}
