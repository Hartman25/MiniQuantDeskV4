//! Scenario: Broker Replay After Restart
//!
//! # Invariant under test
//! A broker event replayed after restart with the same broker_message_id
//! must not be applied twice.
//!
//! # PROOF LANE
//!
//! This is a load-bearing institutional proof test. It MUST fail hard if
//! MQK_DATABASE_URL is absent or the DB is unreachable. Silent skip is not
//! acceptable — a skipped proof test is an unproven invariant.
//!
//! Run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-testkit

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
            git_hash: "SIM-BROKER-REPLAY".to_string(),
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
async fn restart_replay_preserves_durable_apply_order() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_inbox(&pool).await?;
    let run_id = make_run(&pool).await?;

    let ev = BrokerEvent::Fill {
        broker_message_id: "replay-msg-1".to_string(),
        broker_fill_id: None,
        internal_order_id: "ord-1".to_string(),
        broker_order_id: Some("broker-1".to_string()),
        symbol: "SPY".to_string(),
        side: Side::Buy,
        delta_qty: 10,
        price_micros: 500_000_000,
        fee_micros: 0,
    };

    let msg_json = serde_json::to_value(&ev)?;

    let inserted_1 =
        mqk_db::inbox_insert_deduped(&pool, run_id, ev.broker_message_id(), msg_json.clone())
            .await?;
    assert!(
        inserted_1,
        "first insert of replayed broker event must be accepted"
    );

    // Simulated restart: same broker replay arrives again.
    let inserted_2 =
        mqk_db::inbox_insert_deduped(&pool, run_id, ev.broker_message_id(), msg_json).await?;
    assert!(
        !inserted_2,
        "same broker_message_id must be deduped after restart replay"
    );

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    let matching = unapplied
        .iter()
        .filter(|r| r.broker_message_id == "replay-msg-1")
        .count();
    assert_eq!(
        matching, 1,
        "only one inbox row may exist for replayed event"
    );

    Ok(())
}
