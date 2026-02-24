//! Scenario: Outbox Claim/Lock Prevents Double Dispatch — Patch L3
//!
//! # Invariant under test
//! At most one dispatcher can claim a given outbox row at a time.
//!
//! `outbox_claim_batch` uses `FOR UPDATE SKIP LOCKED`, which means:
//! - The first caller atomically transitions matching PENDING rows to CLAIMED.
//! - Any concurrent caller finds no unlocked PENDING rows and gets an empty result.
//!
//! These tests simulate the two-dispatcher scenario synchronously:
//! Dispatcher A claims first, Dispatcher B finds nothing (skipped).
//! Only the claiming dispatcher can advance the row to SENT.
//!
//! All tests skip gracefully when `MQK_DATABASE_URL` is not set.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn make_pool(url: &str) -> anyhow::Result<sqlx::PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
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
            git_hash: "L3-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

// ---------------------------------------------------------------------------
// Test 1: only one dispatcher claims the row; the second gets nothing
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn only_one_dispatcher_claims_row_second_gets_empty() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = make_pool(&url).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_intent_double_dispatch");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "SPY", "qty": 1}),
    )
    .await?;

    // --- Dispatcher A claims the row ---
    let claimed_a = mqk_db::outbox_claim_batch(&pool, 10, "dispatcher-A").await?;
    assert_eq!(claimed_a.len(), 1, "dispatcher A must claim exactly 1 row");
    assert_eq!(claimed_a[0].status, "CLAIMED");
    assert_eq!(
        claimed_a[0].claimed_by.as_deref(),
        Some("dispatcher-A"),
        "claimed_by must record dispatcher identity"
    );

    // --- Dispatcher B tries to claim the same row — must get nothing ---
    // Because SKIP LOCKED skips rows held by A's implicit row lock.
    let claimed_b = mqk_db::outbox_claim_batch(&pool, 10, "dispatcher-B").await?;
    assert_eq!(
        claimed_b.len(),
        0,
        "dispatcher B must find no claimable rows while A holds the claim"
    );

    // --- Only dispatcher A can advance the row to SENT ---
    let sent = mqk_db::outbox_mark_sent(&pool, &intent_id).await?;
    assert!(sent, "dispatcher A must be able to mark SENT");

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("outbox row must exist");
    assert_eq!(row.status, "SENT");

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2: releasing a claim returns the row to PENDING for another dispatcher
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn release_claim_returns_row_to_pending_for_next_dispatcher() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = make_pool(&url).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_intent_release_test");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "AAPL", "qty": 5}),
    )
    .await?;

    // Dispatcher A claims the row.
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-A").await?;
    assert_eq!(claimed.len(), 1);

    // Dispatcher A fails and releases its claim.
    let released = mqk_db::outbox_release_claim(&pool, &intent_id).await?;
    assert!(released, "release_claim must succeed when row is CLAIMED");

    // Row is back to PENDING with no claim metadata.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.status, "PENDING", "released row must return to PENDING");
    assert!(
        row.claimed_by.is_none(),
        "claimed_by must be cleared on release"
    );
    assert!(
        row.claimed_at_utc.is_none(),
        "claimed_at_utc must be cleared on release"
    );

    // Dispatcher B can now claim the released row.
    let claimed_b = mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-B").await?;
    assert_eq!(
        claimed_b.len(),
        1,
        "dispatcher B must claim the released row"
    );
    assert_eq!(claimed_b[0].claimed_by.as_deref(), Some("dispatcher-B"));

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 3: unclaimed row cannot be marked SENT (enforces CLAIMED → SENT)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn unclaimed_row_cannot_be_marked_sent() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = make_pool(&url).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_intent_noclaim_test");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "QQQ", "qty": 10}),
    )
    .await?;

    // Attempt to mark SENT directly — no claim step.
    let sent = mqk_db::outbox_mark_sent(&pool, &intent_id).await?;
    assert!(
        !sent,
        "mark_sent must return false if the row was never claimed"
    );

    // Row must remain PENDING.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("outbox row must exist");
    assert_eq!(
        row.status, "PENDING",
        "row must remain PENDING after a failed mark_sent attempt"
    );

    Ok(())
}
