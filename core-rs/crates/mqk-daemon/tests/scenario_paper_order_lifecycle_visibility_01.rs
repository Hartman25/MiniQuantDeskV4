//! PAPER-ORDER-LIFECYCLE-VIS-01C — GET /api/v1/execution/paper-lifecycle proof.
//!
//! Proves the route reconstructs one paper run's lifecycle chain (run ->
//! signal evaluation -> no-trade diagnostics -> outbox -> inbox) from
//! durable DB rows only, resolves the latest PAPER run when no `run_id` is
//! supplied, and never writes to any table.
//!
//! PL-01..PL-05 are in-process, always runnable without a DB connection.
//! PL-06..PL-13 are DB-backed and require MQK_DATABASE_URL; marked
//! `#[ignore]`, run with `--include-ignored --test-threads=1`.
//!
//! Known limitation (documented, not silently skipped): a DB-backed
//! "zero runs at all for engine=mqk-daemon/mode=PAPER" (`no_rows`) case is
//! not exercised here — the route's engine/mode filter is a fixed
//! constant shared with every other durable-run route in this crate, so
//! isolating "literally zero rows" in the shared `mqk_test` database
//! (populated by many sibling test files using the same engine_id) cannot
//! be done without a destructive truncate. The `not_found` test below
//! exercises the structurally identical "no row resolved" branch via an
//! explicit nonexistent `run_id` instead.
//!
//! No broker/provider/network call anywhere in this file. No order
//! submitted, cancelled, or replaced.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
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

fn make_router_no_db() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

// ---------------------------------------------------------------------------
// PL-01: No DB pool -> truth_state = "db_unavailable"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pl_01_no_db_returns_db_unavailable() {
    let router = make_router_no_db();
    let (status, body) = call(router, get("/api/v1/execution/paper-lifecycle")).await;

    assert_eq!(status, StatusCode::OK, "PL-01: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "db_unavailable",
        "PL-01: truth_state must be 'db_unavailable'; got: {json}"
    );
    assert!(
        json["run"].is_null(),
        "PL-01: run must be null when db_unavailable; got: {json}"
    );
    assert_eq!(
        json["signal_evaluations"],
        serde_json::json!([]),
        "PL-01: signal_evaluations must be empty; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-02: Route is mounted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pl_02_route_is_mounted_not_404() {
    let router = make_router_no_db();
    let (status, _body) = call(router, get("/api/v1/execution/paper-lifecycle")).await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "PL-02: /api/v1/execution/paper-lifecycle must be mounted"
    );
}

// ---------------------------------------------------------------------------
// PL-03: Invalid run_id -> 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pl_03_invalid_run_id_returns_400() {
    let router = make_router_no_db();
    let (status, body) = call(
        router,
        get("/api/v1/execution/paper-lifecycle?run_id=not-a-uuid"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "PL-03: non-UUID run_id must return 400; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "invalid_request",
        "PL-03: truth_state must be 'invalid_request'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-04: canonical_route is self-identifying
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pl_04_canonical_route_is_self_identifying() {
    let router = make_router_no_db();
    let (_status, body) = call(router, get("/api/v1/execution/paper-lifecycle")).await;
    let json = parse_json(body);
    assert_eq!(
        json["canonical_route"], "/api/v1/execution/paper-lifecycle",
        "PL-04: canonical_route mismatch; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-05: db_unavailable never fabricates a lifecycle_summary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pl_05_db_unavailable_has_no_lifecycle_summary() {
    let router = make_router_no_db();
    let (_status, body) = call(router, get("/api/v1/execution/paper-lifecycle")).await;
    let json = parse_json(body);
    assert!(
        json["lifecycle_summary"].is_null(),
        "PL-05: lifecycle_summary must be null when db_unavailable; got: {json}"
    );
}

// ===========================================================================
// DB-backed tests — PL-06..PL-13
//
// Run with:
//   MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test \
//   cargo test -p mqk-daemon --test scenario_paper_order_lifecycle_visibility_01 \
//     -- --include-ignored --test-threads=1
// ===========================================================================

async fn pl_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test \
             cargo test -p mqk-daemon --test scenario_paper_order_lifecycle_visibility_01 \
             -- --include-ignored --test-threads=1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("pl_db_pool: connect failed");
    mqk_db::migrate(&pool).await.expect("pl_db_pool: migrate failed");
    pool
}

/// Delete every row this suite could have written for `run_id`, FK-safe order.
async fn pl_cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
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
    let _ = sqlx::query("delete from strategy_signal_evaluations where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from autonomous_no_trade_diagnostics where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn pl_seed_run(pool: &sqlx::PgPool, run_id: Uuid, ts: chrono::DateTime<chrono::Utc>) {
    pl_cleanup_run(pool, run_id).await;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: ts,
            git_hash: "pl-test-hash".to_string(),
            config_hash: "pl-test-config".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "pl-test-host".to_string(),
        },
    )
    .await
    .expect("pl_seed_run: insert_run failed");
}

fn router_with_pool(pool: sqlx::PgPool) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

// ---------------------------------------------------------------------------
// PL-06: Explicit nonexistent run_id -> not_found (also stands in for the
// "no_rows" branch's structural sibling — see module doc).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_06_nonexistent_run_id_returns_not_found() {
    let pool = pl_db_pool().await;
    let never_used_run = Uuid::parse_str("f1600001-0000-4000-8000-000000000001").unwrap();

    let router = router_with_pool(pool);
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={never_used_run}");
    let (status, body) = call(router, get(&uri)).await;

    assert_eq!(status, StatusCode::OK, "PL-06: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "not_found",
        "PL-06: truth_state must be 'not_found'; got: {json}"
    );
    assert!(
        json["run"].is_null(),
        "PL-06: run must be null; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-07: No run_id -> resolves the latest durable PAPER run (far-future
// timestamp guarantees it sorts first regardless of other seeded rows).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_07_no_run_id_resolves_latest_paper_run() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000002").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2099-06-01T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    pl_seed_run(&pool, run_id, ts).await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(router, get("/api/v1/execution/paper-lifecycle")).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-07: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "active",
        "PL-07: truth_state must be 'active'; got: {json}"
    );
    assert_eq!(
        json["run_id"], run_id.to_string(),
        "PL-07: must resolve the far-future seeded run as latest; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-08: Signal-only lifecycle -> no_signal_durably_explained
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_08_signal_only_is_no_signal_durably_explained() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000003").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-01T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    pl_seed_run(&pool, run_id, ts).await;

    mqk_db::insert_strategy_signal_evaluation(
        &pool,
        &mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id: Uuid::parse_str("f1600001-0000-4000-8000-000000000103").unwrap(),
            ts_utc: ts,
            run_id: Some(run_id),
            strategy_id: "swing_momentum".to_string(),
            symbol: "AAPL".to_string(),
            timeframe: "5m".to_string(),
            bar_context_source: "md_bars".to_string(),
            bars_loaded: 20,
            latest_bar_ts_utc: Some(ts),
            signal_generated: false,
            signal_qty: None,
            signal_side: None,
            reason_code: "no_edge".to_string(),
            reason: "no edge detected".to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "pl_test".to_string(),
        },
    )
    .await
    .expect("PL-08: insert_strategy_signal_evaluation failed");

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let (status, body) = call(router, get(&uri)).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-08: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "no_signal_durably_explained",
        "PL-08: got: {json}"
    );
    assert_eq!(json["signal_truth_state"], "present");
    assert_eq!(json["outbox_truth_state"], "empty");
}

// ---------------------------------------------------------------------------
// PL-09: No-trade-diagnostic-only lifecycle -> no_signal_durably_explained
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_09_no_trade_diagnostic_only_is_no_signal_durably_explained() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000004").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-02T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    pl_seed_run(&pool, run_id, ts).await;

    mqk_db::insert_autonomous_no_trade_diagnostic(
        &pool,
        &mqk_db::InsertAutonomousNoTradeDiagnosticArgs {
            diagnostic_id: Uuid::parse_str("f1600001-0000-4000-8000-000000000204").unwrap(),
            observed_at_utc: ts,
            run_id: Some(run_id),
            mode: "PAPER".to_string(),
            session_window_state: "outside_window".to_string(),
            runtime_start_allowed: false,
            arm_state: "STOPPED".to_string(),
            overall_ready: false,
            reason_code: "outside_market_hours".to_string(),
            reason: "outside market hours".to_string(),
            stage: "readiness_check".to_string(),
            paper_order_attempted: false,
            live_order_attempted: false,
            source: "pl_test".to_string(),
        },
    )
    .await
    .expect("PL-09: insert_autonomous_no_trade_diagnostic failed");

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let (status, body) = call(router, get(&uri)).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-09: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "no_signal_durably_explained",
        "PL-09: got: {json}"
    );
    assert_eq!(json["no_trade_truth_state"], "present");
}

// ---------------------------------------------------------------------------
// PL-10: Outbox-only lifecycle -> order_submitted_fill_pending
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_10_outbox_only_is_order_submitted_fill_pending() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000005").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-03T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    pl_seed_run(&pool, run_id, ts).await;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "pl10-order-001",
        serde_json::json!({"symbol": "AAPL", "qty": 5, "side": "buy"}),
    )
    .await
    .expect("PL-10: outbox_enqueue failed");

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let (status, body) = call(router, get(&uri)).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-10: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_submitted_fill_pending",
        "PL-10: got: {json}"
    );
    assert_eq!(json["outbox_truth_state"], "present");
    assert_eq!(json["inbox_truth_state"], "empty");
    assert_eq!(json["lifecycle_summary"]["paper_order_attempted"], true);
}

// ---------------------------------------------------------------------------
// PL-11: Outbox + inbox fill -> order_filled_pnl_pending; portfolio/pnl
// truth_state is always the honest in-memory-only label.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_11_outbox_plus_inbox_fill_is_order_filled_pnl_pending() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000006").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-04T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    pl_seed_run(&pool, run_id, ts).await;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "pl11-order-001",
        serde_json::json!({"symbol": "AAPL", "qty": 5, "side": "buy"}),
    )
    .await
    .expect("PL-11: outbox_enqueue failed");

    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:pl11-broker-001:fill:2024-02-04T10:00:01Z",
        Some("pl11-fill-001"),
        "pl11-order-001",
        "pl11-broker-001",
        "fill",
        &serde_json::json!({
            "type": "fill",
            "internal_order_id": "pl11-order-001",
            "broker_order_id": "pl11-broker-001",
        }),
        0,
        ts,
    )
    .await
    .expect("PL-11: inbox_insert_deduped_with_identity failed");

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let (status, body) = call(router, get(&uri)).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-11: must return 200");
    let json = parse_json(body);
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_pnl_pending",
        "PL-11: got: {json}"
    );
    assert_eq!(json["inbox_truth_state"], "present");
    assert_eq!(json["lifecycle_summary"]["fill_seen"], true);
    // DURABLE-PAPER-PORTFOLIO-AND-PNL-01E: portfolio_truth_state/pnl_truth_state
    // are now real computed values read from the durable snapshot/accounting
    // tables (B4-B/B4-C/B4-D), not the prior hardcoded
    // "in_memory_only_not_restart_surviving" placeholder. This fixture seeds
    // an outbox+inbox fill but no durable Paper+Alpaca snapshot or
    // accounting row, so both must honestly report unavailable/not_found --
    // never fabricated as "active".
    assert_eq!(
        json["portfolio_truth_state"], "snapshot_unavailable",
        "PL-11: portfolio_truth_state must be honest, not fabricated 'active'; got: {json}"
    );
    assert_eq!(
        json["pnl_truth_state"], "not_found",
        "PL-11: pnl_truth_state must be honest, not fabricated 'active'; got: {json}"
    );

    let inbox_events = json["inbox_events"]
        .as_array()
        .expect("PL-11: inbox_events must be array");
    assert!(
        inbox_events
            .iter()
            .any(|r| r["internal_order_id"] == "pl11-order-001"),
        "PL-11: inbox_events must include the seeded fill row; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PL-12: Route performs zero writes to oms_outbox / oms_inbox / runs.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_12_route_writes_nothing() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000007").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-05T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    pl_seed_run(&pool, run_id, ts).await;

    let count_before: (i64, i64, i64) = (
        sqlx::query_scalar("select count(*) from oms_outbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*) from oms_inbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*) from runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
    );

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let _ = call(router.clone(), get(&uri)).await;
    let _ = call(router, get("/api/v1/execution/paper-lifecycle")).await;

    let count_after: (i64, i64, i64) = (
        sqlx::query_scalar("select count(*) from oms_outbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*) from oms_inbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*) from runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
    );

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(
        count_before, count_after,
        "PL-12: route calls must not change any row counts"
    );
}

// ---------------------------------------------------------------------------
// PL-13: Empty run (no signal/no-trade/outbox rows) -> partial_visibility,
// authoritative empty rather than a fabricated active-with-data claim.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn pl_13_empty_run_is_partial_visibility_not_fabricated() {
    let pool = pl_db_pool().await;
    let run_id = Uuid::parse_str("f1600001-0000-4000-8000-000000000008").unwrap();
    let ts = chrono::DateTime::parse_from_rfc3339("2024-02-06T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    pl_seed_run(&pool, run_id, ts).await;
    // Intentionally seed nothing else.

    let router = router_with_pool(pool.clone());
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let (status, body) = call(router, get(&uri)).await;

    pl_cleanup_run(&pool, run_id).await;

    assert_eq!(status, StatusCode::OK, "PL-13: must return 200");
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active", "PL-13: got: {json}");
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "partial_visibility",
        "PL-13: got: {json}"
    );
    assert_eq!(json["signal_truth_state"], "empty");
    assert_eq!(json["no_trade_truth_state"], "empty");
    assert_eq!(json["outbox_truth_state"], "empty");
    assert_eq!(json["inbox_truth_state"], "empty");
}
