//! Scenario: Outbox-First Protocol — Patch L2
//!
//! # Invariant under test
//! An outbox row with status PENDING exists in the DB *before* any broker
//! submit call is made. If the engine crashes between enqueue and submit,
//! the pending row is discoverable at restart and can be replayed exactly
//! once (via the recovery path tested in `scenario_crash_recovery_no_double_order`).
//!
//! Both tests skip gracefully when `MQK_DATABASE_URL` is not set, making
//! them CI-friendly even without a live Postgres instance.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_pool(url: &str) -> anyhow::Result<sqlx::PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

async fn make_run(pool: &sqlx::PgPool) -> anyhow::Result<uuid::Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "L2-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

/// Test isolation helper.
///
/// `outbox_claim_batch()` is not scoped by run_id in these tests, so any
/// leftover PENDING / CLAIMED rows from prior DB tests can be claimed first
/// and break deterministic assertions. Wipe the shared outbox surface before
/// each test in this file.
async fn cleanup_outbox_tables(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::query("delete from broker_order_map")
        .execute(pool)
        .await?;
    sqlx::query("delete from oms_outbox").execute(pool).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Test 1: outbox row is PENDING before broker submit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outbox_row_is_pending_before_broker_submit() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "PROOF: MQK_DATABASE_URL is not set. 
             This is a load-bearing proof test and cannot be skipped. 
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
            );
        }
    };

    let pool = make_pool(&url).await?;
    cleanup_outbox_tables(&pool).await?;
    let run_id = make_run(&pool).await?;

    // The client_order_id is derived from the intent_id (pass-through).
    // In production this is done by `mqk_execution::intent_id_to_client_order_id`.
    let intent_id = format!("{run_id}_intent_buy_SPY_100");
    let client_order_id = intent_id.clone();

    // --- Step 1: Outbox enqueue BEFORE any broker call ---
    let created = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &client_order_id,
        json!({"symbol": "SPY", "side": "BUY", "qty": 100}),
    )
    .await?;
    assert!(
        created,
        "outbox_enqueue must create a new row on first call"
    );

    // --- Step 2: Verify PENDING row exists (broker not yet called) ---
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &client_order_id)
        .await?
        .expect("outbox row must exist after enqueue");
    assert_eq!(
        row.status, "PENDING",
        "outbox row must be PENDING before broker submit"
    );
    assert_eq!(
        row.idempotency_key, client_order_id,
        "fetched row must match the inserted idempotency key"
    );

    // --- Step 3: Dispatcher claims the row (PENDING -> CLAIMED) ---
    //
    // Because we cleaned the shared outbox tables first, the single claimable
    // row should be the one created by this test.
    let claimed =
        mqk_db::outbox_claim_batch(&pool, 1, "test-dispatcher", chrono::Utc::now()).await?;
    assert_eq!(claimed.len(), 1, "dispatcher must claim exactly one row");
    assert_eq!(claimed[0].row.idempotency_key, client_order_id);
    assert_eq!(claimed[0].row.status, "CLAIMED");

    // --- Step 4: Simulate broker submit (advance status to SENT) ---
    //
    // In production the dispatcher calls the broker adapter *after* claiming,
    // then marks SENT. Here we skip the actual broker call.
    let marked = mqk_db::outbox_mark_sent_with_broker_map(
        &pool,
        &client_order_id,
        "test-broker-id",
        chrono::Utc::now(),
    )
    .await?;
    assert!(
        marked,
        "outbox_mark_sent must succeed after the row has been CLAIMED"
    );

    // --- Step 5: Confirm final SENT status ---
    let row2 = mqk_db::outbox_fetch_by_idempotency_key(&pool, &client_order_id)
        .await?
        .expect("outbox row must still exist after marking SENT");
    assert_eq!(
        row2.status, "SENT",
        "outbox row must be SENT after broker submit"
    );
    assert_eq!(
        row2.idempotency_key, client_order_id,
        "final row must still match the inserted idempotency key"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2: retry enqueue on same intent_id does NOT create a second outbox row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_enqueue_does_not_create_second_outbox_row() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "PROOF: MQK_DATABASE_URL is not set. 
             This is a load-bearing proof test and cannot be skipped. 
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
            );
        }
    };

    let pool = make_pool(&url).await?;
    cleanup_outbox_tables(&pool).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_intent_retry_test");
    let order_json = json!({"symbol": "AAPL", "side": "BUY", "qty": 50});

    // First enqueue — must create the row.
    let created1 = mqk_db::outbox_enqueue(&pool, run_id, &intent_id, order_json.clone()).await?;
    assert!(created1, "first enqueue must create row");

    // Retry with the SAME intent_id — must NOT create a second row.
    let created2 = mqk_db::outbox_enqueue(&pool, run_id, &intent_id, order_json.clone()).await?;
    assert!(!created2, "retry enqueue must not create a second row");

    // Exactly one row exists for this run.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("outbox row must exist after retry");
    assert_eq!(row.idempotency_key, intent_id);
    assert_eq!(
        row.status, "PENDING",
        "retry enqueue must not mutate the original row out of PENDING"
    );

    Ok(())
}
