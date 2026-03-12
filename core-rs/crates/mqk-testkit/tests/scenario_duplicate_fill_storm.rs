//! Scenario: Duplicate Fill Storm
//!
//! # Invariant under test
//! Many duplicate copies of the same fill event must collapse to one durable inbox row.

use chrono::Utc;
use mqk_execution::{BrokerEvent, Side};
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
            git_hash: "SIM-DUP-FILL-STORM".to_string(),
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
async fn duplicate_fill_storm_collapses_to_one_inbox_row() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_inbox(&pool).await?;
    let run_id = make_run(&pool).await?;

    let ev = BrokerEvent::PartialFill {
        broker_message_id: "storm-fill-1".to_string(),
        broker_fill_id: None,
        internal_order_id: "ord-storm".to_string(),
        broker_order_id: Some("broker-storm".to_string()),
        symbol: "QQQ".to_string(),
        side: Side::Buy,
        delta_qty: 5,
        price_micros: 400_000_000,
        fee_micros: 0,
    };
    let msg_json = serde_json::to_value(&ev)?;

    let mut accepted = 0usize;
    let mut deduped = 0usize;

    for _ in 0..50 {
        let inserted =
            mqk_db::inbox_insert_deduped(&pool, run_id, ev.broker_message_id(), msg_json.clone())
                .await?;
        if inserted {
            accepted += 1;
        } else {
            deduped += 1;
        }
    }

    assert_eq!(accepted, 1, "exactly one duplicate storm event may insert");
    assert_eq!(
        deduped, 49,
        "all remaining duplicate storm events must dedupe"
    );

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    let matching = unapplied
        .iter()
        .filter(|r| r.broker_message_id == "storm-fill-1")
        .count();
    assert_eq!(matching, 1);

    Ok(())
}
