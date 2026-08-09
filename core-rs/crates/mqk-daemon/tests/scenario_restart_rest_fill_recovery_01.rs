//! BRK-GAP-REST-RECOVERY-01 — WS gap fill recovery service proof.
//!
//! Proves `POST /api/v1/ops/repair/ws-gap-fill-recovery` is fail-closed,
//! idempotent, and correctly matches REST account activities to known broker
//! orders for a run.
//!
//! ## Invariants under test
//!
//! | Test | Claim                                                                    | DB? |
//! |------|--------------------------------------------------------------------------|-----|
//! | G01  | No DB → 503 `no_db` fail-closed                                         | No  |
//! | G02  | Gate ordering: without DB, invalid run_id still returns 503 no_db       | No  |
//! | G03  | No fetcher configured → 503 `fetcher_unavailable` fail-closed           | Yes |
//! | G04  | Fetcher returns REST error → 503 `rest_unavailable` fail-closed         | Yes |
//! | G05  | Valid fill for known broker order inserts one inbox row                  | Yes |
//! | G06  | Second recovery is idempotent (already_present=true, no duplicate)      | Yes |
//! | G07  | Unknown broker order → skipped, unknown_order_count=1                   | Yes |
//! | G08  | Malformed activity (missing price) → skipped, malformed_count=1         | Yes |
//! | G09  | dry_run=true plans without mutating inbox                               | Yes |
//! | G10  | Existing manual repair workflow still passes (regression guard)         | No  |
//!
//! G01, G02, G10 are pure in-process (no DB required).
//! G03–G09 require `MQK_DATABASE_URL` and skip gracefully without it.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaOrderActivity;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fake WsGapFillFetcher — no real network calls
// ---------------------------------------------------------------------------

struct FakeGapFetcher {
    activities: Vec<AlpacaOrderActivity>,
    error: Option<String>,
}

impl FakeGapFetcher {
    fn returning(activities: Vec<AlpacaOrderActivity>) -> Arc<Self> {
        Arc::new(Self {
            activities,
            error: None,
        })
    }

    fn failing(msg: &str) -> Arc<Self> {
        Arc::new(Self {
            activities: vec![],
            error: Some(msg.to_owned()),
        })
    }
}

impl state::WsGapFillFetcher for FakeGapFetcher {
    fn fetch_fills_since(
        &self,
        _since_activity_id: Option<&str>,
    ) -> Result<Vec<AlpacaOrderActivity>, String> {
        if let Some(ref e) = self.error {
            return Err(e.clone());
        }
        Ok(self.activities.clone())
    }
}

// ---------------------------------------------------------------------------
// Activity constructors
// ---------------------------------------------------------------------------

fn make_fill_activity(
    activity_id: &str,
    order_id: &str,
    symbol: &str,
    side: &str,
) -> AlpacaOrderActivity {
    AlpacaOrderActivity {
        id: activity_id.to_owned(),
        activity_type: "FILL".to_owned(),
        order_id: order_id.to_owned(),
        transaction_time: "2026-05-09T14:30:00.000000000Z".to_owned(),
        price: Some("150.00".to_owned()),
        qty: Some("5".to_owned()),
        side: side.to_owned(),
        symbol: symbol.to_owned(),
        cum_qty: None,
    }
}

fn make_fill_no_price(activity_id: &str, order_id: &str) -> AlpacaOrderActivity {
    let mut a = make_fill_activity(activity_id, order_id, "AAPL", "buy");
    a.price = None;
    a
}

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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("response body is not valid JSON")
}

fn gap_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/ws-gap-fill-recovery")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("serialize body"),
        ))
        .unwrap()
}

fn no_db_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

async fn db_state_with_fetcher(
    fetcher: Arc<dyn state::WsGapFillFetcher>,
) -> Option<Arc<state::AppState>> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    );
    st.set_ws_gap_fill_fetcher_for_test(fetcher);
    Some(Arc::new(st))
}

async fn db_state_no_fetcher() -> Option<(Arc<state::AppState>, sqlx::PgPool)> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    );
    // BRK-GAP-REST-RECOVERY-01: explicitly clear the production-wired fetcher so the
    // "no fetcher configured" gate tests remain valid regardless of env credentials.
    st.ws_gap_fill_fetcher = None;
    Some((Arc::new(st), pool))
}

// Fixed identifiers for DB-backed tests (UUIDs use only hex digits).
const TEST_RUN_ID: &str = "a0050001-0000-0000-0000-000000000001";
const TEST_BROKER_ORDER_ID: &str = "broker-gap-order-001";
const TEST_INTERNAL_ORDER_ID: &str = "internal-gap-order-001";
// Alpaca activity IDs use the format "YYYYMMDDHHMMSS{fraction}::{uuid}" — using
// a deterministic hex-only stub here.
const TEST_ACTIVITY_ID: &str = "20260509143000000000001::a0b0c0d0e0f0a0b0";

/// Insert a minimal RUNNING run + outbox row + broker_order_map entry.
async fn setup_run_with_order(pool: &sqlx::PgPool) {
    use chrono::Utc;
    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let now = Utc::now();

    sqlx::query(
        "insert into runs (run_id, engine_id, mode, started_at_utc,
                            git_hash, config_hash, config_json, host_fingerprint)
         values ($1, 'test-engine', 'PAPER', $2, 'test-hash',
                 'test-cfg-hash', '{\"src\":\"brk-gap-rest-recovery-01\"}'::jsonb,
                 'test-host')
         on conflict (run_id) do nothing",
    )
    .bind(run_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("insert run");

    let order_json = serde_json::json!({
        "order_id": TEST_INTERNAL_ORDER_ID,
        "symbol": "AAPL",
        "side": "buy",
        "quantity": 5,
        "order_type": "limit",
        "limit_price": 150_000_000_i64,
    });
    sqlx::query(
        "insert into oms_outbox
             (run_id, idempotency_key, order_json, status, created_at_utc)
         values ($1, $2, $3, 'SENT', $4)
         on conflict (idempotency_key) do nothing",
    )
    .bind(run_id)
    .bind(TEST_INTERNAL_ORDER_ID)
    .bind(&order_json)
    .bind(now)
    .execute(pool)
    .await
    .expect("insert outbox");

    sqlx::query(
        "insert into broker_order_map (internal_id, broker_id, registered_at_utc)
         values ($1, $2, $3)
         on conflict (internal_id) do nothing",
    )
    .bind(TEST_INTERNAL_ORDER_ID)
    .bind(TEST_BROKER_ORDER_ID)
    .bind(now)
    .execute(pool)
    .await
    .expect("insert broker_order_map");
}

/// Clean up all test rows for this scenario.
async fn cleanup_run(pool: &sqlx::PgPool) {
    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from broker_order_map where internal_id = $1")
        .bind(TEST_INTERNAL_ORDER_ID)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from oms_outbox where idempotency_key = $1")
        .bind(TEST_INTERNAL_ORDER_ID)
        .execute(pool)
        .await
        .ok();
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// G01 — No DB → 503 no_db
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g01_no_db_fails_closed() {
    let st = no_db_state();
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({
            "run_id": TEST_RUN_ID,
            "dry_run": true,
        })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G01: expect 503; body={j}"
    );
    assert_eq!(j["truth_state"], "no_db", "G01: truth_state must be no_db");
    assert_eq!(
        j["gate"], "repair.db_required",
        "G01: gate must be db_required"
    );
}

// ---------------------------------------------------------------------------
// G02 — Gate ordering: without DB, invalid run_id still gets 503 (DB gate first)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g02_gate_ordering_db_before_run_id_parse() {
    let st = no_db_state();
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({
            "run_id": "not-a-uuid",
            "dry_run": true,
        })),
    )
    .await;
    let j = parse_json(body);

    // Gate 1 (DB) fires before Gate 2 (run_id parse).
    // Without DB: 503 no_db even when run_id is invalid.
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G02: DB gate fires before run_id parse; body={j}"
    );
    assert_eq!(j["truth_state"], "no_db", "G02: DB gate fires first");
}

// ---------------------------------------------------------------------------
// G03 — No fetcher configured → 503 fetcher_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g03_no_fetcher_fails_closed() {
    let Some((st, _pool)) = db_state_no_fetcher().await else {
        eprintln!("G03: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({
            "run_id": TEST_RUN_ID,
            "dry_run": true,
        })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G03: expect 503; body={j}"
    );
    assert_eq!(
        j["gate"], "repair.fetcher_unavailable",
        "G03: must be fetcher_unavailable"
    );
    assert_eq!(
        j["fetcher_available"], false,
        "G03: fetcher_available must be false"
    );
}

// ---------------------------------------------------------------------------
// G04 — Fetcher REST error → 503 rest_unavailable fail-closed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g04_rest_unavailable_fails_closed() {
    let fetcher = FakeGapFetcher::failing("simulated REST failure");
    let Some(st) = db_state_with_fetcher(fetcher).await else {
        eprintln!("G04: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({
            "run_id": TEST_RUN_ID,
            "dry_run": true,
        })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G04: expect 503; body={j}"
    );
    assert_eq!(
        j["gate"], "repair.rest_unavailable",
        "G04: gate must be rest_unavailable"
    );
    assert_eq!(j["recovered_count"], 0, "G04: no recovery on REST error");
}

// ---------------------------------------------------------------------------
// G05 — Matching fill inserts one inbox row (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g05_matching_fill_inserts_inbox_row() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some(st) = db_state_with_fetcher(fetcher).await else {
        eprintln!("G05: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G05: expect 200; body={j}");
    assert_eq!(
        j["truth_state"], "active",
        "G05: truth_state must be active"
    );
    assert_eq!(j["recovered_count"], 1, "G05: expect 1 recovered");
    assert_eq!(
        j["already_present_count"], 0,
        "G05: expect 0 already present"
    );
    assert_eq!(j["unknown_order_count"], 0, "G05: expect 0 unknown");
    assert_eq!(j["malformed_count"], 0, "G05: expect 0 malformed");
    assert_eq!(j["dry_run"], false, "G05: dry_run false");

    let fills = j["recovered_fills"]
        .as_array()
        .expect("recovered_fills is array");
    assert_eq!(fills.len(), 1, "G05: one fill in recovered_fills");
    let expected_msg_id = format!("alpaca-rest-gap:{TEST_ACTIVITY_ID}");
    assert_eq!(
        fills[0]["inbox_broker_message_id"], expected_msg_id,
        "G05: stable broker_message_id"
    );
    assert_eq!(
        fills[0]["already_present"], false,
        "G05: not already present"
    );

    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 1, "G05: one unapplied inbox row in DB");
    assert_eq!(
        rows[0].broker_message_id, expected_msg_id,
        "G05: inbox broker_message_id matches"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// G06 — Second recovery is idempotent (no duplicate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g06_second_recovery_is_idempotent() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");

    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("G06: skipped — MQK_DATABASE_URL not set");
            return;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");

    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    // First run — inserts the row.
    let fetcher1 = FakeGapFetcher::returning(vec![activity.clone()]);
    let st1 = {
        let mut s = state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        );
        s.set_ws_gap_fill_fetcher_for_test(fetcher1);
        Arc::new(s)
    };
    let (s1, b1) = call(
        routes::build_router(st1),
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j1 = parse_json(b1);
    assert_eq!(s1, StatusCode::OK, "G06: first run must be 200; body={j1}");
    assert_eq!(j1["recovered_count"], 1, "G06: first run inserts one row");

    // Second run — same activity, must be idempotent.
    let fetcher2 = FakeGapFetcher::returning(vec![activity]);
    let st2 = {
        let mut s = state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        );
        s.set_ws_gap_fill_fetcher_for_test(fetcher2);
        Arc::new(s)
    };
    let (s2, b2) = call(
        routes::build_router(st2),
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j2 = parse_json(b2);
    assert_eq!(s2, StatusCode::OK, "G06: second run must be 200");
    assert_eq!(j2["recovered_count"], 0, "G06: no new rows on second run");
    assert_eq!(j2["already_present_count"], 1, "G06: already_present=1");

    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 1, "G06: exactly one inbox row (idempotent)");

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// G07 — Unknown broker order → skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g07_unknown_broker_order_skipped() {
    let activity = make_fill_activity(
        "20260509143000000000002::a1b1c1d1e1f1a1b1",
        "broker-order-NOT-IN-MAP",
        "AAPL",
        "buy",
    );
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some(st) = db_state_with_fetcher(fetcher).await else {
        eprintln!("G07: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G07: expect 200; body={j}");
    assert_eq!(
        j["recovered_count"], 0,
        "G07: no recovery for unknown order"
    );
    assert_eq!(j["unknown_order_count"], 1, "G07: unknown_order_count=1");

    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 0, "G07: no inbox rows for unknown order");

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// G08 — Malformed activity (missing price) → skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g08_malformed_activity_skipped() {
    let activity = make_fill_no_price(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID);
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some(st) = db_state_with_fetcher(fetcher).await else {
        eprintln!("G08: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G08: expect 200; body={j}");
    assert_eq!(j["recovered_count"], 0, "G08: no recovery for malformed");
    assert_eq!(j["malformed_count"], 1, "G08: malformed_count=1");

    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 0, "G08: no inbox row for malformed activity");

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// G09 — dry_run=true plans without mutating inbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g09_dry_run_plans_without_mutation() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some(st) = db_state_with_fetcher(fetcher).await else {
        eprintln!("G09: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": true })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "G09: expect 200; body={j}");
    assert_eq!(j["dry_run"], true, "G09: dry_run must be true");
    assert_eq!(j["recovered_count"], 1, "G09: plan shows 1 recovery");
    assert_eq!(
        j["recovered_fills"][0]["already_present"], false,
        "G09: fill not already present in plan"
    );

    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 0, "G09: dry_run must not mutate inbox");

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// G10 — Existing manual repair workflow regression guard (pure in-process)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g10_existing_manual_repair_route_unaffected() {
    let st = no_db_state();
    let router = routes::build_router(st);

    // The existing halted-run-fill-rest-recovery route must still be mounted and
    // return 503 no_db — proving it was not displaced or renamed by the new route.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/halted-run-fill-rest-recovery")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "run_id": TEST_RUN_ID,
                "internal_order_id": "test-order",
                "broker_order_id": "broker-order",
                "dry_run": true,
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, body) = call(router, req).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G10: halted-run repair route must still return 503 no_db; body={j}"
    );
    assert_eq!(
        j["truth_state"], "no_db",
        "G10: halted-run REST recovery route must be untouched"
    );
}

// ---------------------------------------------------------------------------
// BRK-GAP-REST-CURSOR-ADVANCE-01 — cursor advancement tests (C01–C09)
//
// All C tests require MQK_DATABASE_URL and skip gracefully without it.
//
// Cursor JSON format: {"schema_version":1,"rest_activity_after":"<id>","trade_updates":{"status":"cold_start_unproven"}}
// Activity IDs are chronological: lexicographic order == chronological order.
// ---------------------------------------------------------------------------

const CURSOR_ANCHOR_OLD: &str = "20260508000000000000001::old-anchor-001000";
const CURSOR_ANCHOR_NEW: &str = "20260510000000000000001::new-anchor-001000";

fn cursor_json_with_anchor(anchor: &str) -> String {
    format!(
        r#"{{"schema_version":1,"rest_activity_after":"{}","trade_updates":{{"status":"cold_start_unproven"}}}}"#,
        anchor
    )
}

async fn seed_cursor(pool: &sqlx::PgPool, adapter_id: &str, cursor_json: &str) {
    mqk_db::advance_broker_cursor(pool, adapter_id, cursor_json, chrono::Utc::now())
        .await
        .expect("seed_cursor failed");
}

async fn load_cursor_json(pool: &sqlx::PgPool, adapter_id: &str) -> Option<String> {
    mqk_db::load_broker_cursor(pool, adapter_id)
        .await
        .expect("load_cursor_json failed")
}

async fn delete_cursor(pool: &sqlx::PgPool, adapter_id: &str) {
    sqlx::query("delete from broker_event_cursor where adapter_id = $1")
        .bind(adapter_id)
        .execute(pool)
        .await
        .ok();
}

async fn cursor_rest_anchor(pool: &sqlx::PgPool, adapter_id: &str) -> Option<String> {
    let json = load_cursor_json(pool, adapter_id).await?;
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| {
            v.get("rest_activity_after")
                .and_then(|a| a.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
}

/// Build a DB-backed AppState and return (Arc<AppState>, pool).
async fn cursor_test_state(
    fetcher: std::sync::Arc<dyn state::WsGapFillFetcher>,
) -> Option<(std::sync::Arc<state::AppState>, sqlx::PgPool)> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    );
    st.set_ws_gap_fill_fetcher_for_test(fetcher);
    Some((std::sync::Arc::new(st), pool))
}

// ---------------------------------------------------------------------------
// C01 — dry_run=true does not advance rest_activity_after
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c01_dry_run_does_not_advance_cursor() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C01: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": true })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C01: expect 200; body={j}");
    assert_eq!(j["dry_run"], true, "C01: dry_run must be true");
    assert_eq!(
        j["cursor_advanced"], false,
        "C01: cursor must not advance on dry_run"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C01: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C01: cursor must remain unchanged"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C02 — successful confirmed recovery advances rest_activity_after
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c02_successful_recovery_advances_cursor() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C02: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    // Seed cursor with old anchor — recovery activity ID is newer.
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C02: expect 200; body={j}");
    assert_eq!(j["recovered_count"], 1, "C02: one fill recovered");
    assert_eq!(j["cursor_advanced"], true, "C02: cursor must advance");
    assert_eq!(
        j["new_rest_activity_after"], TEST_ACTIVITY_ID,
        "C02: new anchor must be activity ID"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(TEST_ACTIVITY_ID),
        "C02: DB cursor must reflect advanced anchor"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C03 — second run idempotent: cursor does not move backward
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c03_second_run_cursor_does_not_move_backward() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("C03: skipped — MQK_DATABASE_URL not set");
            return;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");

    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;

    // First run: cursor is seeded with old anchor, recovery advances to TEST_ACTIVITY_ID.
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;
    let f1 = FakeGapFetcher::returning(vec![activity.clone()]);
    let st1 = {
        let mut s = state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        );
        s.set_ws_gap_fill_fetcher_for_test(f1);
        std::sync::Arc::new(s)
    };
    let (s1, _b1) = call(
        routes::build_router(st1),
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    assert_eq!(s1, StatusCode::OK, "C03: first run must succeed");
    let anchor_after_first = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor_after_first.as_deref(),
        Some(TEST_ACTIVITY_ID),
        "C03: cursor advanced to TEST_ACTIVITY_ID after first run"
    );

    // Second run: same activity (already_present), cursor must not move backward.
    let f2 = FakeGapFetcher::returning(vec![activity]);
    let st2 = {
        let mut s = state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        );
        s.set_ws_gap_fill_fetcher_for_test(f2);
        std::sync::Arc::new(s)
    };
    let (s2, b2) = call(
        routes::build_router(st2),
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j2 = parse_json(b2);
    assert_eq!(s2, StatusCode::OK, "C03: second run must succeed");
    // Activity is already present — cursor stays at TEST_ACTIVITY_ID (same value, not backward).
    // advance_broker_cursor_rest_anchor returns false when new_anchor == existing.
    assert_eq!(
        j2["already_present_count"], 1,
        "C03: second run sees already_present"
    );
    let anchor_after_second = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor_after_second.as_deref(),
        Some(TEST_ACTIVITY_ID),
        "C03: cursor must remain at TEST_ACTIVITY_ID (no backward movement)"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C04 — unknown order activity does not advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c04_unknown_order_does_not_advance_cursor() {
    let activity = make_fill_activity(TEST_ACTIVITY_ID, "broker-order-NOT-IN-MAP", "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C04: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C04: expect 200; body={j}");
    assert_eq!(j["unknown_order_count"], 1, "C04: unknown_order_count=1");
    assert_eq!(
        j["cursor_advanced"], false,
        "C04: cursor must not advance on unknown order"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C04: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C04: DB cursor must remain unchanged"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C05 — malformed activity (missing price) does not advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c05_malformed_activity_does_not_advance_cursor() {
    let activity = make_fill_no_price(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID);
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C05: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C05: expect 200; body={j}");
    assert_eq!(j["malformed_count"], 1, "C05: malformed_count=1");
    assert_eq!(
        j["cursor_advanced"], false,
        "C05: cursor must not advance on malformed"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C05: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C05: DB cursor must remain unchanged"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C06 — ambiguous mapping (unrecognized side) does not advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c06_ambiguous_side_does_not_advance_cursor() {
    let mut activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    activity.side = "long".to_owned(); // unrecognized side — malformed
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C06: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C06: expect 200; body={j}");
    assert_eq!(
        j["malformed_count"], 1,
        "C06: malformed_count=1 (unrecognized side)"
    );
    assert_eq!(
        j["cursor_advanced"], false,
        "C06: cursor must not advance on ambiguous side"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C06: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C06: DB cursor must remain unchanged"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C07 — REST unavailable does not advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c07_rest_unavailable_does_not_advance_cursor() {
    let fetcher = FakeGapFetcher::failing("simulated REST failure");
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C07: skipped — MQK_DATABASE_URL not set");
        return;
    };
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "C07: expect 503 on REST failure; body={j}"
    );
    assert_eq!(
        j["cursor_advanced"], false,
        "C07: cursor must not advance on REST error"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C07: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C07: DB cursor must remain unchanged after REST failure"
    );

    delete_cursor(&pool, "alpaca").await;
}

// ---------------------------------------------------------------------------
// C08 — mixed safe + unsafe batch does not advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c08_mixed_safe_unsafe_batch_does_not_advance_cursor() {
    // Two activities: one maps to known order (safe), one does not (unsafe).
    let safe_activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let unsafe_activity = make_fill_activity(
        "20260509143000000000002::a1b1c1d1e1f1a1b1",
        "broker-order-NOT-IN-MAP",
        "AAPL",
        "buy",
    );
    let fetcher = FakeGapFetcher::returning(vec![safe_activity, unsafe_activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C08: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_OLD)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C08: expect 200; body={j}");
    assert_eq!(j["recovered_count"], 1, "C08: safe fill is recovered");
    assert_eq!(j["unknown_order_count"], 1, "C08: unsafe fill is unknown");
    assert_eq!(
        j["cursor_advanced"], false,
        "C08: cursor must NOT advance on mixed unsafe batch"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C08: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_OLD),
        "C08: DB cursor must remain unchanged for mixed unsafe batch"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// C09 — cursor monotonic: existing newer anchor not overwritten by older activity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c09_cursor_is_monotonic_newer_anchor_not_overwritten() {
    // Seed cursor with CURSOR_ANCHOR_NEW (newer than TEST_ACTIVITY_ID).
    // A fill is successfully recovered, but its ID is older than the current anchor.
    // The cursor must not move backward.
    let activity = make_fill_activity(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let fetcher = FakeGapFetcher::returning(vec![activity]);
    let Some((st, pool)) = cursor_test_state(fetcher).await else {
        eprintln!("C09: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool).await;
    // Seed with NEWER anchor — activity ID (TEST_ACTIVITY_ID) is lexicographically smaller.
    seed_cursor(&pool, "alpaca", &cursor_json_with_anchor(CURSOR_ANCHOR_NEW)).await;

    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        gap_req(serde_json::json!({ "run_id": TEST_RUN_ID, "dry_run": false })),
    )
    .await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::OK, "C09: expect 200; body={j}");
    assert_eq!(j["recovered_count"], 1, "C09: fill is recovered");
    // cursor_advanced must be false because TEST_ACTIVITY_ID < CURSOR_ANCHOR_NEW.
    assert_eq!(
        j["cursor_advanced"], false,
        "C09: cursor must not advance (monotonic guard)"
    );
    assert!(
        j["new_rest_activity_after"].is_null(),
        "C09: new anchor must be null"
    );

    let anchor = cursor_rest_anchor(&pool, "alpaca").await;
    assert_eq!(
        anchor.as_deref(),
        Some(CURSOR_ANCHOR_NEW),
        "C09: DB cursor must remain at CURSOR_ANCHOR_NEW (monotonic)"
    );

    delete_cursor(&pool, "alpaca").await;
    cleanup_run(&pool).await;
}
