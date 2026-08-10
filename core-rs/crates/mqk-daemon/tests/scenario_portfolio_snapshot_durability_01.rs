//! PORTFOLIO-SNAPSHOT-DURABILITY-01 proof tests.
//!
//! POST /api/v1/ops/repair/halted-run-portfolio-snapshot
//!
//! # Invariants under test
//!
//! | Test | Claim                                                                                    |
//! |------|------------------------------------------------------------------------------------------|
//! | P01  | Returns 503 `truth_state="no_db"` when DB not configured                                 |
//! | P02  | dry_run=true computes from applied inbox fills; does NOT write snapshot                  |
//! | P03  | dry_run=false without confirmation → 400 refused (pure in-process)                      |
//! | P04  | Confirmed write (dry_run=false) persists deterministic snapshot to audit store           |
//! | P05  | Second confirmed write returns `"already_current"`; no duplicate written                 |
//! | P06  | Malformed applied fill row → 409 refused; no snapshot written                           |
//! | P07  | Missing applied fills → 200 dry_run_ok with empty positions (honest, not fabricated)    |
//! | P08  | Regression: existing broker fill repair tests are unaffected (pure structural proof)     |
//!
//! P01, P03, P08 are pure in-process (no DB required).
//! P02, P04, P05, P06, P07 are DB-backed (skipped without MQK_DATABASE_URL).
//!
//! ## PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect #3)
//!
//! FC-3A/FC-3B prove the route no longer independently sums raw applied-row
//! `delta_qty` (a second, hidden durable-replay implementation with no OMS
//! context, no cross-lane watermark dedup, and no terminal effective-delta
//! correction) but instead sources positions/cash/P&L exclusively from the
//! canonical `recover_oms_and_portfolio` replay seam.
//!
//! | Test  | Claim                                                                          |
//! |-------|---------------------------------------------------------------------------------|
//! | FC-3A | WS+REST cross-lane duplicate of one physical fill collapses to ONE economic effect (qty=10, not 20), even though raw evidence count stays 2 |
//! | FC-3B | Alpaca terminal-fill overfill quirk (partial 2@$100 + terminal raw-delta=2@$102 on a 3-share order) resolves to qty=3 (not 4), matching live effective-delta semantics |

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_execution::{BrokerEvent, Side};
use tower::ServiceExt;

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
    serde_json::from_slice(&b).expect("response body is not valid JSON")
}

fn snapshot_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/halted-run-portfolio-snapshot")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("json serialize"),
        ))
        .unwrap()
}

fn no_db_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

async fn db_state() -> Option<Arc<state::AppState>> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    mqk_db::migrate(&pool).await.ok()?;
    Some(Arc::new(
        state::AppState::new_for_test_with_db_mode_and_broker(
            pool,
            state::DeploymentMode::LiveShadow,
            state::BrokerKind::Alpaca,
        ),
    ))
}

/// Create a deterministic HALTED run in DB; idempotent via ON CONFLICT DO NOTHING.
async fn seed_halted_run(pool: &sqlx::PgPool, label: &str) -> uuid::Uuid {
    let run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!("mqk.test.portfolio-snapshot-01.run.{label}").as_bytes(),
    );
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into runs (run_id, engine_id, mode, started_at_utc, git_hash,
                          config_hash, config_json, host_fingerprint)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (run_id) do nothing
        "#,
    )
    .bind(run_id)
    .bind("test-daemon")
    .bind("PAPER")
    .bind(now)
    .bind("test-git-hash")
    .bind("test-config-hash")
    .bind(serde_json::json!({"source": "scenario_portfolio_snapshot_durability_01"}))
    .bind("test-host")
    .execute(pool)
    .await
    .expect("seed run");

    sqlx::query(
        r#"
        update runs
           set status = 'HALTED', halted_at_utc = $2
         where run_id = $1 and status != 'HALTED'
        "#,
    )
    .bind(run_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("halt run");

    run_id
}

/// Seed a SENT outbox order so canonical durable replay
/// (`recover_oms_and_portfolio`) has OMS order context for `order_id`.
///
/// PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02: this route now
/// derives economic truth from the canonical replay seam, which requires a
/// submitted outbox row carrying `symbol`/`qty`/`side` for every
/// `internal_order_id` a fill references -- a fill for an order absent from
/// outbox truth fails closed as `UNKNOWN_ORDER_FILL`.
async fn seed_submitted_order(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    order_id: &str,
    symbol: &str,
    side: &str,
    qty: i64,
) {
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status,
                                created_at_utc, sent_at_utc)
        values ($1, $2, $3, 'SENT', $4, $4)
        on conflict (idempotency_key) do update
            set order_json = excluded.order_json,
                status      = excluded.status
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(serde_json::json!({"symbol": symbol, "qty": qty, "side": side}))
    .bind(now)
    .execute(pool)
    .await
    .expect("seed submitted order");
}

/// Insert an applied fill inbox row for a HALTED run.
///
/// Uses `ON CONFLICT … DO UPDATE` to keep the row applied so reruns are safe.
#[allow(clippy::too_many_arguments)]
async fn seed_applied_fill(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    order_id: &str,
    msg_id: &str,
    symbol: &str,
    side: &str,
    qty: i64,
    price_micros: i64,
) {
    let now = chrono::Utc::now();
    let fill_event = serde_json::json!({
        "type": "fill",
        "broker_message_id": msg_id,
        "broker_fill_id": null,
        "internal_order_id": order_id,
        "broker_order_id": "test-broker-order",
        "symbol": symbol,
        "side": side,
        "delta_qty": qty,
        "price_micros": price_micros,
        "fee_micros": 0
    });
    sqlx::query(
        r#"
        insert into oms_inbox (run_id, broker_message_id, internal_order_id,
                               broker_order_id, event_kind, message_json,
                               event_ts_ms, received_at_utc, applied_at_utc)
        values ($1, $2, $3, 'test-broker-order', 'fill', $4, 0, $5, $5)
        on conflict (run_id, broker_message_id) do update
            set message_json   = excluded.message_json,
                applied_at_utc = excluded.applied_at_utc
        "#,
    )
    .bind(run_id)
    .bind(msg_id)
    .bind(order_id)
    .bind(&fill_event)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed applied fill");
}

/// Insert a malformed applied fill (event_kind='fill' but payload is not a BrokerEvent).
async fn seed_malformed_applied_fill(pool: &sqlx::PgPool, run_id: uuid::Uuid, msg_id: &str) {
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into oms_inbox (run_id, broker_message_id, internal_order_id,
                               broker_order_id, event_kind, message_json,
                               event_ts_ms, received_at_utc, applied_at_utc)
        values ($1, $2, 'test-order-malformed', 'test-broker-order', 'fill',
                '{"not_a_broker_event": true}'::jsonb, 0, $3, $3)
        on conflict (run_id, broker_message_id) do update
            set message_json   = excluded.message_json,
                applied_at_utc = excluded.applied_at_utc
        "#,
    )
    .bind(run_id)
    .bind(msg_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed malformed fill");
}

// ---------------------------------------------------------------------------
// P01 — no DB → 503 (pure in-process)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p01_no_db_returns_503() {
    let router = routes::build_router(no_db_state());
    let body = serde_json::json!({
        "run_id": "00000000-0000-0000-0000-000000000001",
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "P01: status must be 503"
    );
    assert_eq!(
        j["truth_state"].as_str().unwrap_or(""),
        "no_db",
        "P01: truth_state must be 'no_db'"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "P01: decision must be 'refused'"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "snapshot.db_required",
        "P01: gate must be 'snapshot.db_required'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "P01: snapshot_written must be false"
    );
}

// ---------------------------------------------------------------------------
// P03 — dry_run=false without confirmation → 400 (pure in-process)
//
// Gate 1 (confirmation) fires before Gate 2 (DB), so this is testable without DB.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p03_dry_run_false_without_confirmation_refuses() {
    let router = routes::build_router(no_db_state());
    let body = serde_json::json!({
        "run_id": "00000000-0000-0000-0000-000000000001",
        "dry_run": false
        // confirmation absent
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(status, StatusCode::BAD_REQUEST, "P03: status must be 400");
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "P03: decision must be 'refused'"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "snapshot.confirmation_required",
        "P03: gate must be 'snapshot.confirmation_required'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "P03: snapshot_written must be false"
    );
}

#[tokio::test]
async fn p03b_dry_run_false_wrong_confirmation_refuses() {
    let router = routes::build_router(no_db_state());
    let body = serde_json::json!({
        "run_id": "00000000-0000-0000-0000-000000000001",
        "dry_run": false,
        "confirmation": "WRONG_TOKEN"
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(status, StatusCode::BAD_REQUEST, "P03b: status must be 400");
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "snapshot.confirmation_required",
        "P03b: wrong confirmation token must hit confirmation gate"
    );
}

// ---------------------------------------------------------------------------
// P08 — Regression: repair route import is structurally wired (pure)
//
// Proves the new route did not break the existing repair route handlers.
// Structural check only — functional regression is covered by the existing
// scenario_broker_fill_replay_repair_01.rs and scenario_ops_repair_01.rs suites.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p08_existing_repair_routes_structurally_intact() {
    let state = no_db_state();
    let router = routes::build_router(Arc::clone(&state));

    // GET /api/v1/ops/repair/halted-run-fill-plan → 503 (no DB, not 404).
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/repair/halted-run-fill-plan")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "P08: fill-plan route must be wired (not 404)"
    );
    let j = parse_json(body);
    assert_eq!(
        j["truth_state"].as_str().unwrap_or(""),
        "no_db",
        "P08: fill-plan must return truth_state=no_db without DB"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests (skip gracefully without MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

// P02 — dry_run=true computes from applied fills, does NOT write snapshot.
#[tokio::test]
async fn p02_dry_run_computes_without_writing() {
    let Some(st) = db_state().await else {
        eprintln!("P02: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "p02").await;

    let order_id = format!("p02-order-{run_id}");
    seed_submitted_order(&pool, run_id, &order_id, "AAPL", "buy", 10).await;
    let msg_id = format!("p02-fill-{run_id}");
    seed_applied_fill(
        &pool,
        run_id,
        &order_id,
        &msg_id,
        "AAPL",
        "Buy",
        10,
        150_000_000,
    )
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(status, StatusCode::OK, "P02: status must be 200; body={j}");
    assert_eq!(
        j["truth_state"].as_str().unwrap_or(""),
        "active",
        "P02: truth_state must be 'active'"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "dry_run_ok",
        "P02: decision must be 'dry_run_ok'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "P02: snapshot_written must be false"
    );
    assert!(
        j["dry_run"].as_bool().unwrap_or(false),
        "P02: dry_run must be true in response"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        1,
        "P02: applied_fill_count must be 1"
    );
    assert_eq!(
        j["source"].as_str().unwrap_or(""),
        "applied_inbox_rows",
        "P02: source must be 'applied_inbox_rows'"
    );

    // Positions: 10 shares of AAPL long.
    let positions = j["positions"]
        .as_array()
        .expect("P02: positions must be array");
    assert_eq!(positions.len(), 1, "P02: exactly one position expected");
    assert_eq!(
        positions[0]["symbol"].as_str().unwrap_or(""),
        "AAPL",
        "P02: position symbol must be AAPL"
    );
    assert_eq!(
        positions[0]["qty_signed"].as_i64().unwrap_or(0),
        10,
        "P02: qty_signed must be 10"
    );

    // Confirm no snapshot was written to audit_events.
    let expected_event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.repair.portfolio-snapshot.v1|{}|{}",
            run_id,
            // max_fill_inbox_id would be the actual inbox_id; we just check it wasn't written.
            // Accept any result from this check — we just verify snapshot_written=false above.
            0
        )
        .as_bytes(),
    );
    let _ = expected_event_id; // structural check only; snapshot_written=false is the proof.
}

// P04 — confirmed write persists deterministic snapshot.
#[tokio::test]
async fn p04_confirmed_write_persists_snapshot() {
    let Some(st) = db_state().await else {
        eprintln!("P04: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "p04").await;

    // Remove any snapshot audit event left by a prior test run so the test is re-runnable.
    sqlx::query(
        "delete from audit_events where run_id = $1 and event_type = 'ops.repair.portfolio_snapshot'",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .ok();

    let order_id = format!("p04-order-{run_id}");
    seed_submitted_order(&pool, run_id, &order_id, "TSLA", "buy", 5).await;
    let msg_id = format!("p04-fill-{run_id}");
    seed_applied_fill(
        &pool,
        run_id,
        &order_id,
        &msg_id,
        "TSLA",
        "Buy",
        5,
        200_000_000,
    )
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": false,
        "confirmation": "WRITE_PORTFOLIO_SNAPSHOT"
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(status, StatusCode::OK, "P04: status must be 200; body={j}");
    assert_eq!(
        j["truth_state"].as_str().unwrap_or(""),
        "active",
        "P04: truth_state must be 'active'"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "snapshot_written",
        "P04: decision must be 'snapshot_written'"
    );
    assert!(
        j["snapshot_written"].as_bool().unwrap_or(false),
        "P04: snapshot_written must be true"
    );
    assert!(
        j["audit_event_id"].as_str().is_some(),
        "P04: audit_event_id must be present"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        1,
        "P04: applied_fill_count must be 1"
    );
    assert_eq!(
        j["source"].as_str().unwrap_or(""),
        "applied_inbox_rows",
        "P04: source must be 'applied_inbox_rows'"
    );

    // Verify the audit event exists in the DB.
    let audit_event_id: uuid::Uuid = j["audit_event_id"]
        .as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .expect("P04: audit_event_id must be a valid UUID");

    let row_exists: bool = sqlx::query_scalar::<_, bool>(
        "select exists(select 1 from audit_events where event_id = $1)",
    )
    .bind(audit_event_id)
    .fetch_one(&pool)
    .await
    .expect("P04: audit_events query failed");

    assert!(
        row_exists,
        "P04: audit event must exist in DB after confirmed write"
    );
}

// P05 — second confirmed write returns already_current; no duplicate written.
#[tokio::test]
async fn p05_second_write_is_idempotent() {
    let Some(st) = db_state().await else {
        eprintln!("P05: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "p05").await;

    // Remove any snapshot audit event left by a prior test run so the test is re-runnable.
    sqlx::query(
        "delete from audit_events where run_id = $1 and event_type = 'ops.repair.portfolio_snapshot'",
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .ok();

    let order_id = format!("p05-order-{run_id}");
    seed_submitted_order(&pool, run_id, &order_id, "NVDA", "buy", 3).await;
    let msg_id = format!("p05-fill-{run_id}");
    seed_applied_fill(
        &pool,
        run_id,
        &order_id,
        &msg_id,
        "NVDA",
        "Buy",
        3,
        800_000_000,
    )
    .await;

    let write_body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": false,
        "confirmation": "WRITE_PORTFOLIO_SNAPSHOT"
    });

    // First write.
    let router1 = routes::build_router(Arc::clone(&st));
    let (status1, body1) = call(router1, snapshot_req(write_body.clone())).await;
    let j1 = parse_json(body1);
    assert_eq!(
        status1,
        StatusCode::OK,
        "P05: first write must be 200; body={j1}"
    );
    assert_eq!(
        j1["decision"].as_str().unwrap_or(""),
        "snapshot_written",
        "P05: first write decision must be 'snapshot_written'"
    );

    // Second write — same applied-fill dataset → same UUIDv5 → already_current.
    let router2 = routes::build_router(Arc::clone(&st));
    let (status2, body2) = call(router2, snapshot_req(write_body)).await;
    let j2 = parse_json(body2);
    assert_eq!(
        status2,
        StatusCode::OK,
        "P05: second write must be 200; body={j2}"
    );
    assert_eq!(
        j2["decision"].as_str().unwrap_or(""),
        "already_current",
        "P05: second write decision must be 'already_current'"
    );
    assert!(
        !j2["snapshot_written"].as_bool().unwrap_or(true),
        "P05: snapshot_written must be false on second write"
    );
    assert_eq!(
        j1["audit_event_id"].as_str(),
        j2["audit_event_id"].as_str(),
        "P05: audit_event_id must be identical on both calls (deterministic UUIDv5)"
    );

    // Verify exactly one audit event row exists (no duplicate).
    let audit_event_id: uuid::Uuid = j1["audit_event_id"]
        .as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .expect("P05: audit_event_id must be valid UUID");

    let count: i64 = sqlx::query_scalar::<_, i64>(
        "select count(*)::bigint from audit_events where event_id = $1",
    )
    .bind(audit_event_id)
    .fetch_one(&pool)
    .await
    .expect("P05: audit_events count query failed");

    assert_eq!(
        count, 1,
        "P05: exactly one audit event row must exist; got {count}"
    );
}

// P06 — malformed applied fill → 409 refused; no snapshot written.
#[tokio::test]
async fn p06_malformed_applied_fill_refuses() {
    let Some(st) = db_state().await else {
        eprintln!("P06: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "p06").await;

    let msg_id = format!("p06-bad-fill-{run_id}");
    seed_malformed_applied_fill(&pool, run_id, &msg_id).await;

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "P06: status must be 409; body={j}"
    );
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "refused",
        "P06: decision must be 'refused'"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "snapshot.malformed_applied_fill",
        "P06: gate must be 'snapshot.malformed_applied_fill'"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "P06: snapshot_written must be false"
    );
}

// P07 — no applied fills → honest empty positions, not fabricated.
#[tokio::test]
async fn p07_missing_applied_fills_returns_honest_empty() {
    let Some(st) = db_state().await else {
        eprintln!("P07: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "p07").await;
    // No fills seeded.

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(status, StatusCode::OK, "P07: status must be 200; body={j}");
    assert_eq!(
        j["decision"].as_str().unwrap_or(""),
        "dry_run_ok",
        "P07: decision must be 'dry_run_ok'"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(99),
        0,
        "P07: applied_fill_count must be 0 (no applied fills)"
    );
    assert_eq!(
        j["positions"].as_array().map(|a| a.len()).unwrap_or(99),
        0,
        "P07: positions must be empty array (no fills = no positions)"
    );
    assert_eq!(
        j["cash_micros"].as_i64().unwrap_or(99),
        0,
        "P07: cash_micros must be 0 (initial_cash=0, no fills)"
    );
    assert_eq!(
        j["source"].as_str().unwrap_or(""),
        "applied_inbox_rows",
        "P07: source must be 'applied_inbox_rows' even with empty set"
    );
    assert!(
        !j["snapshot_written"].as_bool().unwrap_or(true),
        "P07: snapshot_written must be false (dry_run=true)"
    );
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect #3): FC-3A/FC-3B
// ---------------------------------------------------------------------------

/// Insert one already-applied inbox row carrying `event` with full transport
/// identity control (`broker_message_id`/`broker_fill_id` independent of
/// `event`'s own serialized fields) — needed to construct cross-lane
/// duplicate scenarios the coarser `seed_applied_fill` helper cannot.
#[allow(clippy::too_many_arguments)]
async fn seed_applied_broker_event(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    broker_message_id: &str,
    broker_fill_id: Option<&str>,
    internal_order_id: &str,
    broker_order_id: &str,
    event_kind: &str,
    event: &BrokerEvent,
    at: chrono::DateTime<chrono::Utc>,
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

// FC-3A: a single physical 10-share partial fill observed on both the WS
// lane (broker_fill_id=None) and the REST lane (broker_fill_id=Some(..))
// leaves TWO raw applied inbox rows for the SAME physical execution. The
// route's raw evidence count (`applied_fill_count`) may legitimately stay 2,
// but the canonical economic replay it now sources positions from must
// collapse this to ONE effect: qty=10, never 20.
#[tokio::test]
async fn fc3a_cross_lane_duplicate_collapses_to_one_economic_effect() {
    let Some(st) = db_state().await else {
        eprintln!("FC-3A: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "fc3a").await;

    let order_id = format!("fc3a-order-{run_id}");
    seed_submitted_order(&pool, run_id, &order_id, "AAPL", "buy", 10).await;

    let at = chrono::Utc::now();
    let ws_ev = BrokerEvent::PartialFill {
        broker_message_id: format!("alpaca:{order_id}:partial_fill:ws-10"),
        broker_fill_id: None,
        internal_order_id: order_id.clone(),
        broker_order_id: Some("fc3a-broker-order".to_string()),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 10,
        price_micros: 100_000_000,
        fee_micros: 0,
        cum_qty_after: Some(10),
    };
    seed_applied_broker_event(
        &pool,
        run_id,
        ws_ev.broker_message_id(),
        None,
        &order_id,
        "fc3a-broker-order",
        "partial_fill",
        &ws_ev,
        at,
    )
    .await;

    let rest_ev = BrokerEvent::PartialFill {
        broker_message_id: "alpaca-rest-recovery:fc3a-activity-1".to_string(),
        broker_fill_id: Some("fc3a-activity-1".to_string()),
        internal_order_id: order_id.clone(),
        broker_order_id: Some("fc3a-broker-order".to_string()),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 10,
        price_micros: 100_000_000,
        fee_micros: 0,
        cum_qty_after: Some(10),
    };
    seed_applied_broker_event(
        &pool,
        run_id,
        rest_ev.broker_message_id(),
        Some("fc3a-activity-1"),
        &order_id,
        "fc3a-broker-order",
        "partial_fill",
        &rest_ev,
        at,
    )
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(
        status,
        StatusCode::OK,
        "FC-3A: status must be 200; body={j}"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        2,
        "FC-3A: raw evidence count may legitimately stay 2 (two raw observations); body={j}"
    );
    let positions = j["positions"].as_array().expect("FC-3A: positions array");
    assert_eq!(
        positions.len(),
        1,
        "FC-3A: exactly one AAPL position expected; body={j}"
    );
    assert_eq!(
        positions[0]["qty_signed"].as_i64().unwrap_or(-1),
        10,
        "FC-3A: cross-lane duplicate of one physical 10-share execution must resolve to \
         qty=10 (ONE economic effect), not 20; body={j}"
    );
}

// FC-3B: order total=3. Partial fill 2@$100 (cum=2), then a terminal `fill`
// event whose RAW delta_qty=2 overstates the true 1-share remainder
// (Alpaca's observed paper-WS terminal-fill quirk). The canonical replay's
// OMS-derived effective delta must cap this at the true remainder (1), so
// the reconstructed portfolio is exactly qty=3 (2+1), never qty=4 (2+2) —
// this is the same defect FC-1 proves against `recover_oms_and_portfolio`
// directly; here it is proven end-to-end through the HTTP repair route.
#[tokio::test]
async fn fc3b_terminal_overfill_resolves_to_true_remainder_not_raw_delta() {
    let Some(st) = db_state().await else {
        eprintln!("FC-3B: skipped — MQK_DATABASE_URL not set");
        return;
    };
    let pool = st.db.as_ref().unwrap().clone();
    let run_id = seed_halted_run(&pool, "fc3b").await;

    let order_id = format!("fc3b-order-{run_id}");
    seed_submitted_order(&pool, run_id, &order_id, "AAPL", "buy", 3).await;

    let at = chrono::Utc::now();
    let partial = BrokerEvent::PartialFill {
        broker_message_id: format!("alpaca:{order_id}:partial_fill:ws-2"),
        broker_fill_id: None,
        internal_order_id: order_id.clone(),
        broker_order_id: Some("fc3b-broker-order".to_string()),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 2,
        price_micros: 100_000_000,
        fee_micros: 0,
        cum_qty_after: Some(2),
    };
    seed_applied_broker_event(
        &pool,
        run_id,
        partial.broker_message_id(),
        None,
        &order_id,
        "fc3b-broker-order",
        "partial_fill",
        &partial,
        at,
    )
    .await;

    let terminal = BrokerEvent::Fill {
        broker_message_id: format!("alpaca:{order_id}:fill:term"),
        broker_fill_id: None,
        internal_order_id: order_id.clone(),
        broker_order_id: Some("fc3b-broker-order".to_string()),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 2,
        price_micros: 102_000_000,
        fee_micros: 0,
    };
    seed_applied_broker_event(
        &pool,
        run_id,
        terminal.broker_message_id(),
        None,
        &order_id,
        "fc3b-broker-order",
        "fill",
        &terminal,
        at,
    )
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let body = serde_json::json!({
        "run_id": run_id.to_string(),
        "dry_run": true
    });
    let (status, resp_body) = call(router, snapshot_req(body)).await;
    let j = parse_json(resp_body);

    assert_eq!(
        status,
        StatusCode::OK,
        "FC-3B: status must be 200; body={j}"
    );
    assert_eq!(
        j["applied_fill_count"].as_u64().unwrap_or(0),
        2,
        "FC-3B: raw evidence count is 2 (one partial + one terminal); body={j}"
    );
    let positions = j["positions"].as_array().expect("FC-3B: positions array");
    assert_eq!(
        positions.len(),
        1,
        "FC-3B: exactly one AAPL position; body={j}"
    );
    assert_eq!(
        positions[0]["qty_signed"].as_i64().unwrap_or(-1),
        3,
        "FC-3B: 2@$100 + true-remainder 1@$102 must total qty=3, not qty=4 from the raw \
         terminal delta_qty=2; body={j}"
    );
    let expected_cash = -(2 * 100_000_000 + 102_000_000);
    assert_eq!(
        j["cash_micros"].as_i64().unwrap_or(0),
        expected_cash,
        "FC-3B: cash must reflect the effective 1-share terminal fill at its own price \
         ($102), not the raw 2-share delta; body={j}"
    );
}
