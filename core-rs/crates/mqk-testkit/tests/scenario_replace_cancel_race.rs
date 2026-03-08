//! Scenario: Replace/Cancel Race
//!
//! # Invariant under test
//! A replace/cancel race must preserve distinct durable broker event identities
//! and must not collapse different broker messages into one inbox row.

use chrono::Utc;
use mqk_execution::BrokerEvent;
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

async fn make_run(pool: &sqlx::PgPool) -> anyhow::Result<Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "SIM-REPLACE-CANCEL-RACE".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

async fn cleanup_inbox(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::query("delete from oms_inbox").execute(pool).await?;
    Ok(())
}

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replace_cancel_race_events_remain_distinct() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_inbox(&pool).await?;
    let run_id = make_run(&pool).await?;

    let cancel_ack = BrokerEvent::CancelAck {
        broker_message_id: "race-cancel-ack".to_string(),
        internal_order_id: "ord-race".to_string(),
        broker_order_id: Some("broker-race".to_string()),
    };

    let replace_reject = BrokerEvent::ReplaceReject {
        broker_message_id: "race-replace-reject".to_string(),
        internal_order_id: "ord-race".to_string(),
        broker_order_id: Some("broker-race".to_string()),
    };

    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        cancel_ack.broker_message_id(),
        serde_json::to_value(&cancel_ack)?,
    )
    .await?;
    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        replace_reject.broker_message_id(),
        serde_json::to_value(&replace_reject)?,
    )
    .await?;

    let ids: Vec<String> = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await?
        .into_iter()
        .map(|r| r.broker_message_id)
        .collect();

    assert!(
        ids.iter().any(|x| x == "race-cancel-ack"),
        "cancel ack must persist independently"
    );
    assert!(
        ids.iter().any(|x| x == "race-replace-reject"),
        "replace reject must persist independently"
    );
    assert_eq!(ids.len(), 2, "race events must not collapse into one row");

    Ok(())
}
