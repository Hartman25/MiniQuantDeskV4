//! BRK-GAP-REST-AUTO-TRIGGER-01 — Startup WS gap fill auto-recovery proof.
//!
//! Proves `AppState::try_ws_gap_auto_recovery` is fail-closed, only fires on
//! proven GapDetected conditions, and delegates to the same service function
//! used by the manual repair route.
//!
//! ## Invariants under test
//!
//! | Test | Claim                                                                           | DB? |
//! |------|---------------------------------------------------------------------------------|-----|
//! | A01  | Non-Alpaca broker → returns None (auto-trigger does not apply)                  | No  |
//! | A02  | Alpaca + ColdStartUnproven (no gap) → returns None                              | No  |
//! | A03  | Alpaca + GapDetected + no fetcher → returns None (fail-closed)                  | No  |
//! | A04  | Alpaca + GapDetected + fetcher + no DB → returns None (fail-closed)             | No  |
//! | A05  | GapDetected + fetcher + DB + known order → inserts one inbox row                | Yes |
//! | A06  | Second auto-trigger with same fill → idempotent (already_present=1, no dup)    | Yes |
//! | A07  | Unknown broker order REST fill → unknown_order_count=1, cursor not advanced     | Yes |
//! | A08  | Malformed fill (missing price) → malformed_count=1, cursor not advanced         | Yes |
//! | A09  | Mixed safe+unsafe batch → no cursor advance                                     | Yes |
//! | A10  | Halted latest run → returns None (manual repair required)                       | Yes |
//!
//! A01–A04 are pure in-process (no DB required, always pass).
//! A05–A10 require `MQK_DATABASE_URL` and skip gracefully without it.

use std::sync::Arc;

use mqk_broker_alpaca::types::AlpacaOrderActivity;
use mqk_daemon::state::{self, AlpacaWsContinuityState, BrokerKind, DeploymentMode};

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

    fn empty() -> Arc<Self> {
        Arc::new(Self {
            activities: vec![],
            error: None,
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

fn make_fill(activity_id: &str, order_id: &str, symbol: &str, side: &str) -> AlpacaOrderActivity {
    AlpacaOrderActivity {
        id: activity_id.to_owned(),
        activity_type: "FILL".to_owned(),
        trade_type: Some("fill".to_owned()),
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
    let mut a = make_fill(activity_id, order_id, "AAPL", "buy");
    a.price = None;
    a
}

// ---------------------------------------------------------------------------
// DB test constants
// ---------------------------------------------------------------------------

const TEST_ENGINE_ID: &str = "mqk-daemon";
const TEST_RUN_ID: &str = "a0050002-0000-0000-0000-000000000001";
const TEST_BROKER_ORDER_ID: &str = "broker-auto-gap-order-001";
const TEST_INTERNAL_ORDER_ID: &str = "internal-auto-gap-order-001";
const TEST_ACTIVITY_ID: &str = "20260509143000000000002::a0b0c0d0e0f0a0b1";
const TEST_ACTIVITY_ID_2: &str = "20260509143000000000003::a0b0c0d0e0f0a0b2";
const TEST_UNKNOWN_BROKER_ID: &str = "broker-not-in-map-001";

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn setup_run_with_order(pool: &sqlx::PgPool, run_status: &str) {
    use chrono::Utc;
    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let now = Utc::now();

    sqlx::query(
        "insert into runs (run_id, engine_id, mode, started_at_utc,
                            git_hash, config_hash, config_json, host_fingerprint)
         values ($1, $2, 'PAPER', $3, 'test-hash',
                 'test-cfg-hash', '{\"src\":\"brk-gap-auto-trigger-01\"}'::jsonb, 'test-host')
         on conflict (run_id) do nothing",
    )
    .bind(run_id)
    .bind(TEST_ENGINE_ID)
    .bind(now)
    .execute(pool)
    .await
    .expect("insert run");

    // Force the run to the desired status.
    sqlx::query("update runs set status = $1 where run_id = $2")
        .bind(run_status)
        .bind(run_id)
        .execute(pool)
        .await
        .expect("update run status");

    let order_json = serde_json::json!({
        "order_id": TEST_INTERNAL_ORDER_ID,
        "symbol": "AAPL",
        "side": "buy",
        "quantity": 5,
        "order_type": "limit",
        "limit_price": 150_000_000_i64,
    });
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc)
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

async fn db_pool_from_env() -> Option<sqlx::PgPool> {
    let url = std::env::var(mqk_db::ENV_DB_URL).ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DB connect failed");
    Some(pool)
}

async fn build_gap_state_with_fetcher(
    pool: sqlx::PgPool,
    fetcher: Arc<dyn state::WsGapFillFetcher>,
) -> Arc<state::AppState> {
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_ws_gap_fill_fetcher_for_test(fetcher);
    let st = Arc::new(st);
    // Set continuity to GapDetected (the proven trigger condition).
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("msg-gap-test".to_string()),
        last_event_at: Some("2026-05-09T14:00:00Z".to_string()),
        detail: "test gap".to_string(),
    })
    .await;
    st
}

// ---------------------------------------------------------------------------
// A01 — Non-Alpaca broker → auto-trigger does not apply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a01_non_alpaca_returns_none() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    // Continuity is NotApplicable for non-Alpaca.
    let result = st.try_ws_gap_auto_recovery().await;
    assert!(result.is_none(), "A01: non-Alpaca must return None");
}

// ---------------------------------------------------------------------------
// A02 — ColdStartUnproven (no gap condition) → does not fire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a02_cold_start_unproven_returns_none() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    // Default for Alpaca is ColdStartUnproven — not a gap trigger.
    let result = st.try_ws_gap_auto_recovery().await;
    assert!(result.is_none(), "A02: ColdStartUnproven must return None");
}

// ---------------------------------------------------------------------------
// A03 — GapDetected + no fetcher → returns None (fail-closed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a03_gap_detected_no_fetcher_returns_none() {
    let mut st = state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    // Ensure no fetcher (in case env credentials are present).
    st.ws_gap_fill_fetcher = None;
    let st = Arc::new(st);
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: None,
        last_event_at: None,
        detail: "test gap".to_string(),
    })
    .await;
    let result = st.try_ws_gap_auto_recovery().await;
    assert!(
        result.is_none(),
        "A03: GapDetected + no fetcher must return None"
    );
}

// ---------------------------------------------------------------------------
// A04 — GapDetected + fetcher + no DB → returns None (fail-closed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a04_gap_detected_no_db_returns_none() {
    let mut st = state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_ws_gap_fill_fetcher_for_test(FakeGapFetcher::empty());
    // DB is None (no_db constructor).
    assert!(st.db.is_none(), "A04: test requires no-DB state");
    let st = Arc::new(st);
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: None,
        last_event_at: None,
        detail: "test gap".to_string(),
    })
    .await;
    let result = st.try_ws_gap_auto_recovery().await;
    assert!(
        result.is_none(),
        "A04: GapDetected + no DB must return None"
    );
}

// ---------------------------------------------------------------------------
// A05 — GapDetected + fetcher + DB + known order → inserts one inbox row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a05_gap_with_known_order_inserts_inbox_row() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A05: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool, "RUNNING").await;

    let activity = make_fill(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let st =
        build_gap_state_with_fetcher(pool.clone(), FakeGapFetcher::returning(vec![activity])).await;

    let result = st.try_ws_gap_auto_recovery().await;
    let outcome = result.expect("A05: must return Some with GapDetected + fetcher + DB");

    assert!(
        outcome.gate.is_none(),
        "A05: no gate on success; gate={:?}",
        outcome.gate
    );
    assert_eq!(outcome.recovered_count, 1, "A05: one fill recovered");
    assert_eq!(outcome.already_present_count, 0, "A05: not already present");
    assert_eq!(outcome.unknown_order_count, 0, "A05: no unknown orders");
    assert_eq!(outcome.malformed_count, 0, "A05: no malformed");
    assert!(!outcome.dry_run, "A05: dry_run must be false");

    // Verify the inbox row exists in DB.
    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(rows.len(), 1, "A05: one inbox row in DB");
    let expected_msg_id = format!("alpaca-rest-gap:{TEST_ACTIVITY_ID}");
    assert_eq!(
        rows[0].broker_message_id, expected_msg_id,
        "A05: stable message_id"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// A06 — Repeated auto-trigger with same fill → idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a06_repeated_auto_trigger_is_idempotent() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A06: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool, "RUNNING").await;

    let activity = make_fill(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");

    // First trigger.
    let st1 = build_gap_state_with_fetcher(
        pool.clone(),
        FakeGapFetcher::returning(vec![activity.clone()]),
    )
    .await;
    let r1 = st1
        .try_ws_gap_auto_recovery()
        .await
        .expect("A06: first trigger must return Some");
    assert_eq!(r1.recovered_count, 1, "A06: first trigger inserts one");

    // Second trigger — same activity, same run.
    let st2 =
        build_gap_state_with_fetcher(pool.clone(), FakeGapFetcher::returning(vec![activity])).await;
    let r2 = st2
        .try_ws_gap_auto_recovery()
        .await
        .expect("A06: second trigger must return Some");
    assert_eq!(
        r2.recovered_count, 0,
        "A06: second trigger inserts zero (idempotent)"
    );
    assert_eq!(
        r2.already_present_count, 1,
        "A06: already_present=1 on second trigger"
    );
    assert!(r2.gate.is_none(), "A06: no gate on idempotent run");

    // Still only one inbox row in DB.
    let run_id = uuid::Uuid::parse_str(TEST_RUN_ID).unwrap();
    let rows = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox query");
    assert_eq!(
        rows.len(),
        1,
        "A06: still exactly one inbox row after two triggers"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// A07 — Unknown broker order → unknown_order_count=1, cursor not advanced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a07_unknown_broker_order_not_inserted_cursor_not_advanced() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A07: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool, "RUNNING").await;

    // Activity references an order_id that is NOT in broker_order_map.
    let activity = make_fill(TEST_ACTIVITY_ID, TEST_UNKNOWN_BROKER_ID, "AAPL", "buy");
    let st =
        build_gap_state_with_fetcher(pool.clone(), FakeGapFetcher::returning(vec![activity])).await;

    let outcome = st
        .try_ws_gap_auto_recovery()
        .await
        .expect("A07: must return Some");

    assert_eq!(outcome.unknown_order_count, 1, "A07: one unknown order");
    assert_eq!(outcome.recovered_count, 0, "A07: no recovered fills");
    assert!(
        !outcome.cursor_advanced,
        "A07: cursor must not advance on unknown order"
    );
    assert!(
        outcome.gate.is_none(),
        "A07: no gate (processing completed)"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// A08 — Malformed fill (missing price) → malformed_count=1, cursor not advanced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a08_malformed_fill_not_inserted_cursor_not_advanced() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A08: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool, "RUNNING").await;

    let activity = make_fill_no_price(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID);
    let st =
        build_gap_state_with_fetcher(pool.clone(), FakeGapFetcher::returning(vec![activity])).await;

    let outcome = st
        .try_ws_gap_auto_recovery()
        .await
        .expect("A08: must return Some");

    assert_eq!(outcome.malformed_count, 1, "A08: one malformed");
    assert_eq!(outcome.recovered_count, 0, "A08: no recovered fills");
    assert!(
        !outcome.cursor_advanced,
        "A08: cursor must not advance on malformed"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// A09 — Mixed safe+unsafe batch → cursor not advanced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a09_mixed_batch_cursor_not_advanced() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A09: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    setup_run_with_order(&pool, "RUNNING").await;

    // One safe fill + one unknown order fill in the same batch.
    let safe_fill = make_fill(TEST_ACTIVITY_ID, TEST_BROKER_ORDER_ID, "AAPL", "buy");
    let unsafe_fill = make_fill(TEST_ACTIVITY_ID_2, TEST_UNKNOWN_BROKER_ID, "AAPL", "buy");
    let st = build_gap_state_with_fetcher(
        pool.clone(),
        FakeGapFetcher::returning(vec![safe_fill, unsafe_fill]),
    )
    .await;

    let outcome = st
        .try_ws_gap_auto_recovery()
        .await
        .expect("A09: must return Some");

    assert_eq!(outcome.recovered_count, 1, "A09: one safe fill recovered");
    assert_eq!(outcome.unknown_order_count, 1, "A09: one unknown order");
    assert!(
        !outcome.cursor_advanced,
        "A09: cursor must NOT advance on mixed batch"
    );

    cleanup_run(&pool).await;
}

// ---------------------------------------------------------------------------
// A10 — Halted latest run → returns None (manual repair path)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a10_halted_run_returns_none() {
    let Some(pool) = db_pool_from_env().await else {
        eprintln!("A10: skipped — MQK_DATABASE_URL not set");
        return;
    };
    cleanup_run(&pool).await;
    // Insert the run as HALTED.
    setup_run_with_order(&pool, "HALTED").await;

    let st = build_gap_state_with_fetcher(pool.clone(), FakeGapFetcher::empty()).await;

    let result = st.try_ws_gap_auto_recovery().await;
    assert!(
        result.is_none(),
        "A10: halted run must return None (manual repair required)"
    );

    cleanup_run(&pool).await;
}
