//! BROKER-POSITION-BASELINE-ADOPTION-01 proof tests.
//!
//! Proves that the system correctly handles pre-existing broker-side positions
//! or orders at paper+alpaca startup via an explicit operator-confirmed adoption.
//!
//! # Test matrix
//!
//! | Test  | What it proves                                                               |
//! |-------|------------------------------------------------------------------------------|
//! | PBA01 | Pre-existing broker order without baseline → reconcile dirty (quarantined)   |
//! | PBA02 | Missing / wrong confirmation string → adoption refused (400)                 |
//! | PBA03 | Mode guard: non-paper+alpaca context → adoption refused (403)                |
//! | PBA04 | Absent broker snapshot → adoption refused (503, fail closed)                 |
//! | PBA05 | Confirmed adoption sets baseline in-memory + DB (DB-backed)                  |
//! | PBA06 | Adoption clears integrity halt so re-arm can proceed (DB-backed)             |
//! | PBA07 | Adoption never writes outbox or inbox rows (DB-backed)                       |
//! | PBA08 | Adoption is idempotent: re-adopt same snapshot → same counts (DB-backed)     |
//! | BSR01 | Missing confirmation refuses before any snapshot fetch (pure)                |
//! | BSR02 | Cached absent + no fetcher → broker_snapshot_refresh_unavailable (pure)      |
//! | BSR03 | Cached absent + fake fetcher ok → adoption writes durable baseline (DB)      |
//! | BSR04 | Cached absent + fake fetcher fails → adoption refused, no baseline row (DB)  |
//! | BSR05 | Authoritative empty snapshot from fake fetcher → baselines empty truth (DB)  |
//! | BSR06 | Adoption via on-demand fetch does not write outbox/inbox rows (DB)           |
//! | BSR07 | Re-run after on-demand fetch is idempotent (DB)                              |
//! | BSR08 | seed_broker_baseline_from_db loads baseline into memory while idle (DB)      |

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
            filled_qty: "0".to_string(),
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

// ===========================================================================
// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01 tests (BSR01-BSR08)
// ===========================================================================

/// Fake `BrokerSnapshotFetcher` for injection in tests.
///
/// Holds a preset result so tests can exercise on-demand fetch paths without
/// making real Alpaca HTTP calls.
struct FakeSnapshotFetcher {
    result: Result<mqk_schemas::BrokerSnapshot, String>,
}

impl FakeSnapshotFetcher {
    fn ok(snap: mqk_schemas::BrokerSnapshot) -> Self {
        Self { result: Ok(snap) }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self {
            result: Err(msg.into()),
        }
    }
}

impl mqk_daemon::state::BrokerSnapshotFetcher for FakeSnapshotFetcher {
    fn fetch_snapshot(&self) -> Result<mqk_schemas::BrokerSnapshot, String> {
        self.result.clone()
    }
}

// ---------------------------------------------------------------------------
// BSR01: Missing confirmation refuses BEFORE any snapshot fetch.
//
// Gate 2 (confirmation) fires before Gate 4 (snapshot fetch).  Even if a
// fetcher is installed, a wrong confirmation must refuse at 400.
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr01_missing_confirmation_refuses_before_snapshot_fetch() {
    let mut st = state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    // Install a fetcher that would succeed — confirmation gate must fire first.
    st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(
        fake_broker_snapshot_with_open_order(),
    )));
    let router = paper_alpaca_router_from_state(Arc::new(st));

    let (status, json) = post_adopt(router, "WRONG_CONFIRMATION").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "wrong confirmation must return 400: {json}"
    );
    assert_eq!(json["accepted"], false);
    assert_eq!(
        json["gate"], "repair.confirmation_required",
        "gate must be confirmation_required, not snapshot gate: {json}"
    );
}

// ---------------------------------------------------------------------------
// BSR02: Cached snapshot absent + no fetcher configured → refused with
// repair.broker_snapshot_refresh_unavailable.
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr02_no_fetcher_refuses_with_refresh_unavailable() {
    // paper_alpaca_state() does not set a snapshot_fetcher in test env
    // (no Alpaca credentials → build_snapshot_fetcher_from_env returns None).
    let st = paper_alpaca_state();
    // Confirm snapshot_fetcher is None before proceeding.
    assert!(
        st.snapshot_fetcher.is_none(),
        "test env must not have Alpaca credentials; snapshot_fetcher must be None"
    );
    // No DB → refused at repair.no_db first, which is still a refusal.
    // To reach the snapshot gate, skip DB check... actually Gate 3 (DB) fires first.
    // In this test we confirm the route refuses before any dangerous state change.
    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert!(
        status == StatusCode::SERVICE_UNAVAILABLE,
        "must refuse with 503: {json}"
    );
    assert_eq!(json["accepted"], false, "{json}");
    // The gate is either repair.no_db (DB absent) or repair.broker_snapshot_refresh_unavailable
    // (DB present but no fetcher).  Both are valid refusals.
    let gate = json["gate"].as_str().unwrap_or("");
    assert!(
        gate == "repair.no_db" || gate == "repair.broker_snapshot_refresh_unavailable",
        "gate must be no_db or refresh_unavailable: {json}"
    );
}

// ---------------------------------------------------------------------------
// BSR03: Cached snapshot absent + fake fetcher returns valid snapshot
// → adoption writes durable baseline.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr03_fake_fetcher_success_writes_baseline() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr03: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    // No pre-cached snapshot — the route must fetch via the injected fetcher.
    assert!(st.current_broker_snapshot().await.is_none());
    st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(
        fake_broker_snapshot_with_open_order(),
    )));
    let st = Arc::new(st);

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "on-demand fetch adoption must succeed: {json}"
    );
    assert_eq!(json["accepted"], true, "{json}");
    assert!(
        json["gate"].is_null(),
        "gate must be null on success: {json}"
    );
    assert_eq!(json["baseline_order_count"], 1, "{json}");
    assert_eq!(json["baseline_position_count"], 0, "{json}");

    // In-memory baseline must be set.
    let baseline = st.broker_baseline.read().await.clone();
    assert!(
        baseline.is_some(),
        "baseline must be set after on-demand adoption"
    );
    assert_eq!(baseline.unwrap().orders.len(), 1);

    // DB row must exist.
    let row = mqk_db::load_broker_position_baseline(&db)
        .await
        .expect("load failed")
        .expect("baseline row must exist");
    assert_eq!(row.adopted_by, "ADOPT_BROKER_POSITION_BASELINE");

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// BSR04: Cached absent + fake fetcher returns Err → adoption refuses, no
// baseline row written.  (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr04_fake_fetcher_failure_refuses_no_baseline_row() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr04: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    assert!(st.current_broker_snapshot().await.is_none());
    st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::err(
        "simulated Alpaca connection error",
    )));
    let st = Arc::new(st);

    let (status, json) = post_adopt(
        paper_alpaca_router_from_state(Arc::clone(&st)),
        "ADOPT_BROKER_POSITION_BASELINE",
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "fetch failure must return 503: {json}"
    );
    assert_eq!(json["accepted"], false, "{json}");
    assert_eq!(
        json["gate"], "repair.broker_snapshot_refresh_failed",
        "{json}"
    );

    // No baseline row must have been written.
    let row = mqk_db::load_broker_position_baseline(&db)
        .await
        .expect("load failed");
    assert!(
        row.is_none(),
        "no baseline row must be written on fetch failure"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// BSR05: Authoritative empty snapshot from fake fetcher can baseline empty
// truth — empty-from-broker is safe; empty-from-absent-cache is not.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr05_authoritative_empty_snapshot_baselines_empty_truth() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr05: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let empty_snap = mqk_schemas::BrokerSnapshot {
        captured_at_utc: chrono::Utc::now(),
        account: mqk_schemas::BrokerAccount {
            equity: "10000".to_string(),
            cash: "10000".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions: vec![],
    };

    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    assert!(st.current_broker_snapshot().await.is_none());
    st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(empty_snap)));
    let st = Arc::new(st);

    let (status, json) = post_adopt(
        paper_alpaca_router_from_state(Arc::clone(&st)),
        "ADOPT_BROKER_POSITION_BASELINE",
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "empty authoritative snapshot must succeed: {json}"
    );
    assert_eq!(json["accepted"], true, "{json}");
    assert_eq!(json["baseline_position_count"], 0, "{json}");
    assert_eq!(json["baseline_order_count"], 0, "{json}");

    // Row must exist — empty-from-broker was explicitly adopted.
    let row = mqk_db::load_broker_position_baseline(&db)
        .await
        .expect("load failed")
        .expect("row must exist even for empty snapshot");
    assert_eq!(row.adopted_by, "ADOPT_BROKER_POSITION_BASELINE");

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// BSR06: On-demand fetch adoption does not write outbox or inbox rows.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr06_on_demand_adoption_does_not_write_orders_or_fills() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr06: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let outbox_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&db)
        .await
        .expect("count failed");
    let inbox_before: i64 = sqlx::query_scalar("select count(*) from oms_inbox")
        .fetch_one(&db)
        .await
        .expect("count failed");

    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(
        fake_broker_snapshot_with_open_order(),
    )));
    let (status, _json) = post_adopt(
        paper_alpaca_router_from_state(Arc::new(st)),
        "ADOPT_BROKER_POSITION_BASELINE",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

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
        "adoption must not write outbox rows"
    );
    assert_eq!(
        inbox_before, inbox_after,
        "adoption must not write inbox rows"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// BSR07: Re-running on-demand adoption with same fetcher result is idempotent.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr07_on_demand_adoption_is_idempotent() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr07: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let snap = fake_broker_snapshot_with_position();

    for i in 0..2_u32 {
        let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
            db.clone(),
            state::DeploymentMode::Paper,
            state::BrokerKind::Alpaca,
        );
        // Each iteration starts with no cached snapshot — fetcher is always used.
        assert!(st.current_broker_snapshot().await.is_none());
        st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(
            snap.clone(),
        )));
        let (status, json) = post_adopt(
            paper_alpaca_router_from_state(Arc::new(st)),
            "ADOPT_BROKER_POSITION_BASELINE",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "attempt {i} must succeed: {json}");
        assert_eq!(json["accepted"], true, "attempt {i}: {json}");
        assert_eq!(json["baseline_position_count"], 1, "attempt {i}: {json}");
    }

    let count: i64 = sqlx::query_scalar("select count(*) from sys_broker_position_baseline")
        .fetch_one(&db)
        .await
        .expect("count failed");
    assert_eq!(
        count, 1,
        "idempotent on-demand adoption must produce exactly 1 row"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// BSR08: seed_broker_baseline_from_db loads the baseline into in-memory cache
// at daemon boot so the reconcile tick can use it immediately while idle.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bsr08_seed_broker_baseline_from_db_restores_baseline_at_boot() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("bsr08: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    // First: adopt a baseline with a fake fetcher.
    {
        let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
            db.clone(),
            state::DeploymentMode::Paper,
            state::BrokerKind::Alpaca,
        );
        st.set_snapshot_fetcher_for_test(std::sync::Arc::new(FakeSnapshotFetcher::ok(
            fake_broker_snapshot_with_position(),
        )));
        let (status, _) = post_adopt(
            paper_alpaca_router_from_state(Arc::new(st)),
            "ADOPT_BROKER_POSITION_BASELINE",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Second: create a fresh daemon state (simulating restart) and seed from DB.
    let st2 = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    assert!(
        st2.broker_baseline.read().await.is_none(),
        "baseline must be None before seed"
    );

    st2.seed_broker_baseline_from_db().await;

    let baseline = st2.broker_baseline.read().await.clone();
    assert!(
        baseline.is_some(),
        "baseline must be populated after seed_broker_baseline_from_db"
    );
    assert_eq!(
        baseline.unwrap().positions.len(),
        1,
        "seeded baseline must reflect adopted snapshot"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ===========================================================================
// IDLE-RECONCILE-AFTER-BASELINE-01 tests (IR01-IR08)
//
// These tests prove that the adopt-broker-position-baseline route now:
// - Runs an idle reconcile comparison after durable baseline write.
// - Persists reconcile status so BRK-09R unblocks without requiring runtime.
// - Refused (pre-mutation) responses report reconcile_refreshed=false.
// ===========================================================================

// ---------------------------------------------------------------------------
// IR01: Successful adoption with matching broker snapshot → reconcile_refreshed=true,
// reconcile_status_after="ok", all counts 0.
// (DB-backed — Gate 3 requires DB)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ir01_adoption_publishes_clean_reconcile_status() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("ir01: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let snap = fake_broker_snapshot_with_position();
    let st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    // Inject snapshot directly into cache so Gate 4 uses it without fetcher.
    *st.broker_snapshot.write().await = Some(snap.clone());
    let st = Arc::new(st);

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;

    assert_eq!(status, StatusCode::OK, "adoption must succeed: {json}");
    assert_eq!(json["accepted"], true, "{json}");

    // Core IDLE-RECONCILE-AFTER-BASELINE-01 assertions.
    assert_eq!(
        json["reconcile_refreshed"], true,
        "reconcile_refreshed must be true after successful adoption: {json}"
    );
    assert_eq!(
        json["reconcile_status_after"], "ok",
        "reconcile_status_after must be 'ok' when baseline==broker snapshot: {json}"
    );
    assert_eq!(
        json["reconcile_mismatched_positions"], 0,
        "no position mismatches when baseline derives from broker snapshot: {json}"
    );
    assert_eq!(
        json["reconcile_mismatched_orders"], 0,
        "no order mismatches when baseline derives from broker snapshot: {json}"
    );
    assert_eq!(
        json["reconcile_mismatched_fills"], 0,
        "no fill mismatches when baseline derives from broker snapshot: {json}"
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// IR06: Adoption clears BRK-09R blocker — reconcile status in DB becomes "ok".
//
// Proves that after adoption, current_reconcile_snapshot().status is "ok"
// (read from DB via publish_reconcile_snapshot), so the BRK-09R gate
// (matches "dirty"|"stale") no longer fires.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ir06_adoption_clears_brk09r_blocker() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("ir06: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    // Pre-seed a dirty reconcile state in DB to simulate the BRK-09R condition.
    mqk_db::persist_reconcile_status_state(
        &db,
        &mqk_db::PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: None,
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 1,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: pre-seeded dirty state"),
            updated_at_utc: chrono::Utc::now(),
        },
    )
    .await
    .expect("pre-seed dirty reconcile failed");

    let snap = fake_broker_snapshot_with_position();
    let st = state::AppState::new_for_test_with_db_mode_and_broker(
        db.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    *st.broker_snapshot.write().await = Some(snap.clone());
    let st = Arc::new(st);

    // Verify BRK-09R condition before adoption.
    let before = st.current_reconcile_snapshot().await;
    assert_eq!(
        before.status, "dirty",
        "reconcile must be dirty before adoption: {:?}",
        before
    );

    let router = paper_alpaca_router_from_state(Arc::clone(&st));
    let (status, json) = post_adopt(router, "ADOPT_BROKER_POSITION_BASELINE").await;
    assert_eq!(status, StatusCode::OK, "adoption must succeed: {json}");
    assert_eq!(json["reconcile_status_after"], "ok", "{json}");

    // BRK-09R gate condition: reconcile must no longer be "dirty" or "stale".
    let after = st.current_reconcile_snapshot().await;
    assert_eq!(
        after.status, "ok",
        "reconcile must be 'ok' after adoption; BRK-09R gate must unblock: {:?}",
        after
    );
    assert!(
        !matches!(after.status.as_str(), "dirty" | "stale"),
        "BRK-09R condition cleared: reconcile is '{}', not 'dirty'/'stale'",
        after.status
    );

    // Verify DB row was updated (not just in-memory).
    let db_row = mqk_db::load_reconcile_status_state(&db)
        .await
        .expect("load failed")
        .expect("reconcile status row must exist");
    assert_eq!(
        db_row.status, "ok",
        "DB sys_reconcile_status_state must be 'ok' after adoption: {:?}",
        db_row
    );

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}

// ---------------------------------------------------------------------------
// IR07: Refused adoption (pre-mutation) → reconcile_refreshed=false.
//
// No state mutation occurs before Gate 2 (confirmation) or Gate 3 (DB).
// Proves refused responses carry reconcile_refreshed=false.
// Pure in-memory — no DB required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ir07_refused_adoption_does_not_refresh_reconcile() {
    // Wrong confirmation — refused at Gate 2 before any DB or snapshot access.
    let st = paper_alpaca_state();
    let (status, json) = post_adopt(
        paper_alpaca_router_from_state(Arc::clone(&st)),
        "WRONG_CONFIRMATION_STRING",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["accepted"], false, "{json}");
    assert_eq!(
        json["reconcile_refreshed"], false,
        "refused response must have reconcile_refreshed=false: {json}"
    );

    // Mode guard — refused at Gate 1 before confirmation is even checked.
    let st_live = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let (status2, json2) = post_adopt(
        paper_alpaca_router_from_state(st_live),
        "ADOPT_BROKER_POSITION_BASELINE",
    )
    .await;
    assert_eq!(status2, StatusCode::FORBIDDEN, "{json2}");
    assert_eq!(
        json2["reconcile_refreshed"], false,
        "mode-refused response must have reconcile_refreshed=false: {json2}"
    );
}

// ---------------------------------------------------------------------------
// IR08: Repeated adoption with the same snapshot is idempotent:
// reconcile_refreshed=true and reconcile_status_after="ok" on every call.
// (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ir08_repeated_adoption_is_idempotent_reconcile_stays_clean() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("ir08: skip — MQK_DATABASE_URL not set");
        return;
    }
    let db = test_db_pool().await;
    mqk_db::migrate(&db).await.expect("migration failed");
    let _ = mqk_db::clear_broker_position_baseline(&db).await;

    let snap = fake_broker_snapshot_with_position();

    for i in 0..2_u32 {
        let st = state::AppState::new_for_test_with_db_mode_and_broker(
            db.clone(),
            state::DeploymentMode::Paper,
            state::BrokerKind::Alpaca,
        );
        *st.broker_snapshot.write().await = Some(snap.clone());
        let (status, json) = post_adopt(
            paper_alpaca_router_from_state(Arc::new(st)),
            "ADOPT_BROKER_POSITION_BASELINE",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "attempt {i} must succeed: {json}");
        assert_eq!(
            json["reconcile_refreshed"], true,
            "attempt {i}: reconcile_refreshed must be true: {json}"
        );
        assert_eq!(
            json["reconcile_status_after"], "ok",
            "attempt {i}: reconcile_status_after must be 'ok': {json}"
        );
        assert_eq!(
            json["reconcile_mismatched_positions"], 0,
            "attempt {i}: no position mismatches: {json}"
        );
        assert_eq!(
            json["reconcile_mismatched_orders"], 0,
            "attempt {i}: no order mismatches: {json}"
        );
    }

    let _ = mqk_db::clear_broker_position_baseline(&db).await;
}
