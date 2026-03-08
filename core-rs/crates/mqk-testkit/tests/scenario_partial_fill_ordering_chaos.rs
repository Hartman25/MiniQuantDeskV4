//! Scenario: Partial Fill Ordering Chaos
//!
//! # Invariant under test
//! Out-of-order partial fill delivery must still produce a deterministic,
//! non-duplicated inbox set keyed by broker_message_id.

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
            git_hash: "SIM-PARTIAL-CHAOS".to_string(),
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
async fn out_of_order_partial_fills_remain_distinct_and_deterministic() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_inbox(&pool).await?;
    let run_id = make_run(&pool).await?;

    let events = vec![
        BrokerEvent::PartialFill {
            broker_message_id: "pf-3".to_string(),
            internal_order_id: "ord-chaos".to_string(),
            broker_order_id: Some("broker-chaos".to_string()),
            symbol: "IWM".to_string(),
            side: Side::Buy,
            delta_qty: 3,
            price_micros: 200_000_000,
            fee_micros: 0,
        },
        BrokerEvent::PartialFill {
            broker_message_id: "pf-1".to_string(),
            internal_order_id: "ord-chaos".to_string(),
            broker_order_id: Some("broker-chaos".to_string()),
            symbol: "IWM".to_string(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 198_000_000,
            fee_micros: 0,
        },
        BrokerEvent::PartialFill {
            broker_message_id: "pf-2".to_string(),
            internal_order_id: "ord-chaos".to_string(),
            broker_order_id: Some("broker-chaos".to_string()),
            symbol: "IWM".to_string(),
            side: Side::Buy,
            delta_qty: 2,
            price_micros: 199_000_000,
            fee_micros: 0,
        },
    ];

    for ev in &events {
        mqk_db::inbox_insert_deduped(
            &pool,
            run_id,
            ev.broker_message_id(),
            serde_json::to_value(ev)?,
        )
        .await?;
    }

    let mut ids: Vec<String> = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await?
        .into_iter()
        .map(|r| r.broker_message_id)
        .collect();
    ids.sort();

    assert_eq!(ids, vec!["pf-1", "pf-2", "pf-3"]);

    Ok(())
}
