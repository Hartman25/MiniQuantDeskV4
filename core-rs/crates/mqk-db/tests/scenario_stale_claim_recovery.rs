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
             cargo test -p mqk-db -- --include-ignored"
        ),
    }
}

//
// ---------------------------------------------------------------------------
// Test 1
// ---------------------------------------------------------------------------
//

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
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

    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_id, threshold).await?;

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
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
async fn fresh_claim_newer_than_threshold_untouched() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;

    let intent_id = format!("{run_id}_fresh_claim");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"AAPL","qty":5})).await?;

    let claimed_at = Utc::now();

    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-active", claimed_at).await?;

    let threshold = Utc::now() - Duration::minutes(5);

    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_id, threshold).await?;

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
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
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

    mqk_db::outbox_reset_stale_claims(&pool, run_id, threshold).await?;

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();

    assert_eq!(row.status, "SENT");

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 4 — DISPATCHING rows are never reset (RT-5's W4 crash window).
// ---------------------------------------------------------------------------
//

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
async fn dispatching_rows_never_reset_by_stale_reaper() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;
    let intent_id = format!("{run_id}_dispatching_row");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"IWM","qty":3})).await?;

    let claimed_at = Utc::now() - Duration::hours(1);
    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-mid-submit", claimed_at).await?;

    let marked =
        mqk_db::outbox_mark_dispatching(&pool, &intent_id, "attempt-1", Utc::now()).await?;
    assert!(marked, "row must transition CLAIMED -> DISPATCHING");

    // A threshold far in the future would reset any still-CLAIMED row --
    // DISPATCHING must be structurally immune regardless of threshold.
    let threshold = Utc::now() + Duration::hours(1);
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_id, threshold).await?;
    assert_eq!(
        reset_count, 0,
        "a DISPATCHING row must never be touched by the stale-claim reaper -- \
         submission may have already reached the broker"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();
    assert_eq!(row.status, "DISPATCHING");

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 5 — a stale claim belonging to a different run is never touched.
// ---------------------------------------------------------------------------
//

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
async fn stale_claim_for_a_different_run_is_never_touched() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_a = make_run(&pool).await?;
    let run_b = make_run(&pool).await?;
    let intent_a = format!("{run_a}_other_run_claim");

    mqk_db::outbox_enqueue(&pool, run_a, &intent_a, json!({"symbol":"SPY","qty":1})).await?;
    let claimed_at = Utc::now() - Duration::minutes(10);
    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-crashed", claimed_at).await?;

    let threshold = Utc::now() - Duration::minutes(5);
    // run_b's startup recovery must never reset run_a's stale claim.
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_b, threshold).await?;
    assert_eq!(
        reset_count, 0,
        "a different run's startup must never reset this run's stale claim"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_a)
        .await?
        .unwrap();
    assert_eq!(row.status, "CLAIMED");

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 6 — concurrent recovery attempts reset the row exactly once.
// ---------------------------------------------------------------------------
//

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
async fn concurrent_stale_claim_recovery_resets_exactly_once() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;
    let intent_id = format!("{run_id}_concurrent_reset");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"AAPL","qty":2})).await?;
    let claimed_at = Utc::now() - Duration::minutes(10);
    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-crashed", claimed_at).await?;

    let threshold = Utc::now() - Duration::minutes(5);
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

    let pool_a = pool.clone();
    let barrier_a = barrier.clone();
    let task_a = tokio::spawn(async move {
        barrier_a.wait().await;
        mqk_db::outbox_reset_stale_claims(&pool_a, run_id, threshold).await
    });
    let pool_b = pool.clone();
    let barrier_b = barrier.clone();
    let task_b = tokio::spawn(async move {
        barrier_b.wait().await;
        mqk_db::outbox_reset_stale_claims(&pool_b, run_id, threshold).await
    });

    let (result_a, result_b) = tokio::join!(task_a, task_b);
    let count_a = result_a.expect("task a must not panic")?;
    let count_b = result_b.expect("task b must not panic")?;

    assert_eq!(
        count_a + count_b,
        1,
        "exactly one of the two concurrent recovery attempts must reset the row \
         (a={count_a}, b={count_b})"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .unwrap();
    assert_eq!(row.status, "PENDING");
    assert!(row.claimed_by.is_none());

    Ok(())
}

//
// ---------------------------------------------------------------------------
// Test 7 — a recovered row is claimable again and dispatches exactly once.
// ---------------------------------------------------------------------------
//

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --features testkit -- --include-ignored"]
async fn recovered_row_is_reclaimed_and_dispatched_exactly_once() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;
    let intent_id = format!("{run_id}_recovered_dispatch");

    mqk_db::outbox_enqueue(&pool, run_id, &intent_id, json!({"symbol":"MSFT","qty":4})).await?;
    let claimed_at = Utc::now() - Duration::minutes(10);
    mqk_db::outbox_claim_batch(&pool, 1, "dispatcher-crashed", claimed_at).await?;

    // Restart recovery: reset the orphaned claim.
    let threshold = Utc::now() - Duration::minutes(5);
    let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_id, threshold).await?;
    assert_eq!(reset_count, 1);

    // The new dispatcher instance re-claims the recovered row exactly once.
    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "dispatcher-restarted", Utc::now())
            .await?;
    assert_eq!(
        claimed.len(),
        1,
        "the recovered row must be claimable again after reset"
    );
    assert_eq!(claimed[0].row.idempotency_key, intent_id);

    // A second concurrent claim attempt must find nothing left to claim --
    // the row is not duplicated or double-dispatchable.
    let second_claim =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "dispatcher-restarted-2", Utc::now())
            .await?;
    assert!(
        second_claim.is_empty(),
        "the recovered row must dispatch exactly once -- no second claim available"
    );

    Ok(())
}
