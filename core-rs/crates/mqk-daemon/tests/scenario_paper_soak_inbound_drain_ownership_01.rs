//! PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01
//!
//! # Problem addressed
//!
//! `stop_execution_runtime` used to clear local ownership unconditionally.
//! Because the WS inbound transport (`alpaca_ws_transport.rs`) gates durable
//! ingest on "is a run currently locally owned" (`active_owned_run_id()`),
//! clearing ownership the instant stop is requested meant any late broker
//! event for a run with unresolved orders was silently dropped -- zero
//! logging, zero durable trace.
//!
//! # Coverage
//!
//! | Test | Claim                                                                 |
//! |------|-------------------------------------------------------------------------|
//! | D01  | Stop with no broker-reachable orders succeeds immediately (unchanged)   |
//! | D02  | Stop with one unresolved order returns a typed conflict; ownership held |
//! | D03  | Stop succeeds once the order resolves to a terminal outcome             |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres), using `AppState::new_for_test_with_db_mode_and_broker`
//! and `establish_db_backed_active_run_for_test` -- no real HTTP router, no
//! real broker connection, no mock Alpaca server. This directly exercises
//! the `stop_execution_runtime` method under test against a real DB. This is
//! a load-bearing institutional proof test -- it must fail hard if
//! `MQK_DATABASE_URL` is absent, not skip.

use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use mqk_daemon::state;
use state::{AppState, BrokerKind, DeploymentMode};

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn require_pool(url: &str) -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("PROOF: cannot connect to DB: {e}"))
}

async fn cleanup(pool: &PgPool, run_id: Uuid) {
    let _ = sqlx::query(
        "delete from broker_order_map where internal_id in \
         (select idempotency_key from oms_outbox where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

fn make_state(pool: PgPool) -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// D01 — stop with no broker-reachable orders succeeds immediately.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d01_stop_with_no_open_orders_succeeds_immediately() {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let run_id = Uuid::new_v4();
    cleanup(&pool, run_id).await;

    let st = make_state(pool.clone());
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("D01: establishing active run must succeed");

    let snapshot = st
        .stop_execution_runtime()
        .await
        .expect("D01: stop with no open orders must succeed (unchanged behavior)");
    assert_eq!(
        snapshot.state, "idle",
        "D01: state must be idle after a clean stop"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Stopped),
        "D01: run must reach STOPPED"
    );

    cleanup(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// D02 — stop with one unresolved broker-reachable order is refused, typed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d02_stop_with_unresolved_order_returns_typed_conflict_and_preserves_ownership() {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let run_id = Uuid::new_v4();
    let idem = format!("d02-order-{run_id}");
    cleanup(&pool, run_id).await;

    let st = make_state(pool.clone());
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("D02: establishing active run must succeed");

    // An order reached the broker (ACKED) but never resolved to a terminal
    // outcome -- exactly the scenario that must block a clean stop.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await
    .expect("outbox_enqueue failed");
    sqlx::query("update oms_outbox set status = 'ACKED' where idempotency_key = $1")
        .bind(&idem)
        .execute(&pool)
        .await
        .expect("mark ACKED failed");

    let err = st
        .stop_execution_runtime()
        .await
        .expect_err("D02: stop must be refused while an order is unresolved");
    assert_eq!(
        err.fault_class(),
        "runtime.stop.orders_draining",
        "D02: refusal must be the typed drain conflict, not a generic error"
    );
    assert!(
        err.to_string().contains(&idem),
        "D02: the conflict message must identify the unresolved order, got: {err}"
    );

    // Ownership must NOT have been cleared -- this is the entire point: the
    // WS inbound transport's active_owned_run_id() gate must still see this
    // run as owned, so a late frame is durably ingested instead of dropped.
    let owned = st.locally_owned_run_id().await;
    assert_eq!(
        owned,
        Some(run_id),
        "D02: local ownership must be preserved across a refused stop"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "D02: run must remain RUNNING, not move to STOPPED on a refused stop"
    );
    assert!(
        run.stop_requested_at_utc.is_some(),
        "D02: stop_requested_at_utc must be durably set so Phase 1 stays suppressed \
         even across a daemon restart"
    );

    cleanup(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// D03 — stop succeeds once the order resolves to a terminal outcome.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d03_stop_succeeds_once_order_resolves() {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let run_id = Uuid::new_v4();
    let idem = format!("d03-order-{run_id}");
    cleanup(&pool, run_id).await;

    let st = make_state(pool.clone());
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("D03: establishing active run must succeed");

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await
    .expect("outbox_enqueue failed");
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(&idem)
        .execute(&pool)
        .await
        .expect("mark SENT failed");

    // First stop attempt: refused, exactly like D02.
    let first = st.stop_execution_runtime().await;
    assert!(
        first.is_err(),
        "D03 precondition: first stop attempt must be refused while unresolved"
    );

    // The order now resolves: a late terminal fill is durably ingested --
    // this is the durable evidence that would have arrived via the WS
    // transport while ownership was preserved.
    let fill_msg_id = "alpaca:d03-broker-order:fill:2026-08-08T14:00:00.000000Z".to_string();
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &fill_msg_id,
        None,
        &idem,
        "broker-d03-order",
        "fill",
        &json!({
            "type":               "fill",
            "broker_message_id":  fill_msg_id,
            "broker_fill_id":     serde_json::Value::Null,
            "internal_order_id":  idem,
            "broker_order_id":    "broker-d03-order",
            "symbol":             "SPY",
            "side":               "Buy",
            "delta_qty":          1_i64,
            "price_micros":       500_000_000_i64,
            "fee_micros":         0_i64
        }),
        0,
        Utc::now(),
    )
    .await
    .expect("late fill insert failed");

    // Second stop attempt: every broker-reachable order is now resolved.
    let snapshot = st
        .stop_execution_runtime()
        .await
        .expect("D03: stop must succeed once the order has resolved");
    assert_eq!(
        snapshot.state, "idle",
        "D03: state must be idle after the drained stop"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Stopped),
        "D03: run must reach STOPPED once drained"
    );

    let owned = st.locally_owned_run_id().await;
    assert_eq!(
        owned, None,
        "D03: local ownership must be cleared once the drained stop completes"
    );

    cleanup(&pool, run_id).await;
}
