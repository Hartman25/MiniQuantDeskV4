//! Scenario: Stale Claim Recovery — FC-6
//!
//! # Invariant under test
//!
//! `outbox_reset_stale_claims(pool, stale_threshold)` is the crash-recovery
//! reaper that resets CLAIMED rows left behind by a crashed dispatcher.
//!
//! Rules enforced:
//!   1. A CLAIMED row whose `claimed_at_utc < stale_threshold` is reset to
//!      PENDING with `claimed_at_utc = NULL` and `claimed_by = NULL`.
//!   2. A CLAIMED row whose `claimed_at_utc >= stale_threshold` is NOT
//!      touched (dispatcher still has a live claim window).
//!   3. Terminal-state rows (`SENT`, `ACKED`, `FAILED`) are never reset,
//!      even when the threshold covers them.
//!
//! All tests skip gracefully when `MQK_DATABASE_URL` is not set.

use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
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
            git_hash: "FC6-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db -- --include-ignored"
        ),
    }
}

// ---------------------------------------------------------------------------
// Test 1: CLAIMED row older than threshold is reset to PENDING
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn stale_claim_older_than_threshold_reset_to_pending() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_stale_claim_reset");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "SPY", "qty": 1}),
    )
    .await?;

    // Claim the row at a time well in the past.
    let claimed_at_past = Utc::now() - Duration::minutes(10);
    let claimed =
        mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-crashed", claimed_at_past).await?;
    assert_eq!(claimed.len(), 1);

    // Reaper threshold: anything claimed more than 5 minutes ago is stale.
    let threshold = Utc::now() - Duration::minutes(5);
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;
    assert_eq!(
        reset_count, 1,
        "exactly one stale CLAIMED row must be reset"
    );

    // Row is back to PENDING with claim metadata cleared.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.status, "PENDING", "reset row must be PENDING");
    assert!(
        row.claimed_by.is_none(),
        "claimed_by must be NULL after reset"
    );
    assert!(
        row.claimed_at_utc.is_none(),
        "claimed_at_utc must be NULL after reset"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2: fresh CLAIMED row (newer than threshold) is NOT reset
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn fresh_claim_newer_than_threshold_untouched() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_fresh_claim_untouched");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "AAPL", "qty": 5}),
    )
    .await?;

    // Claim the row just now.
    let claimed_at_now = Utc::now();
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-active", claimed_at_now).await?;
    assert_eq!(claimed.len(), 1);

    // Reaper threshold: only rows older than 5 minutes are stale.
    // This row was just claimed, so it must be protected.
    let threshold = Utc::now() - Duration::minutes(5);
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;
    assert_eq!(reset_count, 0, "fresh CLAIMED row must NOT be reset");

    // Row remains CLAIMED.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.status, "CLAIMED",
        "fresh CLAIMED row must remain CLAIMED"
    );
    assert_eq!(row.claimed_by.as_deref(), Some("dispatcher-active"));

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 3: SENT rows are never reset by the stale-claim reaper
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sent_rows_never_reset_by_stale_reaper() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_sent_row_untouched");
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol": "QQQ", "qty": 10}),
    )
    .await?;

    // Claim and mark SENT (normal dispatch path).
    let claimed_at_past = Utc::now() - Duration::hours(1);
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-ok", claimed_at_past).await?;
    assert_eq!(claimed.len(), 1);
    let sent = mqk_db::outbox_mark_sent(&pool, &intent_id, chrono::Utc::now()).await?;
    assert!(sent, "outbox_mark_sent must succeed");

    // Reaper with a wide threshold that would cover any CLAIMED row.
    // SENT rows must never be touched.
    let threshold = Utc::now() + Duration::hours(1);
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;
    // reset_count may be 0 or more from other rows in a shared DB; the important
    // assertion is that *this* specific SENT row has not changed.

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.status, "SENT",
        "SENT row must never be reset by stale-claim reaper (reset_count={})",
        reset_count
    );

    Ok(())
}
