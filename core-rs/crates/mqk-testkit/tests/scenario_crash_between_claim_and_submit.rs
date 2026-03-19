//! Scenario: Crash Between Claim And Submit
//!
//! # Invariant under test
//!
//! If the process crashes after `outbox_claim_batch` but before broker submit,
//! the row must remain visible to recovery as an unacked outbox row.
//!
//! This test proves:
//! 1. The row transitions PENDING -> CLAIMED.
//! 2. No submit occurs.
//! 3. Recovery enumeration still surfaces the row.
//! 4. The row is not silently lost.
//!
//! This is a DB-backed test and skips gracefully when `MQK_DATABASE_URL` is not set.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn crash_between_claim_and_submit_surfaces_in_recovery() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
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
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let intent_id = format!("{run_id}_claim_then_crash");
    let created = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &intent_id,
        json!({"symbol":"SPY","side":"BUY","qty":1}),
    )
    .await?;
    assert!(created, "first enqueue must create the row");

    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "dispatcher-A", Utc::now()).await?;
    assert_eq!(claimed.len(), 1, "must claim exactly one row for this run");
    assert_eq!(claimed[0].row.idempotency_key, intent_id);
    assert_eq!(claimed[0].row.status, "CLAIMED");

    // Simulate crash here: no submit, no mark_sent, no release_claim.

    let pending = mqk_db::outbox_list_unacked_for_run(&pool, run_id).await?;
    assert!(
        pending.iter().any(|r| r.idempotency_key == intent_id),
        "claimed-but-unsent row must still appear in unacked recovery listing"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &intent_id)
        .await?
        .expect("outbox row must exist");
    assert_eq!(
        row.status, "CLAIMED",
        "row must remain CLAIMED after crash-before-submit simulation"
    );

    Ok(())
}
