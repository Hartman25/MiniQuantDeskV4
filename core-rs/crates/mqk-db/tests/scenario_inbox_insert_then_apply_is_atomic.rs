//! Scenario: Inbox Insert → Apply Is Atomic — Patch L5
//!
//! # Invariant under test
//! The apply path (mutating in-process state such as a portfolio ledger) MUST
//! be gated on the `inbox_insert_deduped` return value.
//!
//! - `true`  → first-time insert: gate **opens**, apply is permitted.
//! - `false` → duplicate (same `broker_message_id`): gate **closed**, apply
//!   is skipped entirely — no double-apply regardless of retries.
//!
//! These tests require a live Postgres instance (MQK_DATABASE_URL).
//! Without it each test skips with a log message — CI-safe.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helper: apply gate (simulates ledger mutation as an in-process counter)
// ---------------------------------------------------------------------------

/// Stand-in for "apply fill to ledger".
/// The counter represents the ledger's entry count; it only increments when
/// the inbox gate opens (first-time insert).
fn apply_if_inserted(inserted: bool, apply_count: &mut u32) {
    if inserted {
        *apply_count += 1;
    }
}

// ---------------------------------------------------------------------------
// Test 1: First insert gates apply; duplicate is a no-op
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_insert_gates_apply_duplicate_is_noop() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "PROOF: MQK_DATABASE_URL is not set. 
             This is a load-bearing proof test and cannot be skipped. 
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
            );
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
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let broker_fill_id = format!("FILL-{}", Uuid::new_v4());
    let fill_json = json!({"symbol": "SPY", "qty": 10, "price": 450.0});
    let mut apply_count = 0u32;

    // --- First delivery: gate opens → apply runs ---
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, &broker_fill_id, fill_json.clone()).await?;
    apply_if_inserted(inserted, &mut apply_count);

    assert!(inserted, "first inbox insert must succeed and return true");
    assert_eq!(
        apply_count, 1,
        "apply must run exactly once on first insert"
    );

    // --- Duplicate delivery (same broker_fill_id): gate closed → apply skipped ---
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, &broker_fill_id, fill_json.clone()).await?;
    apply_if_inserted(inserted, &mut apply_count);

    assert!(
        !inserted,
        "duplicate broker_fill_id must return false (deduped)"
    );
    assert_eq!(
        apply_count, 1,
        "apply count must remain 1 after duplicate insert"
    );

    // --- Second duplicate to confirm it is not a one-shot fluke ---
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, &broker_fill_id, fill_json.clone()).await?;
    apply_if_inserted(inserted, &mut apply_count);

    assert!(!inserted, "third insert attempt must also return false");
    assert_eq!(
        apply_count, 1,
        "apply count must still be 1 after third attempt"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2: Two distinct fill IDs each gate their apply exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn distinct_fill_ids_each_apply_exactly_once() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "PROOF: MQK_DATABASE_URL is not set. 
             This is a load-bearing proof test and cannot be skipped. 
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
            );
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
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let fill_id_a = format!("FILL-A-{}", Uuid::new_v4());
    let fill_id_b = format!("FILL-B-{}", Uuid::new_v4());
    let fill_json = json!({"qty": 5});
    let mut apply_count = 0u32;

    // First pass: both inserts succeed and gate apply.
    for id in [&fill_id_a, &fill_id_b] {
        let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, id, fill_json.clone()).await?;
        apply_if_inserted(inserted, &mut apply_count);
    }
    assert_eq!(
        apply_count, 2,
        "two distinct fill IDs must each trigger apply once"
    );

    // Replay: both inserts are now duplicates — apply must not run.
    for id in [&fill_id_a, &fill_id_b] {
        let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, id, fill_json.clone()).await?;
        apply_if_inserted(inserted, &mut apply_count);
    }
    assert_eq!(
        apply_count, 2,
        "replayed fills must not increment apply count"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 3: broker_message_id dedupe is run-scoped, not global
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_message_id_uniqueness_is_run_scoped() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "PROOF: MQK_DATABASE_URL is not set. 
             This is a load-bearing proof test and cannot be skipped. 
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
            );
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    let run_id_1 = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run_id_1,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let run_id_2 = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run_id_2,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let shared_message_id = format!("SHARED-MSG-{}", Uuid::new_v4());
    let fill_json = json!({"qty": 25});

    let inserted_1 =
        mqk_db::inbox_insert_deduped(&pool, run_id_1, &shared_message_id, fill_json.clone())
            .await?;
    assert!(inserted_1, "first insert must succeed");

    let inserted_2 =
        mqk_db::inbox_insert_deduped(&pool, run_id_2, &shared_message_id, fill_json.clone())
            .await?;
    assert!(
        inserted_2,
        "same broker_message_id under a different run must still insert because dedupe is run-scoped"
    );

    Ok(())
}

#[tokio::test]
async fn transport_dedupe_is_distinct_from_economic_fill_identity() -> anyhow::Result<()> {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("MQK_DATABASE_URL must be set");
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
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let fill_id = format!("ECON-FILL-{}", Uuid::new_v4());
    let msg_1 = format!("transport-1-{}", Uuid::new_v4());
    let msg_2 = format!("transport-2-{}", Uuid::new_v4());

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &mqk_db::BrokerEventIdentity {
            broker_message_id: msg_1.clone(),
            broker_fill_id: Some(fill_id.clone()),
            broker_sequence_id: None,
            broker_timestamp: None,
        },
        json!({"msg": 1}),
    )
    .await?;
    assert!(first, "first transport event should insert");

    let second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &mqk_db::BrokerEventIdentity {
            broker_message_id: msg_2,
            broker_fill_id: Some(fill_id),
            broker_sequence_id: None,
            broker_timestamp: None,
        },
        json!({"msg": 2}),
    )
    .await?;
    assert!(
        second,
        "a distinct broker_message_id should insert even when broker_fill_id matches"
    );

    let duplicate_transport = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &mqk_db::BrokerEventIdentity {
            broker_message_id: msg_1,
            broker_fill_id: None,
            broker_sequence_id: None,
            broker_timestamp: None,
        },
        json!({"msg": 1}),
    )
    .await?;
    assert!(
        !duplicate_transport,
        "transport dedupe should still be keyed by broker_message_id"
    );

    Ok(())
}
