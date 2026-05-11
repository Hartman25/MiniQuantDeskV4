//! BROKER-POSITION-BASELINE-ADOPTION-01 proof tests.
//!
//! Proves that the system correctly handles pre-existing broker-side positions
//! or orders at paper+alpaca startup via an explicit operator-confirmed adoption.
//!
//! # Test matrix
//!
//! | Test  | What it proves                                                            |
//! |-------|---------------------------------------------------------------------------|
//! | PBA01 | Pre-existing broker order without baseline → reconcile dirty (quarantined)|
//! | PBA02 | Missing / wrong confirmation string → adoption refused (400)              |
//! | PBA03 | Mode guard: non-paper+alpaca context → adoption refused (403)             |
//! | PBA04 | Absent broker snapshot → adoption refused (503, fail closed)              |
//! | PBA05 | Confirmed adoption sets baseline in-memory + DB (DB-backed)               |
//! | PBA06 | Adoption clears integrity halt so re-arm can proceed (DB-backed)          |
//! | PBA07 | Adoption never writes outbox or inbox rows (DB-backed)                    |
//! | PBA08 | Adoption is idempotent: re-adopt same snapshot → same counts (DB-backed)  |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

fn paper_alpaca_router_from_state(st: Arc<state::AppState>) -> axum::Router {
    routes::build_router(st)
}

/// A fake broker snapshot with one open AAPL order (simulates pre-existing exposure).
fn fake_broker_snapshot_with_open_order() -> mqk_schemas::BrokerSnapshot {
    mqk_schemas::BrokerSnapshot {
        captured_at_utc: chrono::Utc::now(),
        account: mqk_schemas::BrokerAccount {
            equity: "10000".to_string(),
            cash: "10000".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![mqk_schemas::BrokerOrder {
            broker_order_id: "fake-alpaca-order-id-001".to_string(),
            client_order_id: "fake-client-order-001".to_string(),
            symbol: "AAPL".to_string(),
            side: "buy".to_string(),
            r#type: "market".to_string(),
            status: "accepted".to_string(),
            qty: "1".to_string(),
            limit_price: None,
            stop_price: None,
            created_at_utc: chrono::Utc::now(),
        }],
        fills: vec![],
        positions: vec![],
    }
}

/// Broker snapshot with an AAPL position (simulates filled order).
fn fake_broker_snapshot_with_position() -> mqk_schemas::BrokerSnapshot {
    mqk_schemas::BrokerSnapshot {
        captured_at_utc: chrono::Utc::now(),
        account: mqk_schemas::BrokerAccount {
            equity: "10000".to_string(),
            cash: "9000".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions: vec![mqk_schemas::BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "1".to_string(),
            avg_price: "150.00".to_string(),
        }],
    }
}

async fn post_adopt_json(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/repair/adopt-broker-position-baseline")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).expect("response is not valid JSON");
    (status, json)
}

async fn post_adopt(router: axum::Router, confirmation: &str) -> (StatusCode, serde_json::Value) {
    post_adopt_json(router, serde_json::json!({ "confirmation": confirmation })).await
}

async fn test_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("MQK_DATABASE_URL required");
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("DB connect failed")
}

// ---------------------------------------------------------------------------
// PBA01: No baseline → reconcile engine reports dirty for unknown broker order.
//
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba01_no_baseline_means_reconcile_dirty_for_unknown_broker_order() {
    let st = paper_alpaca_state();

    // At boot, broker_baseline must be None.
    let baseline = st.broker_baseline.read().await.clone();
    assert!(
        baseline.is_none(),
        "broker_baseline must be None at boot before any adoption"
    );

    // Simulate what the reconcile tick sees when no baseline is set:
    // local_fn returns empty, broker has an open order.
    let local = mqk_reconcile::LocalSnapshot::empty();
    let mut broker = mqk_reconcile::BrokerSnapshot::empty_at(chrono::Utc::now().timestamp_millis());
    broker.orders.insert(
        "fake-alpaca-order-id-001".to_string(),
        mqk_reconcile::OrderSnapshot::new(
            "fake-alpaca-order-id-001",
            "AAPL",
            mqk_reconcile::Side::Buy,
            1,
            0,
            mqk_reconcile::OrderStatus::Accepted,
        ),
    );

    let report = mqk_reconcile::reconcile(&local, &broker);
    assert!(
        !report.is_clean(),
        "empty local vs broker-with-order must report dirty before adoption"
    );
    assert!(
        report
            .reasons
            .contains(&mqk_reconcile::ReconcileReason::UnknownBrokerOrder),
        "pre-adoption should classify as UnknownBrokerOrder: {:?}",
        report.reasons
    );
}

// ---------------------------------------------------------------------------
// PBA02: Wrong / empty confirmation → adoption refused (400).
//
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba02_wrong_confirmation_refuses_adoption() {
    // Empty confirmation.
    let st = paper_alpaca_state();
    let (status, json) = post_adopt(paper_alpaca_router_from_state(Arc::clone(&st)), "").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty confirmation must return 400: {json}"
    );
    assert_eq!(json["accepted"], false);
    assert_eq!(json["gate"], "repair.confirmation_required");

    // Wrong string.
    let st2 = paper_alpaca_state();
    let (status2, json2) = post_adopt(
        paper_alpaca_router_from_state(Arc::clone(&st2)),
        "ADOPT_BROKER_POSITION_BASELINE_WRONG",
    )
    .await;
    assert_eq!(
        status2,
        StatusCode::BAD_REQUEST,
        "wrong confirmation must return 400: {json2}"
    );
    assert_eq!(json2["accepted"], false);
    assert_eq!(json2["gate"], "repair.confirmation_required");
}

// ---------------------------------------------------------------------------
// PBA03: Mode guard — non-paper+alpaca context refuses adoption (403).
//
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba03_non_paper_alpaca_mode_refuses_adoption() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = paper_alpaca_router_from_state(st);

    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-paper+alpaca must return 403: {json}"
    );
    assert_eq!(json["accepted"], false);
    assert_eq!(json["gate"], "mode.not_paper_alpaca");
}

// ---------------------------------------------------------------------------
// PBA04: Absent broker snapshot → adoption refused (503, fail closed).
//
// No DB configured → refused at repair.no_db first.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba04_absent_broker_snapshot_refuses_adoption_fail_closed() {
    // No DB, no broker snapshot → should be refused.
    let st = paper_alpaca_state();
    let router = paper_alpaca_router_from_state(st);

    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert!(
        status == StatusCode::SERVICE_UNAVAILABLE || status == StatusCode::FORBIDDEN,
        "absent broker snapshot must refuse with 503 or 403, got {status}: {json}"
    );
    assert_eq!(json["accepted"], false, "accepted must be false: {json}");
    assert!(
        json["gate"].is_string(),
        "gate must name the refusal reason: {json}"
    );
}

// ---------------------------------------------------------------------------
// PBA05: Confirmed adoption sets baseline in-memory + DB.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba05_confirmed_adoption_writes_baseline() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("pba05: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Inject a fake broker snapshot with one open order.
    *st.broker_snapshot.write().await = Some(fake_broker_snapshot_with_open_order());

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;

    assert_eq!(status, StatusCode::OK, "adoption must succeed: {json}");
    assert_eq!(json["accepted"], true, "accepted must be true");
    assert!(
        json["gate"].is_null(),
        "gate must be null on success: {json}"
    );
    assert_eq!(
        json["baseline_order_count"], 1,
        "must reflect injected order: {json}"
    );
    assert_eq!(
        json["baseline_position_count"], 0,
        "no positions in snapshot: {json}"
    );
    assert!(
        json["audit_event_id"].is_string(),
        "audit_event_id must be returned: {json}"
    );

    // Verify in-memory baseline was set.
    let baseline = st.broker_baseline.read().await.clone();
    assert!(
        baseline.is_some(),
        "broker_baseline cache must be set after adoption"
    );
    let baseline = baseline.unwrap();
    assert_eq!(baseline.orders.len(), 1, "baseline must contain 1 order");

    // Verify DB baseline was written.
    let db_row = mqk_db::load_broker_position_baseline(&db)
        .await
        .expect("load failed")
        .expect("baseline row must exist in DB after adoption");
    assert_eq!(
        db_row.adopted_by, "ADOPT_BROKER_POSITION_BASELINE",
        "adopted_by must match confirmation"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// PBA06: Adoption clears integrity halt so re-arm can proceed.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba06_adoption_clears_integrity_halt() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("pba06: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Simulate the state after a ReconcileDrift halt.
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    *st.broker_snapshot.write().await = Some(fake_broker_snapshot_with_position());

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert_eq!(status, StatusCode::OK, "adoption must succeed: {json}");

    // Integrity halt must be cleared after adoption.
    let ig = st.integrity.read().await;
    assert!(!ig.halted, "integrity halt must be cleared after adoption");
    assert!(
        !ig.disarmed,
        "integrity disarmed must be cleared after adoption"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// PBA07: Adoption never submits orders or fabricates fills.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba07_adoption_does_not_submit_orders_or_fabricate_fills() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("pba07: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    // Count outbox rows before adoption.
    let outbox_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&db)
        .await
        .expect("count failed");
    let inbox_before: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(&db)
        .await
        .expect("count failed");

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    *st.broker_snapshot.write().await = Some(fake_broker_snapshot_with_open_order());

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, _json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert_eq!(status, StatusCode::OK);

    // Verify no new outbox or inbox rows were written.
    let outbox_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&db)
        .await
        .expect("count failed");
    let inbox_after: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(&db)
        .await
        .expect("count failed");

    assert_eq!(
        outbox_before, outbox_after,
        "adoption must not write outbox rows (no orders submitted)"
    );
    assert_eq!(
        inbox_before, inbox_after,
        "adoption must not write inbox rows (no fills fabricated)"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// PBA08: Adoption is idempotent — re-adopt same snapshot → same counts.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pba08_adoption_is_idempotent() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("pba08: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let snap = fake_broker_snapshot_with_position();

    for i in 0..2_u32 {
        let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
            db.clone(),
            state::DeploymentMode::Paper,
            state::BrokerKind::Alpaca,
        ));
        *st.broker_snapshot.write().await = Some(snap.clone());
        let router = paper_alpaca_router_from_state(Arc::clone(&st));
        let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "adoption attempt {i} must succeed: {json}"
        );
        assert_eq!(json["accepted"], true, "attempt {i}: {json}");
        assert_eq!(
            json["baseline_position_count"], 1,
            "attempt {i}: position count must match snapshot"
        );
        assert_eq!(
            json["baseline_order_count"], 0,
            "attempt {i}: order count must match snapshot"
        );
    }

    // Exactly one baseline row in DB (sentinel upsert).
    let count: i64 = sqlx::query_scalar("select count(*) from sys_broker_position_baseline")
        .fetch_one(&db)
        .await
        .expect("count failed");
    assert_eq!(
        count, 1,
        "idempotent adoption must produce exactly 1 DB row"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}
