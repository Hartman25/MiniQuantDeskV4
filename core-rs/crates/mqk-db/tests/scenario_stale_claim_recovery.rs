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
//!   3. Terminal-state rows (`SENT`, `ACKED`, `FAILED`) are never reset.

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

/// Test isolation helper.
/// The outbox is shared across all DB tests so we wipe it here.
async fn cleanup_outbox(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::query("delete from broker_order_map")
        .execute(pool)
        .await?;

    sqlx::query("delete from oms_outbox").execute(pool).await?;

    Ok(())
}

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db --test scenario_stale_claim_recovery"
        ),
    }
}

//
// ---------------------------------------------------------------------------
// Test 1
// ---------------------------------------------------------------------------
//

#[tokio::test]
async fn stale_claim_older_than_threshold_reset_to_pending() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_stale_claim_reset");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"SPY","qty":1})).await?;

    // claim far in the past
    let claimed_at = Utc::now() - Duration::minutes(10);

    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-crashed", claimed_at).await?;

    assert_eq!(claimed.len(), 1);

    let threshold = Utc::now() - Duration::minutes(5);

    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;

    assert_eq!(reset_count, 1);

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();

    assert_eq!(row.status, "PENDING");
    assert!(row.claimed_by.is_none());
    assert!(row.claimed_at_utc.is_none());

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 2
// ---------------------------------------------------------------------------
//

#[tokio::test]
async fn fresh_claim_newer_than_threshold_untouched() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_fresh_claim");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"AAPL","qty":5})).await?;

    let claimed_at = Utc::now();

    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-active", claimed_at).await?;

    let threshold = Utc::now() - Duration::minutes(5);

    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;

    assert_eq!(reset_count, 0);

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();

    assert_eq!(row.status, "CLAIMED");
    assert_eq!(row.claimed_by.as_deref(), Some("dispatcher-active"));

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 3
// ---------------------------------------------------------------------------
//

#[tokio::test]
async fn sent_rows_never_reset_by_stale_reaper() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_sent_row");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"QQQ","qty":10})).await?;

    let claimed_at = Utc::now() - Duration::hours(1);

    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-ok", claimed_at).await?;

    let sent =
        mqk_db::outbox_mark_sent_with_broker_map(&pool, &intent_id, "test-broker-id", Utc::now())
            .await?;

    assert!(sent);

    let threshold = Utc::now() + Duration::hours(1);

    mqk_db::outbox_reset_stale_claims(&pool, threshold).await?;

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();

    assert_eq!(row.status, "SENT");

    Ok(())
}
