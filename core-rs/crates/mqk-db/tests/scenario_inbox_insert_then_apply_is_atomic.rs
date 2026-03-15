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

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
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

fn require_db_url() -> String {
    std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "PROOF: MQK_DATABASE_URL is not set.\n\
             This is a load-bearing proof test and cannot be skipped.\n\
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        )
    })
}

async fn connect_db(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;

    mqk_db::migrate(&pool).await?;
    Ok(pool)
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
async fn economic_fill_identity_is_durably_deduped() -> Result<()> {
    let url = require_db_url();
    let pool = connect_db(&url).await?;
    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();
    let now = Utc::now();

    let new_run = mqk_db::NewRun {
        run_id,
        engine_id: "test-engine".to_string(),
        mode: "PAPER".to_string(),
        started_at_utc: now,
        git_hash: "test-git".to_string(),
        config_hash: "test-config".to_string(),
        config_json: json!({}),
        host_fingerprint: "test-host".to_string(),
    };
    mqk_db::insert_run(&pool, &new_run).await?;

    let internal_order_id = "internal-order-1";
    let broker_order_id = "broker-order-1";
    let event_kind = "fill";
    let event_ts_ms = 1_700_000_000_000_i64;
    let received_at = now;

    let fill_id = "fill-abc-123".to_string();
    let msg_1 = "msg-1".to_string();
    let msg_2 = "msg-2".to_string();
    let no_fill_msg_1 = "msg-3".to_string();
    let no_fill_msg_2 = "msg-4".to_string();

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &msg_1,
        Some(fill_id.as_str()),
        internal_order_id,
        broker_order_id,
        event_kind,
        &json!({"msg": 1}),
        event_ts_ms,
        received_at,
    )
    .await?;
    assert!(first, "first fill insert must succeed");

    let second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &msg_2,
        Some(fill_id.as_str()),
        internal_order_id,
        broker_order_id,
        event_kind,
        &json!({"msg": 2}),
        event_ts_ms + 1,
        received_at,
    )
    .await?;
    assert!(
        !second,
        "same economic fill identity must dedupe even with different broker_message_id"
    );

    let duplicate_transport = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &msg_1,
        None,
        internal_order_id,
        broker_order_id,
        event_kind,
        &json!({"msg": 1}),
        event_ts_ms,
        received_at,
    )
    .await?;
    assert!(
        !duplicate_transport,
        "same broker_message_id must dedupe transport duplicate"
    );

    let no_fill_first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &no_fill_msg_1,
        None,
        internal_order_id,
        broker_order_id,
        event_kind,
        &json!({"msg": 3}),
        event_ts_ms + 2,
        received_at,
    )
    .await?;
    assert!(no_fill_first, "first no-fill insert must succeed");

    let no_fill_second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &no_fill_msg_2,
        None,
        internal_order_id,
        broker_order_id,
        event_kind,
        &json!({"msg": 4}),
        event_ts_ms + 3,
        received_at,
    )
    .await?;
    assert!(
        no_fill_second,
        "different broker_message_id without broker_fill_id must not dedupe"
    );

    Ok(())
}
