//! Scenario: Execution Crash Windows — EB-5
//!
//! # Invariants under test
//!
//! The outbox dispatch protocol (claim → submit → mark_sent → mark_acked)
//! has two windows where a process crash leaves the DB in a state that
//! could, naïvely, produce a double-submit on restart.
//!
//! This file proves the recovery path eliminates double-submit across
//! both crash boundaries.
//!
//! ## Crash Window W1 — after SENT, before ACK persisted
//!
//! Normal path:   claim → submit_to_broker → mark_sent → [receive ACK] → mark_acked
//! Crash at:      ^— after mark_sent, process exits before mark_acked
//! DB state:      outbox = SENT, broker has the order
//! Recovery:      broker has order → mark_acked locally, do NOT resubmit
//! Invariant:     broker.submit_count() == 1 after recovery
//!
//! ## Crash Window W2 — after CLAIMED, before broker submit
//!
//! Normal path:   claim → [send to broker] → mark_sent → ...
//! Crash at:      ^— after claim, process exits before broker submit
//! DB state:      outbox = CLAIMED, broker does NOT have the order
//! Recovery:      broker missing → submit exactly once → mark_acked
//! Invariant:     broker.submit_count() == 1 after recovery (no zero, no two)
//!
//! ## Scenario W3 — ACKED row not reinspected on second restart
//!
//! After a recovery pass marks a row ACKED, a subsequent call to
//! outbox_list_unacked_for_run must not return it.  A second restart
//! therefore inspects zero rows and makes zero broker calls.
//!
//! Requires `MQK_DATABASE_URL`. Skips with a diagnostic message if absent
//! or misconfigured.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixed run UUIDs — deterministic, never collide with production runs.
// ---------------------------------------------------------------------------

const W1_RUN_ID: &str = "eb5b0001-0000-0000-0000-000000000000";
const W2_RUN_ID: &str = "eb5b0002-0000-0000-0000-000000000000";
const W3_RUN_ID: &str = "eb5b0003-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: requires MQK_DATABASE_URL");
            None
        }
    }
}

async fn try_pool_or_skip(url: &str) -> Result<Option<PgPool>> {
    match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
    {
        Ok(pool) => Ok(Some(pool)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

/// Insert a minimal test run and a single outbox entry.
/// Returns the idempotency key used for the outbox row.
async fn seed_run_and_outbox(pool: &PgPool, run_id: Uuid, idem_key: &str) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "eb5-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "eb5-test".to_string(),
            config_hash: "eb5-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "eb5-test".to_string(),
        },
    )
    .await?;

    mqk_db::outbox_enqueue(pool, run_id, idem_key, json!({"symbol":"SPY","qty":1})).await?;

    Ok(())
}

/// Remove test data for the given run (cascades oms_outbox from runs delete).
/// broker_order_map rows must already be gone before calling this.
async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// W1: Crash after SENT, before ACK — no resubmit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w1_crash_after_sent_before_ack_no_double_submit() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W1_RUN_ID.parse().unwrap();
    let key = "eb5-w1-ord-001";

    // Pre-test cleanup: run delete cascades to outbox.
    cleanup_run(&pool, run_id).await?;

    // Seed run + outbox entry (PENDING).
    seed_run_and_outbox(&pool, run_id, key).await?;

    // --- Simulate pre-crash dispatch ---

    // Dispatcher claims the row.
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "eb5-dispatcher").await?;
    assert_eq!(claimed.len(), 1, "must claim the PENDING row");

    // Broker stub: submit the order. Broker now has it.
    let mut broker = mqk_testkit::FakeBroker::new();
    broker.submit(key, json!({"symbol":"SPY","qty":1}));
    assert_eq!(
        broker.submit_count(),
        1,
        "broker must record exactly one submit"
    );

    // Mark outbox SENT to record the dispatch attempt.
    let sent = mqk_db::outbox_mark_sent(&pool, key).await?;
    assert!(sent, "outbox_mark_sent must transition CLAIMED → SENT");

    // --- CRASH: process exits here, mark_acked never called ---
    // DB state: outbox = SENT, broker has the order.

    // --- Restart: run recovery ---
    let report = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;

    assert_eq!(
        report.inspected, 1,
        "W1: recovery must inspect the SENT row"
    );
    assert_eq!(
        report.resubmitted, 0,
        "W1: must NOT resubmit — broker already has the order"
    );
    assert_eq!(
        report.acked, 1,
        "W1: must mark ACKED when broker already has the order"
    );
    assert_eq!(
        broker.submit_count(),
        1,
        "W1: broker must have received exactly one submit total (no double-submit)"
    );

    // DB must now show ACKED.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, key).await?;
    assert_eq!(
        row.expect("row must exist").status,
        "ACKED",
        "W1: outbox row must be ACKED after recovery"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// W2: Crash after CLAIMED, before broker submit — resubmit exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w2_crash_after_claimed_before_sent_resubmits_exactly_once() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W2_RUN_ID.parse().unwrap();
    let key = "eb5-w2-ord-001";

    cleanup_run(&pool, run_id).await?;
    seed_run_and_outbox(&pool, run_id, key).await?;

    // --- Simulate pre-crash dispatch ---

    // Dispatcher claims the row but crashes before submitting to broker.
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "eb5-dispatcher").await?;
    assert_eq!(claimed.len(), 1, "must claim the PENDING row");

    // --- CRASH: process exits here, broker submit never happened ---
    // DB state: outbox = CLAIMED, broker does NOT have the order.

    // --- Restart: run recovery ---
    let mut broker = mqk_testkit::FakeBroker::new();
    assert_eq!(
        broker.submit_count(),
        0,
        "W2: broker must start with zero submits on restart"
    );

    let report = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;

    assert_eq!(
        report.inspected, 1,
        "W2: recovery must inspect the CLAIMED row"
    );
    assert_eq!(
        report.resubmitted, 1,
        "W2: must resubmit exactly once — broker did not have the order"
    );
    assert_eq!(report.acked, 1, "W2: must mark ACKED after resubmit");
    assert_eq!(
        broker.submit_count(),
        1,
        "W2: broker must have received exactly one submit (no zero, no two)"
    );
    assert!(
        broker.has_order(key),
        "W2: broker must have the order after recovery resubmit"
    );

    // DB must now show ACKED.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, key).await?;
    assert_eq!(
        row.expect("row must exist").status,
        "ACKED",
        "W2: outbox row must be ACKED after recovery"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// W3: ACKED row not reinspected on second restart
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w3_acked_row_not_reinspected_on_second_restart() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W3_RUN_ID.parse().unwrap();
    let key = "eb5-w3-ord-001";

    cleanup_run(&pool, run_id).await?;
    seed_run_and_outbox(&pool, run_id, key).await?;

    // --- First restart: simulate a W1-style crash recovery (SENT → ACKED) ---
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "eb5-dispatcher").await?;
    assert_eq!(claimed.len(), 1);

    let mut broker = mqk_testkit::FakeBroker::new();
    broker.submit(key, json!({"symbol":"SPY","qty":1}));
    mqk_db::outbox_mark_sent(&pool, key).await?;

    // First recovery: marks the row ACKED.
    let first = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;
    assert_eq!(first.acked, 1, "first recovery must mark ACKED");
    assert_eq!(broker.submit_count(), 1, "one submit after first recovery");

    // --- Second restart: verify ACKED row is not reinspected ---
    // A second recovery pass (another daemon restart) must see zero unacked rows.
    let second = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;

    assert_eq!(
        second.inspected, 0,
        "W3: second recovery must inspect zero rows — ACKED is terminal"
    );
    assert_eq!(
        second.resubmitted, 0,
        "W3: second recovery must not resubmit anything"
    );
    assert_eq!(
        broker.submit_count(),
        1,
        "W3: broker submit count must remain at 1 across both recovery passes"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}
