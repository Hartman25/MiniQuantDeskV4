//! DURABLE-PAPER-PORTFOLIO-AND-PNL-01E: read-only API and paper-lifecycle
//! integration proof.
//!
//! Proves GET /api/v1/portfolio/durable-summary, durable-positions,
//! durable-snapshots, and the extended GET /api/v1/execution/paper-lifecycle
//! all read the durable tables B4-B/B4-C/B4-D added, never fabricate a
//! value, never mutate any row, and reject non-GET methods.
//!
//! In-process (no-DB) proofs run always; DB-backed proofs require
//! `MQK_DATABASE_URL` and are `#[ignore]`, run with `--include-ignored
//! --test-threads=1` (matching `scenario_paper_order_lifecycle_visibility_01.rs`'s
//! documented convention -- this file shares its DB fixtures with many
//! sibling test files via the same `mqk-daemon` engine_id, and some of this
//! file's own proofs check global-ish counts that are safest checked
//! single-threaded).
//!
//! No broker/provider/network call anywhere in this file. No order
//! submitted, cancelled, or replaced.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn get(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn post(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn make_router_no_db() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn router_with_pool(pool: sqlx::PgPool) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test \
             cargo test -p mqk-daemon --test scenario_durable_paper_portfolio_read_only_api_01 \
             -- --include-ignored --test-threads=1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");
    pool
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.durable-paper-portfolio-read-only-api.v1|{seed}").as_bytes(),
    )
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid, ts: chrono::DateTime<Utc>) {
    cleanup_run(pool, run_id).await;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: ts,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "delete from broker_order_map where internal_id in (select idempotency_key from oms_outbox where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_accounting_state where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "delete from sys_paper_portfolio_snapshot_positions where snapshot_id in (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn seed_snapshot(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    captured_at_utc: chrono::DateTime<Utc>,
    qty_signed: i64,
) -> Uuid {
    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk.paper-portfolio-snapshot.v1|{}|{}|{}",
            captured_at_utc.to_rfc3339(),
            run_id,
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        )
        .as_bytes(),
    );
    let mut positions = vec![];
    if qty_signed != 0 {
        positions.push(mqk_db::PaperPortfolioSnapshotPosition {
            symbol: "AAPL".to_string(),
            qty_signed,
            avg_entry_price_micros: 150_000_000,
            provenance: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
        });
    }
    mqk_db::insert_or_confirm_paper_portfolio_snapshot(
        pool,
        mqk_db::NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc,
            deployment_mode: "paper".to_string(),
            source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions,
        },
    )
    .await
    .expect("snapshot insert should succeed");
    snapshot_id
}

async fn seed_accounting(pool: &sqlx::PgPool, run_id: Uuid, epoch: &str, snapshot_id: Uuid) {
    mqk_db::upsert_paper_portfolio_accounting_state(
        pool,
        mqk_db::UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: -1_500_000_000,
            realized_pnl_micros: 40_000_000,
            fees_micros: 0,
            last_applied_inbox_id: 1,
            accounting_epoch: epoch.to_string(),
            accounting_epoch_reason: if epoch == "incomplete" {
                Some("test_reason".to_string())
            } else {
                None
            },
            updated_at_utc: Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("accounting upsert should succeed");
}

async fn seed_outbox_and_fill(pool: &sqlx::PgPool, run_id: Uuid, ts: chrono::DateTime<Utc>) {
    mqk_db::outbox_enqueue(
        pool,
        run_id,
        "order-1",
        serde_json::json!({"symbol": "AAPL", "qty": 10, "side": "buy"}),
    )
    .await
    .expect("outbox_enqueue should succeed");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        "msg-1",
        None,
        "order-1",
        "",
        "fill",
        &serde_json::json!({
            "type": "fill",
            "internal_order_id": "order-1",
            "broker_order_id": null,
        }),
        0,
        ts,
    )
    .await
    .expect("inbox insert should succeed");
}

// ---------------------------------------------------------------------------
// No DB -> db_unavailable for all three durable routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_db_returns_db_unavailable_for_all_durable_routes() {
    let router = make_router_no_db();

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/durable-summary")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse_json(body)["truth_state"], "db_unavailable");

    let (status, body) = call(router.clone(), get("/api/v1/portfolio/durable-positions")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse_json(body)["truth_state"], "db_unavailable");

    let (status, body) = call(router, get("/api/v1/portfolio/durable-snapshots")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse_json(body)["truth_state"], "db_unavailable");
}

/// None of the three durable routes accept a mutation method.
#[tokio::test]
async fn mutation_methods_are_rejected() {
    let router = make_router_no_db();
    for path in [
        "/api/v1/portfolio/durable-summary",
        "/api/v1/portfolio/durable-positions",
        "/api/v1/portfolio/durable-snapshots",
    ] {
        let (status, _) = call(router.clone(), post(path)).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must reject POST"
        );
    }
}

// ---------------------------------------------------------------------------
// durable-summary
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_no_snapshot_is_snapshot_unavailable() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_summary_no_snapshot_is_snapshot_unavailable");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "snapshot_unavailable");
    assert_eq!(json["accounting_truth_state"], "not_found");
    assert!(json["account_equity"].is_null());
    assert!(json["realized_pnl"].is_null());

    cleanup_run(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_with_complete_accounting_populates_realized_pnl() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_summary_with_complete_accounting_populates_realized_pnl");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    let snapshot_id = seed_snapshot(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        10,
    )
    .await;
    seed_accounting(&pool, run_id, "complete", snapshot_id).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["accounting_truth_state"], "active");
    assert_eq!(json["realized_pnl_truth_state"], "active");
    assert_eq!(json["realized_pnl"], 40.0);
    assert_eq!(json["account_equity"], 100000.0);
    assert_eq!(json["cash"], 40000.0);

    cleanup_run(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_incomplete_epoch_blocks_realized_pnl_but_shows_position() {
    let pool = test_pool().await;
    let run_id =
        fixed_run_id("durable_summary_incomplete_epoch_blocks_realized_pnl_but_shows_position");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    let snapshot_id = seed_snapshot(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        25,
    )
    .await;
    seed_accounting(&pool, run_id, "incomplete", snapshot_id).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["accounting_truth_state"], "fill_history_incomplete");
    assert_eq!(json["realized_pnl_truth_state"], "fill_history_incomplete");
    assert!(
        json["realized_pnl"].is_null(),
        "realized_pnl must be null (not fabricated), got: {json}"
    );
    assert!(json["accounting_epoch_reason"].as_str().is_some());
    // Position truth (from the durable snapshot) remains visible even though
    // realized P&L is blocked.
    assert_eq!(json["account_equity"], 100000.0);

    cleanup_run(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// Cross-run isolation (B4 closure repair, Repair A / Defect 1)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_never_returns_a_different_runs_snapshot() {
    let pool = test_pool().await;
    let run_a = fixed_run_id("durable_summary_never_returns_a_different_runs_snapshot.a");
    let run_b = fixed_run_id("durable_summary_never_returns_a_different_runs_snapshot.b");
    seed_run(
        &pool,
        run_a,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    seed_run(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 30, 0).unwrap(),
    )
    .await;

    seed_snapshot(
        &pool,
        run_a,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        10,
    )
    .await;
    // run_b's snapshot is strictly NEWER than run_a's.
    seed_snapshot(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 14, 0, 0).unwrap(),
        99,
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/portfolio/durable-summary?run_id={run_a}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["run_id"],
        run_a.to_string(),
        "response run_id must be run_a, never run_b"
    );
    assert_eq!(
        json["truth_state"], "active",
        "run_a's own snapshot must satisfy the query"
    );

    cleanup_run(&pool, run_a).await;
    cleanup_run(&pool, run_b).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_positions_never_returns_a_different_runs_snapshot() {
    let pool = test_pool().await;
    let run_a = fixed_run_id("durable_positions_never_returns_a_different_runs_snapshot.a");
    let run_b = fixed_run_id("durable_positions_never_returns_a_different_runs_snapshot.b");
    seed_run(
        &pool,
        run_a,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    seed_run(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 30, 0).unwrap(),
    )
    .await;

    seed_snapshot(
        &pool,
        run_a,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        10,
    )
    .await;
    seed_snapshot(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 14, 0, 0).unwrap(),
        99,
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-positions?run_id={run_a}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["run_id"], run_a.to_string());
    let positions = json["positions"].as_array().expect("positions array");
    assert_eq!(positions.len(), 1);
    assert_eq!(
        positions[0]["qty_signed"], 10,
        "must be run_a's qty (10), never run_b's (99)"
    );

    cleanup_run(&pool, run_a).await;
    cleanup_run(&pool, run_b).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn paper_lifecycle_never_reports_a_different_runs_portfolio_truth() {
    let pool = test_pool().await;
    let run_a = fixed_run_id("paper_lifecycle_never_reports_a_different_runs_portfolio_truth.a");
    let run_b = fixed_run_id("paper_lifecycle_never_reports_a_different_runs_portfolio_truth.b");
    seed_run(
        &pool,
        run_a,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    seed_run(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 30, 0).unwrap(),
    )
    .await;

    // Only run_b has a durable snapshot; run_a has none.
    seed_snapshot(
        &pool,
        run_b,
        Utc.with_ymd_and_hms(2099, 5, 1, 14, 0, 0).unwrap(),
        99,
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/execution/paper-lifecycle?run_id={run_a}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["run_id"], run_a.to_string());
    assert_eq!(
        json["portfolio_truth_state"], "snapshot_unavailable",
        "run_a has no snapshot of its own -- run_b's snapshot must never be attributed to run_a"
    );

    cleanup_run(&pool, run_a).await;
    cleanup_run(&pool, run_b).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_non_paper_run_fails_closed() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_summary_non_paper_run_fails_closed");
    cleanup_run(&pool, run_id).await;
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "BACKTEST".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");
    seed_snapshot(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        10,
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "unsupported_source",
        "a resolved non-PAPER run must never surface active Paper+Alpaca portfolio truth"
    );

    cleanup_run(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_unknown_explicit_run_id_is_not_found() {
    let pool = test_pool().await;
    let unknown_run_id =
        fixed_run_id("durable_summary_unknown_explicit_run_id_is_not_found.never_inserted");

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={unknown_run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "not_found");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_invalid_run_id_has_fixed_bounded_message() {
    let pool = test_pool().await;
    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get("/api/v1/portfolio/durable-summary?run_id=not-a-uuid-at-all"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    assert_eq!(
        json["detail"], "run_id query parameter is not a valid UUID",
        "the raw invalid input must never be echoed back onto the wire"
    );
}

// ---------------------------------------------------------------------------
// durable-positions
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_positions_returns_positions_from_latest_snapshot() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_positions_returns_positions_from_latest_snapshot");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    seed_snapshot(&pool, run_id, Utc::now(), 10).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-positions?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    let positions = json["positions"].as_array().expect("positions array");
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["symbol"], "AAPL");
    assert_eq!(positions[0]["qty_signed"], 10);

    cleanup_run(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_positions_flags_stale_snapshot() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_positions_flags_stale_snapshot");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    // Captured far enough in the past to exceed the 180s staleness threshold.
    let stale_captured_at = Utc::now() - chrono::Duration::seconds(600);
    seed_snapshot(&pool, run_id, stale_captured_at, 10).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-positions?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "snapshot_stale");

    cleanup_run(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// durable-snapshots
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_snapshots_list_respects_limit_and_orders_newest_first() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_snapshots_list_respects_limit_and_orders_newest_first");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;
    let mut ids = vec![];
    for i in 0..3i64 {
        let ts = Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap() + chrono::Duration::minutes(i);
        ids.push(seed_snapshot(&pool, run_id, ts, 10 + i).await);
    }

    let router = router_with_pool(pool.clone());
    let (status, body) = call(router, get("/api/v1/portfolio/durable-snapshots?limit=2")).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    let rows = json["snapshots"].as_array().expect("snapshots array");
    assert_eq!(rows.len(), 2, "limit=2 must cap the result to 2 rows");
    let first_id = rows[0]["snapshot_id"].as_str().unwrap();
    assert_eq!(
        first_id,
        ids[2].to_string(),
        "newest snapshot must be first"
    );

    cleanup_run(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// paper-lifecycle integration
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn paper_lifecycle_reports_durable_pnl_available_once_accounting_is_complete() {
    let pool = test_pool().await;
    let run_id =
        fixed_run_id("paper_lifecycle_reports_durable_pnl_available_once_accounting_is_complete");
    let ts = Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap();
    seed_run(&pool, run_id, ts).await;
    seed_outbox_and_fill(&pool, run_id, ts).await;
    let snapshot_id = seed_snapshot(&pool, run_id, ts, 10).await;
    seed_accounting(&pool, run_id, "complete", snapshot_id).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/execution/paper-lifecycle?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["portfolio_truth_state"], "active");
    assert_eq!(json["pnl_truth_state"], "active");
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_portfolio_durable_pnl_available"
    );

    cleanup_run(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn paper_lifecycle_reports_durable_pnl_incomplete() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("paper_lifecycle_reports_durable_pnl_incomplete");
    let ts = Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap();
    seed_run(&pool, run_id, ts).await;
    seed_outbox_and_fill(&pool, run_id, ts).await;
    let snapshot_id = seed_snapshot(&pool, run_id, ts, 25).await;
    seed_accounting(&pool, run_id, "incomplete", snapshot_id).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/execution/paper-lifecycle?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["portfolio_truth_state"], "active");
    assert_eq!(json["pnl_truth_state"], "fill_history_incomplete");
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_portfolio_durable_pnl_incomplete"
    );

    cleanup_run(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// B4 final closure repair (Phase B): shared provenance classifier proof.
// ---------------------------------------------------------------------------

/// Accounting tied to an OLDER snapshot must never be reported active
/// beside a strictly NEWER snapshot -- the exact defect this repair closes
/// (`accounting_fields` never compared `source_snapshot_id` against the
/// currently-selected snapshot at all).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn durable_summary_stale_accounting_snapshot_cannot_appear_active() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("durable_summary_stale_accounting_snapshot_cannot_appear_active");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap(),
    )
    .await;

    let snapshot_old = seed_snapshot(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 13, 0, 0).unwrap(),
        10,
    )
    .await;
    seed_accounting(&pool, run_id, "complete", snapshot_old).await;

    // A strictly newer snapshot arrives, but no new accounting refresh is
    // recorded for it -- the accounting row still points at snapshot_old.
    seed_snapshot(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 5, 1, 14, 0, 0).unwrap(),
        20,
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["accounting_truth_state"], "accounting_snapshot_mismatch",
        "stale accounting provenance must never be reported active beside a newer snapshot: {json}"
    );
    assert_eq!(
        json["realized_pnl_truth_state"],
        "accounting_snapshot_mismatch"
    );
    assert!(
        json["realized_pnl"].is_null(),
        "realized_pnl must be null on a snapshot mismatch, got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .expect("blockers array")
            .iter()
            .any(|b| b.as_str().unwrap_or_default().contains("does not match")),
        "an explicit bounded blocker must be present: {json}"
    );

    cleanup_run(&pool, run_id).await;
}

/// The paper-lifecycle route's invalid-run_id handling uses the exact same
/// fixed, bounded message as durable-summary and never echoes the raw
/// supplied value -- proving the B4 closure repair fix (the prior
/// implementation formatted `"run_id is not a valid UUID: {s}"`, echoing
/// the caller-supplied string).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn paper_lifecycle_invalid_run_id_has_fixed_bounded_message() {
    let pool = test_pool().await;
    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get("/api/v1/execution/paper-lifecycle?run_id=not-a-uuid-at-all"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    let blockers = json["blockers"].as_array().expect("blockers array");
    assert_eq!(
        blockers,
        &vec![serde_json::Value::String(
            "run_id query parameter is not a valid UUID".to_string()
        )],
        "the raw invalid input must never be echoed back onto the wire: {json}"
    );
    for blocker in blockers {
        assert!(
            !blocker
                .as_str()
                .unwrap_or_default()
                .contains("not-a-uuid-at-all"),
            "blocker must never contain the raw supplied value: {json}"
        );
    }
}

/// Paper-lifecycle's `pnl_truth_state` and `overall_lifecycle_state` must
/// agree with durable-summary's `accounting_truth_state` for the identical
/// stale-snapshot scenario -- the same shared classifier, never two
/// independently drifting ones.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn summary_and_lifecycle_agree_on_snapshot_mismatch() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("summary_and_lifecycle_agree_on_snapshot_mismatch");
    let ts = Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap();
    seed_run(&pool, run_id, ts).await;
    seed_outbox_and_fill(&pool, run_id, ts).await;

    let snapshot_old = seed_snapshot(&pool, run_id, ts, 10).await;
    seed_accounting(&pool, run_id, "complete", snapshot_old).await;
    seed_snapshot(&pool, run_id, ts + chrono::Duration::hours(1), 20).await;

    let router = router_with_pool(pool.clone());

    let (status, body) = call(
        router.clone(),
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary_json = parse_json(body);
    assert_eq!(
        summary_json["accounting_truth_state"],
        "accounting_snapshot_mismatch"
    );

    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/execution/paper-lifecycle?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let lifecycle_json = parse_json(body);
    assert_eq!(
        lifecycle_json["pnl_truth_state"], "accounting_snapshot_mismatch",
        "paper-lifecycle must use the identical classification as durable-summary: {lifecycle_json}"
    );
    assert_ne!(
        lifecycle_json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_portfolio_durable_pnl_available",
        "a stale-snapshot mismatch must never be reported as durable P&L available: {lifecycle_json}"
    );

    cleanup_run(&pool, run_id).await;
}

/// A DB query failure (unreachable pool) is reported as `query_failed` on
/// both durable-summary and paper-lifecycle, never silently collapsed into
/// `not_found`.
#[tokio::test]
async fn query_failed_is_distinct_from_not_found_on_both_routes() {
    // A lazily-connecting pool against an unreachable address: connect()
    // never blocks (lazy), but any query against it fails -- self-contained,
    // never touches the real isolated test pool.
    let unreachable_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/mqk_test?sslmode=disable")
        .expect("lazy connect should not fail synchronously");
    let run_id = fixed_run_id("query_failed_is_distinct_from_not_found_on_both_routes");

    let router = router_with_pool(unreachable_pool);

    let (status, body) = call(
        router.clone(),
        get(&format!(
            "/api/v1/portfolio/durable-summary?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary_json = parse_json(body);
    assert_eq!(summary_json["truth_state"], "query_failed");
    assert_ne!(summary_json["truth_state"], "not_found");

    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/execution/paper-lifecycle?run_id={run_id}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let lifecycle_json = parse_json(body);
    assert_eq!(lifecycle_json["truth_state"], "query_failed");
    assert_ne!(lifecycle_json["truth_state"], "not_found");
}

// ---------------------------------------------------------------------------
// Read-only proof
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn repeated_gets_across_all_durable_routes_never_mutate_any_row() {
    let pool = test_pool().await;
    let run_id = fixed_run_id("repeated_gets_across_all_durable_routes_never_mutate_any_row");
    let ts = Utc.with_ymd_and_hms(2099, 5, 1, 12, 0, 0).unwrap();
    seed_run(&pool, run_id, ts).await;
    seed_outbox_and_fill(&pool, run_id, ts).await;
    let snapshot_id = seed_snapshot(&pool, run_id, ts, 10).await;
    seed_accounting(&pool, run_id, "complete", snapshot_id).await;

    async fn counts(pool: &sqlx::PgPool, run_id: Uuid) -> (i64, i64, i64, i64, i64, i64) {
        let snapshots: i64 = sqlx::query_scalar(
            "select count(*) from sys_paper_portfolio_snapshots where run_id = $1",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let positions: i64 = sqlx::query_scalar(
            "select count(*) from sys_paper_portfolio_snapshot_positions where snapshot_id in (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let accounting: i64 = sqlx::query_scalar(
            "select count(*) from sys_paper_portfolio_accounting_state where run_id = $1",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let outbox: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let inbox: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let runs: i64 = sqlx::query_scalar("select count(*) from runs where run_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
        (snapshots, positions, accounting, outbox, inbox, runs)
    }

    let before = counts(&pool, run_id).await;

    let router = router_with_pool(pool.clone());
    for _ in 0..3 {
        let _ = call(
            router.clone(),
            get(&format!(
                "/api/v1/portfolio/durable-summary?run_id={run_id}"
            )),
        )
        .await;
        let _ = call(
            router.clone(),
            get(&format!(
                "/api/v1/portfolio/durable-positions?run_id={run_id}"
            )),
        )
        .await;
        let _ = call(
            router.clone(),
            get("/api/v1/portfolio/durable-snapshots?limit=5"),
        )
        .await;
        let _ = call(
            router.clone(),
            get(&format!(
                "/api/v1/execution/paper-lifecycle?run_id={run_id}"
            )),
        )
        .await;
    }

    let after = counts(&pool, run_id).await;
    assert_eq!(
        before, after,
        "repeated GETs across every durable route must produce zero row-count delta"
    );

    cleanup_run(&pool, run_id).await;
}
