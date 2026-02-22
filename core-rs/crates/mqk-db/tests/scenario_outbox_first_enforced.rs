//! Scenario: Outbox-First Protocol — Patch L2
//!
//! # Invariant under test
//! An outbox row with status PENDING exists in the DB *before* any broker
//! submit call is made.  If the engine crashes between enqueue and submit,
//! the pending row is discoverable at restart and can be replayed exactly
//! once (via the recovery path tested in `scenario_crash_recovery_no_double_order`).
//!
//! Both tests skip gracefully when `MQK_DATABASE_URL` is not set, making
//! them CI-friendly even without a live Postgres instance.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test 1: outbox row is PENDING before broker submit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outbox_row_is_pending_before_broker_submit() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "L2-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"arming": {}}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

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

    // --- Step 3: Dispatcher claims the row (PENDING → CLAIMED) ---
    // In production, the dispatcher calls outbox_claim_batch before submitting
    // to the broker. This is the L3 two-step protocol.
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "test-dispatcher").await?;
    assert_eq!(claimed.len(), 1, "dispatcher must claim exactly one row");
    assert_eq!(claimed[0].status, "CLAIMED");

    // --- Step 4: Simulate broker submit (advance status to SENT) ---
    // In production the dispatcher calls the broker adapter *after* claiming,
    // then marks SENT.  Here we skip the actual broker call.
    let marked = mqk_db::outbox_mark_sent(&pool, &client_order_id).await?;
    assert!(marked, "outbox_mark_sent must succeed");

    // --- Step 5: Confirm final SENT status ---
    let row2 = mqk_db::outbox_fetch_by_idempotency_key(&pool, &client_order_id)
        .await?
        .expect("outbox row must still exist after marking SENT");
    assert_eq!(
        row2.status, "SENT",
        "outbox row must be SENT after broker submit"
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
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
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

    let intent_id = format!("{run_id}_intent_retry_test");
    let order_json = json!({"symbol": "AAPL", "side": "BUY", "qty": 50});

    // First enqueue — must create the row.
    let created1 = mqk_db::outbox_enqueue(&pool, run_id, &intent_id, order_json.clone()).await?;
    assert!(created1, "first enqueue must create row");

    // Retry with the SAME intent_id — must NOT create a second row.
    let created2 = mqk_db::outbox_enqueue(&pool, run_id, &intent_id, order_json.clone()).await?;
    assert!(!created2, "retry enqueue must not create a second row");

    // Exactly one row exists for this run.
    let rows = mqk_db::outbox_list_unacked_for_run(&pool, run_id).await?;
    assert_eq!(
        rows.len(),
        1,
        "exactly one outbox row must exist after retry"
    );
    assert_eq!(rows[0].idempotency_key, intent_id);

    Ok(())
}
