//! AUTON-NO-SIGNAL-OBS-01 — durable strategy signal-evaluation journal proof.
//!
//! Closes the observability gap left open by the AUTON-NO-TRADE-01 market-hours
//! paper proof: a strategy could evaluate real bars and legitimately produce
//! no signal, and the only evidence was readiness-surface formatting computed
//! live from in-memory atomics (`AppState::last_bar_signal_qty` et al.) — none
//! of it survived a daemon restart.
//!
//! This suite proves `AppState::dispatch_native_strategy_for_symbol_with_bar`
//! now writes one durable `strategy_signal_evaluations` row per dispatch
//! attempt, via the real production dispatch path
//! (`tick_strategy_dispatch_for_symbol`) — not just the raw `mqk_db` helpers.
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                                |
//! |-------|--------------------------------------------------------------------------------|
//! | SO-01 | DB-loaded dispatch persists full context (strategy/symbol/timeframe/bars/      |
//! |       | latest bar ts/signal truth) and creates zero new `oms_outbox` rows            |
//! | SO-02 | Missing bars (pre-dispatch gate) persists `bars_loaded=0`, no fabricated ts  |
//! | SO-03 | Stale bars (pre-dispatch gate) persists the stale bar's own timestamp        |
//! | SO-04 | An active run's `run_id` is durably linked onto the journal row              |
//! | SO-05 | `mqk_db` insert/fetch round-trip is idempotent (dup `evaluation_id` is a      |
//! |       | no-op) and orders newest-first                                               |
//! | SO-06 | `GET /api/v1/execution/signal-evaluations` returns honest truth_state in     |
//! |       | both the seeded (`active`) and no-DB (`db_unavailable`) cases                |
//!
//! All DB-backed tests use the established graceful-skip pattern (matching
//! `scenario_md_staleness_per_tick_gate_01.rs`): without `MQK_DATABASE_URL`
//! pointing at the local paper DB (port 5440), each prints a skip notice and
//! returns (passes trivially). SO-06's `db_unavailable` half needs no DB at
//! all and always runs.
//!
//! No broker/provider calls. No paper/live orders submitted. No strategy
//! threshold/gate logic touched — only an additive, best-effort journal write
//! observed from outside.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use mqk_daemon::state::{self, AppState, StrategyBarInput};
use mqk_daemon::{routes::build_router, state::BrokerKind};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors scenario_md_staleness_per_tick_gate_01.rs)
// ---------------------------------------------------------------------------

/// Same paper-DB allowlist guard used elsewhere in this test suite — refuses
/// to run DB-backed assertions against any database that isn't the local
/// port-5440 paper DB.
fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

fn active_bootstrap() -> NativeStrategyBootstrap {
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

fn test_bar_input(now_tick: u64) -> StrategyBarInput {
    StrategyBarInput {
        now_tick,
        end_ts: 1_700_000_000,
        limit_price: Some(150_000_000),
        qty: 10,
    }
}

async fn connect(db_url: &str) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .expect("pool connect failed")
}

/// Build a DB-backed `AppState` with an active native strategy bootstrap
/// (`swing_momentum`), matching `scenario_md_staleness_per_tick_gate_01.rs`'s
/// `db_state` shape so this suite exercises the exact same dispatch path.
async fn db_state(pool: sqlx::PgPool) -> Arc<AppState> {
    let st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    );
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    Arc::new(st)
}

/// Replace any existing `md_bars` rows for `symbol`/`timeframe` with a single
/// completed bar at `end_ts`. Raw SQL: no insert helper exists for `md_bars`
/// outside the CSV/provider ingest paths.
async fn seed_one_bar(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts: i64) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");

    sqlx::query(
        "insert into md_bars \
           (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete) \
         values ($1, $2, $3, $4, $4, $4, $4, $5, true)",
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(100_000_000_i64)
    .bind(1_000_i64)
    .execute(pool)
    .await
    .expect("insert md_bars fixture row");
}

/// Remove any existing `md_bars` rows for `symbol`/`timeframe`, leaving none.
async fn seed_no_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");
}

/// Pre-test hygiene: remove any prior `strategy_signal_evaluations` rows for
/// `symbol` so each test starts from a clean slate.
async fn cleanup_signal_evaluations(pool: &sqlx::PgPool, symbol: &str) {
    sqlx::query("delete from strategy_signal_evaluations where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await
        .expect("pre-test strategy_signal_evaluations cleanup failed");
}

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    (status, body)
}

// ---------------------------------------------------------------------------
// SO-01 — DB-loaded dispatch persists full context, zero new oms_outbox rows
// ---------------------------------------------------------------------------

/// SO-01: A real dispatch through `tick_strategy_dispatch_for_symbol` against
/// a fresh, DB-loaded bar window writes exactly one durable journal row whose
/// fields are bit-for-bit truthful against what the strategy actually saw and
/// produced — and creates zero new `oms_outbox` rows, proving this is purely
/// observability with no order-path side effect.
#[tokio::test]
async fn so01_db_loaded_evaluation_persists_with_full_context_and_no_outbox_rows() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-01: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS01EVAL";
    let timeframe = "1Min";
    let now_ts = Utc::now().timestamp();
    let bar_end_ts = now_ts - 60; // fresh: well within any reasonable staleness cap

    seed_one_bar(&pool, symbol, timeframe, bar_end_ts).await;
    cleanup_signal_evaluations(&pool, symbol).await;

    let outbox_before: i64 = sqlx::query_scalar(
        "select count(*) from oms_outbox where order_json->>'symbol' = $1",
    )
    .bind(symbol)
    .fetch_one(&pool)
    .await
    .expect("oms_outbox pre-count failed");

    let st = db_state(pool.clone()).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(3600));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    let bar_result = result.expect("SO-01: fresh DB-loaded bar must dispatch (Some)");
    let expected_signal_qty: i64 = bar_result
        .intents
        .output
        .targets
        .iter()
        .map(|t| t.qty)
        .sum();

    let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
        .await
        .expect("fetch_recent_strategy_signal_evaluations failed");
    let row = rows
        .iter()
        .find(|r| r.symbol == symbol)
        .expect("SO-01: exactly one journal row must exist for this symbol");

    assert_eq!(row.strategy_id, "swing_momentum", "SO-01: strategy_id must round-trip");
    assert_eq!(row.symbol, symbol, "SO-01: symbol must round-trip");
    assert_eq!(row.timeframe, timeframe, "SO-01: timeframe must round-trip");
    assert_eq!(row.bar_context_source, "db_loaded", "SO-01: bar_context_source must be db_loaded");
    assert_eq!(row.bars_loaded, 1, "SO-01: exactly one md_bars row was seeded");
    assert_eq!(
        row.latest_bar_ts_utc.map(|t| t.timestamp()),
        Some(bar_end_ts),
        "SO-01: latest_bar_ts_utc must be the seeded bar's own end_ts"
    );
    assert_eq!(
        row.signal_qty,
        Some(expected_signal_qty),
        "SO-01: signal_qty must equal the strategy's real target-qty sum"
    );
    assert_eq!(
        row.signal_generated,
        expected_signal_qty != 0,
        "SO-01: signal_generated must agree with signal_qty"
    );
    assert_eq!(
        row.decision_stage, "strategy_evaluated",
        "SO-01: the strategy's on_bar ran to completion"
    );
    assert!(!row.reason_code.is_empty(), "SO-01: reason_code must be present");
    assert!(!row.reason.is_empty(), "SO-01: reason must be present");
    assert_eq!(row.run_id, None, "SO-01: no run was active in this test");

    let outbox_after: i64 = sqlx::query_scalar(
        "select count(*) from oms_outbox where order_json->>'symbol' = $1",
    )
    .bind(symbol)
    .fetch_one(&pool)
    .await
    .expect("oms_outbox post-count failed");
    assert_eq!(
        outbox_before, outbox_after,
        "SO-01: signal-evaluation persistence must never create oms_outbox rows"
    );
}

// ---------------------------------------------------------------------------
// SO-02 — missing bars (pre-dispatch gate) persists bars_loaded=0
// ---------------------------------------------------------------------------

/// SO-02: When `md_bars` has no completed rows at all, the existing
/// MD-STALENESS-PER-TICK-GATE-01 fail-closed gate still refuses dispatch
/// (`None`, unchanged) — and a journal row is now durably written recording
/// that the strategy never ran, with `bars_loaded=0` and no fabricated
/// timestamp.
#[tokio::test]
async fn so02_missing_bars_persists_pre_dispatch_gate_with_zero_bars_loaded() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-02: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS02MISSING";
    let timeframe = "1Min";

    seed_no_bars(&pool, symbol, timeframe).await;
    cleanup_signal_evaluations(&pool, symbol).await;

    let st = db_state(pool.clone()).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(
        result.is_none(),
        "SO-02: missing bars must still fail closed (unchanged gate behavior)"
    );

    let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
        .await
        .expect("fetch_recent_strategy_signal_evaluations failed");
    let row = rows
        .iter()
        .find(|r| r.symbol == symbol)
        .expect("SO-02: a journal row must exist even though the strategy never ran");

    assert_eq!(row.bars_loaded, 0, "SO-02: zero bars were available");
    assert_eq!(row.latest_bar_ts_utc, None, "SO-02: no bar exists to report a timestamp for");
    assert!(!row.signal_generated, "SO-02: no signal — the strategy never ran");
    assert_eq!(row.signal_qty, None, "SO-02: signal_qty must be honestly absent, not zero");
    assert_eq!(
        row.bar_context_source, "no_bars_available",
        "SO-02: bar_context_source must name the missing-bars case"
    );
    assert_eq!(
        row.decision_stage, "pre_dispatch_gate",
        "SO-02: a pre-dispatch gate refused before on_bar ran"
    );
    assert!(!row.reason_code.is_empty(), "SO-02: reason_code must be present");
}

// ---------------------------------------------------------------------------
// SO-03 — stale bars (pre-dispatch gate) persists the stale bar's timestamp
// ---------------------------------------------------------------------------

/// SO-03: A completed bar far older than the staleness cap still fails closed
/// (unchanged gate behavior) — and the journal row records the *real* stale
/// bar's own timestamp (not the dispatch-time `now`), with `bars_loaded=1`.
#[tokio::test]
async fn so03_stale_bars_persists_pre_dispatch_gate_with_stale_bar_ts() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-03: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS03STALE";
    let timeframe = "1Min";
    let now_ts = Utc::now().timestamp();
    let bar_end_ts = now_ts - 10_000; // ~2.8 hours old, far beyond a 300s cap

    seed_one_bar(&pool, symbol, timeframe, bar_end_ts).await;
    cleanup_signal_evaluations(&pool, symbol).await;

    let st = db_state(pool.clone()).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(
        result.is_none(),
        "SO-03: stale bar must still fail closed (unchanged gate behavior)"
    );

    let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
        .await
        .expect("fetch_recent_strategy_signal_evaluations failed");
    let row = rows
        .iter()
        .find(|r| r.symbol == symbol)
        .expect("SO-03: a journal row must exist even though the strategy never ran");

    assert_eq!(row.bars_loaded, 1, "SO-03: exactly one (stale) bar was seeded");
    assert_eq!(
        row.latest_bar_ts_utc.map(|t| t.timestamp()),
        Some(bar_end_ts),
        "SO-03: latest_bar_ts_utc must be the real stale bar's own end_ts"
    );
    assert!(!row.signal_generated, "SO-03: no signal — the strategy never ran");
    assert_eq!(
        row.bar_context_source, "stale_bars",
        "SO-03: bar_context_source must name the stale-bars case"
    );
    assert_eq!(
        row.decision_stage, "pre_dispatch_gate",
        "SO-03: a pre-dispatch gate refused before on_bar ran"
    );
}

// ---------------------------------------------------------------------------
// SO-04 — active run's run_id is durably linked
// ---------------------------------------------------------------------------

/// SO-04: When a real run is active (established via the daemon's own
/// DB-backed test seam), the journal row carries that exact `run_id` — proof
/// that the evaluation can be tied back to a specific run after restart.
#[tokio::test]
async fn so04_active_run_id_links_onto_journal_row() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS04RUNID";
    let timeframe = "1Min";
    let now_ts = Utc::now().timestamp();

    seed_one_bar(&pool, symbol, timeframe, now_ts - 60).await;
    cleanup_signal_evaluations(&pool, symbol).await;

    let st = db_state(pool.clone()).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(3600));

    let run_id = Uuid::new_v4();
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("pre-test runs cleanup failed");
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("SO-04: establish_db_backed_active_run_for_test failed");

    st.deposit_strategy_bar_input(test_bar_input(1)).await;
    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(result.is_some(), "SO-04: fresh bar must dispatch normally");

    let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
        .await
        .expect("fetch_recent_strategy_signal_evaluations failed");
    let row = rows
        .iter()
        .find(|r| r.symbol == symbol)
        .expect("SO-04: a journal row must exist");

    assert_eq!(
        row.run_id,
        Some(run_id),
        "SO-04: run_id must durably link to the active run"
    );
}

// ---------------------------------------------------------------------------
// SO-05 — mqk_db round-trip is idempotent and orders newest-first
// ---------------------------------------------------------------------------

/// SO-05: Direct `mqk_db` proof, independent of `AppState` — a duplicate
/// insert attempt with the same deterministic `evaluation_id` is a no-op
/// (matches the `ON CONFLICT DO NOTHING` contract every sibling telemetry
/// table in this repo uses), and `fetch_recent_strategy_signal_evaluations`
/// returns newest-first.
#[tokio::test]
async fn so05_insert_is_idempotent_and_fetch_orders_newest_first() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS05IDEMP";
    cleanup_signal_evaluations(&pool, symbol).await;

    let evaluation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.signal-evaluation.v1|test|so05|{symbol}|1Min|1").as_bytes(),
    );
    let older_ts = Utc::now() - chrono::Duration::seconds(60);
    let newer_ts = Utc::now();

    let older_args = mqk_db::InsertStrategySignalEvaluationArgs {
        evaluation_id,
        ts_utc: older_ts,
        run_id: None,
        strategy_id: "swing_momentum".to_string(),
        symbol: symbol.to_string(),
        timeframe: "1Min".to_string(),
        bar_context_source: "db_loaded".to_string(),
        bars_loaded: 5,
        latest_bar_ts_utc: Some(older_ts),
        signal_generated: false,
        signal_qty: Some(0),
        signal_side: None,
        reason_code: "flat_below_threshold".to_string(),
        reason: "abs move_bps below threshold; insufficient displacement for signal".to_string(),
        decision_stage: "strategy_evaluated".to_string(),
        source: "test".to_string(),
    };
    mqk_db::insert_strategy_signal_evaluation(&pool, &older_args)
        .await
        .expect("SO-05: first insert must succeed");

    // Duplicate write attempt with the SAME evaluation_id but a different
    // payload — must be silently ignored (ON CONFLICT DO NOTHING), proving
    // the original row's truth is never overwritten.
    let mut dup_args = older_args.clone();
    dup_args.bars_loaded = 999;
    mqk_db::insert_strategy_signal_evaluation(&pool, &dup_args)
        .await
        .expect("SO-05: duplicate insert must not error");

    let evaluation_id_2 = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.signal-evaluation.v1|test|so05|{symbol}|1Min|2").as_bytes(),
    );
    let newer_args = mqk_db::InsertStrategySignalEvaluationArgs {
        evaluation_id: evaluation_id_2,
        ts_utc: newer_ts,
        ..older_args.clone()
    };
    mqk_db::insert_strategy_signal_evaluation(&pool, &newer_args)
        .await
        .expect("SO-05: second insert must succeed");

    let rows: Vec<_> = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
        .await
        .expect("fetch_recent_strategy_signal_evaluations failed")
        .into_iter()
        .filter(|r| r.symbol == symbol)
        .collect();

    assert_eq!(
        rows.len(),
        2,
        "SO-05: exactly two rows — the duplicate write must not have created a third"
    );
    assert_eq!(
        rows[0].evaluation_id, evaluation_id_2,
        "SO-05: newest row (by ts_utc) must come first"
    );
    assert_eq!(
        rows[1].evaluation_id, evaluation_id,
        "SO-05: older row must come second"
    );
    assert_eq!(
        rows[1].bars_loaded, 5,
        "SO-05: the duplicate write's bars_loaded=999 must NOT have overwritten the original row"
    );
}

// ---------------------------------------------------------------------------
// SO-06 — route truth_state: active (seeded) and db_unavailable (no DB)
// ---------------------------------------------------------------------------

/// SO-06a: `GET /api/v1/execution/signal-evaluations` with no DB pool
/// configured at all returns `truth_state="db_unavailable"` and an empty,
/// honestly-non-authoritative row list. Needs no DB connection — always runs.
#[tokio::test]
async fn so06a_route_returns_db_unavailable_with_no_pool() {
    let st = AppState::new_for_test_with_broker_kind(BrokerKind::Paper);
    let router = build_router(Arc::new(st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/signal-evaluations")
        .body(Body::empty())
        .unwrap();
    let (status, body_bytes) = call(router, req).await;
    assert_eq!(status, 200, "SO-06a: route must return 200");

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "db_unavailable",
        "SO-06a: truth_state must be db_unavailable with no DB pool"
    );
    assert_eq!(
        body["rows"].as_array().unwrap().len(),
        0,
        "SO-06a: rows must be empty and not authoritative"
    );
}

/// SO-06b: With a DB pool and at least one seeded row, the route returns
/// `truth_state="active"` with that row's exact field truth, newest-first.
#[tokio::test]
async fn so06b_route_returns_active_with_seeded_rows() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SO-06b: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "SOBS06ROUTE";
    cleanup_signal_evaluations(&pool, symbol).await;

    let now = Utc::now();
    let evaluation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.signal-evaluation.v1|test|so06|{symbol}|1Min|1").as_bytes(),
    );
    mqk_db::insert_strategy_signal_evaluation(
        &pool,
        &mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id,
            ts_utc: now,
            run_id: None,
            strategy_id: "swing_momentum".to_string(),
            symbol: symbol.to_string(),
            timeframe: "1Min".to_string(),
            bar_context_source: "db_loaded".to_string(),
            bars_loaded: 3,
            latest_bar_ts_utc: Some(now),
            signal_generated: false,
            signal_qty: Some(0),
            signal_side: None,
            reason_code: "flat_below_threshold".to_string(),
            reason: "abs move_bps below threshold; insufficient displacement for signal"
                .to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "test".to_string(),
        },
    )
    .await
    .expect("SO-06b: seed insert must succeed");

    let st = AppState::new_with_db(pool.clone());
    let router = build_router(Arc::new(st));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/execution/signal-evaluations")
        .body(Body::empty())
        .unwrap();
    let (status, body_bytes) = call(router, req).await;
    assert_eq!(
        status,
        200,
        "SO-06b: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "active",
        "SO-06b: truth_state must be active with a seeded row present"
    );
    assert_eq!(
        body["backend"].as_str().unwrap(),
        "postgres.strategy_signal_evaluations",
        "SO-06b: backend must name the durable table"
    );

    let rows = body["rows"].as_array().expect("SO-06b: rows must be an array");
    let row = rows
        .iter()
        .find(|r| r["symbol"].as_str() == Some(symbol))
        .expect("SO-06b: seeded row must be present in the response");
    assert_eq!(
        row["evaluation_id"].as_str().unwrap(),
        evaluation_id.to_string(),
        "SO-06b: evaluation_id must round-trip"
    );
    assert_eq!(
        row["strategy_id"].as_str().unwrap(),
        "swing_momentum",
        "SO-06b: strategy_id must round-trip"
    );
    assert_eq!(
        row["bars_loaded"].as_i64().unwrap(),
        3,
        "SO-06b: bars_loaded must round-trip exactly"
    );
    assert!(
        !row["signal_generated"].as_bool().unwrap(),
        "SO-06b: signal_generated must round-trip"
    );
}
