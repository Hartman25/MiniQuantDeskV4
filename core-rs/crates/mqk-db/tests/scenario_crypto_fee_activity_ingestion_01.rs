//! B6 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): durable,
//! restart-safe, deduplicated Crypto fee-activity ingestion (migration 0083,
//! `mqk_db::crypto_fee_activity`).
//!
//! # Coverage
//!
//! | Test | Claim                                                              |
//! |------|---------------------------------------------------------------------|
//! | F01  | A new activity_id inserts; the same activity_id a second time is    |
//! |      | AlreadyExists with zero row mutation -- no duplicate charge         |
//! | F02  | Cursor is None before any ingestion for a fresh (engine, mode, type) |
//! | F03  | ingest_crypto_fee_activity_batch inserts every new row and advances |
//! |      | the cursor to the batch's last activity_id, atomically              |
//! | F04  | Re-running the identical batch (simulating a restart that re-fetches|
//! |      | the same range) reports every row AlreadyExists and leaves the      |
//! |      | cursor unchanged -- no gap, no double-apply                          |
//! | F05  | A partially-pre-inserted batch (simulating a crash after activity 1 |
//! |      | committed but before the cursor advanced) correctly reports         |
//! |      | activity 1 AlreadyExists / activity 2 Inserted and advances the      |
//! |      | cursor to activity 2 -- proves restart resumption is gap-free        |
//! | F06  | CashFee/AssetDenominatedFeeUnsupported/ConfirmedZeroFee all round-   |
//! |      | trip exactly through insert + read-back                             |
//! | F07  | The DB-level payload-shape CHECK constraint refuses a CashFee row    |
//! |      | missing fee_micros -- fail-closed at the schema level, not merely    |
//! |      | caller discipline                                                    |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use chrono::Utc;
use mqk_db::crypto_fee_activity::{
    fetch_crypto_fee_activity, fetch_crypto_fee_ingestion_cursor, ingest_crypto_fee_activity_batch,
    insert_crypto_fee_activity_if_new, CryptoFeeAttributionStatus, CryptoFeeIngestionBatchOutcome,
    InsertCryptoFeeActivityOutcome, NewCryptoFeeActivity,
};
use sqlx::PgPool;
use uuid::Uuid;

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn require_pool(url: &str) -> anyhow::Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

/// Unique-per-test-run engine_id so parallel/repeated runs never collide on
/// the same (engine_id, mode, activity_type) cursor row or activity_id
/// space.
fn test_engine_id(label: &str) -> String {
    format!("test-crypto-fee-{}-{}", label, Uuid::new_v4())
}

fn cash_fee(activity_id: &str, engine_id: &str, fee_micros: i64) -> NewCryptoFeeActivity {
    NewCryptoFeeActivity {
        activity_id: activity_id.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type: "CFEE".to_string(),
        symbol: Some("BTCUSD".to_string()),
        attribution_status: CryptoFeeAttributionStatus::CashFee,
        fee_micros: Some(fee_micros),
        qty_raw: None,
        ingested_at_utc: Utc::now(),
    }
}

#[tokio::test]
async fn f01_duplicate_activity_id_is_dedup_no_duplicate_charge() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f01");
    let activity_id = format!("{engine_id}::act-1");

    let first =
        insert_crypto_fee_activity_if_new(&pool, &cash_fee(&activity_id, &engine_id, -10_000))
            .await
            .expect("first insert must succeed");
    assert_eq!(first, InsertCryptoFeeActivityOutcome::Inserted);

    // Re-insert the identical activity_id with a DIFFERENT fee amount -- if
    // dedup were broken, this would either error or (worse) silently
    // overwrite the original economic evidence. It must be a pure no-op.
    let second =
        insert_crypto_fee_activity_if_new(&pool, &cash_fee(&activity_id, &engine_id, -99_999))
            .await
            .expect("second insert must not error");
    assert_eq!(second, InsertCryptoFeeActivityOutcome::AlreadyExists);

    let stored = fetch_crypto_fee_activity(&pool, &activity_id)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored.fee_micros,
        Some(-10_000),
        "F01: the ORIGINAL fee_micros must survive; a duplicate insert must never overwrite it"
    );
}

#[tokio::test]
async fn f02_cursor_absent_before_any_ingestion() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f02");

    let cursor = fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("fetch must succeed");
    assert_eq!(cursor, None);
}

#[tokio::test]
async fn f03_batch_inserts_and_advances_cursor_atomically() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f03");
    let a1 = format!("{engine_id}::act-1");
    let a2 = format!("{engine_id}::act-2");

    let batch = vec![
        cash_fee(&a1, &engine_id, -1_000),
        cash_fee(&a2, &engine_id, -2_000),
    ];
    let outcome = ingest_crypto_fee_activity_batch(
        &pool,
        &engine_id,
        "PAPER",
        "CFEE",
        &batch,
        &a2,
        Utc::now(),
    )
    .await
    .expect("batch ingest must succeed");
    assert_eq!(
        outcome,
        CryptoFeeIngestionBatchOutcome {
            newly_inserted: 2,
            already_existed: 0,
        }
    );

    let cursor = fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("fetch must succeed");
    assert_eq!(cursor, Some(a2));
}

#[tokio::test]
async fn f04_replaying_the_identical_batch_is_a_pure_noop() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f04");
    let a1 = format!("{engine_id}::act-1");
    let a2 = format!("{engine_id}::act-2");
    let batch = vec![
        cash_fee(&a1, &engine_id, -1_000),
        cash_fee(&a2, &engine_id, -2_000),
    ];

    ingest_crypto_fee_activity_batch(&pool, &engine_id, "PAPER", "CFEE", &batch, &a2, Utc::now())
        .await
        .expect("first ingest must succeed");
    let cursor_after_first = fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("fetch must succeed");

    // Simulate a restart that (not yet knowing the cursor advanced, or
    // re-fetching an overlapping range defensively) re-submits the SAME
    // batch.
    let replay_outcome = ingest_crypto_fee_activity_batch(
        &pool,
        &engine_id,
        "PAPER",
        "CFEE",
        &batch,
        &a2,
        Utc::now(),
    )
    .await
    .expect("replay ingest must succeed");
    assert_eq!(
        replay_outcome,
        CryptoFeeIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 2,
        },
        "F04: replaying an already-ingested batch must apply zero new economic effect"
    );

    let cursor_after_replay = fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("fetch must succeed");
    assert_eq!(
        cursor_after_first, cursor_after_replay,
        "F04: cursor must be unchanged by a no-op replay"
    );
}

#[tokio::test]
async fn f05_partially_pre_inserted_batch_resumes_without_gap_or_double_apply() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f05");
    let a1 = format!("{engine_id}::act-1");
    let a2 = format!("{engine_id}::act-2");

    // Simulate a crash: activity 1 was durably inserted by a prior attempt,
    // but the cursor was never advanced (the process died between the two
    // statements of what would otherwise be one transaction).
    insert_crypto_fee_activity_if_new(&pool, &cash_fee(&a1, &engine_id, -1_000))
        .await
        .expect("pre-insert must succeed");
    assert_eq!(
        fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
            .await
            .expect("fetch must succeed"),
        None,
        "F05 setup: cursor must still be absent after the simulated crash"
    );

    // The resumed ingestion attempt re-fetches the whole range from the
    // beginning (cursor was None) and submits BOTH activities.
    let batch = vec![
        cash_fee(&a1, &engine_id, -1_000),
        cash_fee(&a2, &engine_id, -2_000),
    ];
    let outcome = ingest_crypto_fee_activity_batch(
        &pool,
        &engine_id,
        "PAPER",
        "CFEE",
        &batch,
        &a2,
        Utc::now(),
    )
    .await
    .expect("resumed ingest must succeed");
    assert_eq!(
        outcome,
        CryptoFeeIngestionBatchOutcome {
            newly_inserted: 1,
            already_existed: 1,
        },
        "F05: activity 1 must be recognized AlreadyExists, activity 2 newly Inserted"
    );

    let cursor = fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("fetch must succeed");
    assert_eq!(
        cursor,
        Some(a2),
        "F05: cursor must now correctly reflect the resumed batch's last activity"
    );
}

#[tokio::test]
async fn f06_all_three_attribution_shapes_round_trip_exactly() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f06");

    let cash = cash_fee(&format!("{engine_id}::cash"), &engine_id, -42_000);
    insert_crypto_fee_activity_if_new(&pool, &cash)
        .await
        .expect("cash insert must succeed");
    let stored_cash = fetch_crypto_fee_activity(&pool, &cash.activity_id)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_cash.attribution_status,
        CryptoFeeAttributionStatus::CashFee
    );
    assert_eq!(stored_cash.fee_micros, Some(-42_000));
    assert_eq!(stored_cash.qty_raw, None);

    let asset_denominated = NewCryptoFeeActivity {
        activity_id: format!("{engine_id}::asset"),
        engine_id: engine_id.clone(),
        mode: "PAPER".to_string(),
        activity_type: "CFEE".to_string(),
        symbol: Some("ETHUSD".to_string()),
        attribution_status: CryptoFeeAttributionStatus::AssetDenominatedFeeUnsupported,
        fee_micros: None,
        qty_raw: Some("-0.000195".to_string()),
        ingested_at_utc: Utc::now(),
    };
    insert_crypto_fee_activity_if_new(&pool, &asset_denominated)
        .await
        .expect("asset-denominated insert must succeed");
    let stored_asset = fetch_crypto_fee_activity(&pool, &asset_denominated.activity_id)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_asset.attribution_status,
        CryptoFeeAttributionStatus::AssetDenominatedFeeUnsupported
    );
    assert_eq!(stored_asset.fee_micros, None);
    assert_eq!(stored_asset.qty_raw.as_deref(), Some("-0.000195"));

    let zero = NewCryptoFeeActivity {
        activity_id: format!("{engine_id}::zero"),
        engine_id: engine_id.clone(),
        mode: "PAPER".to_string(),
        activity_type: "CFEE".to_string(),
        symbol: Some("BTCUSD".to_string()),
        attribution_status: CryptoFeeAttributionStatus::ConfirmedZeroFee,
        fee_micros: None,
        qty_raw: None,
        ingested_at_utc: Utc::now(),
    };
    insert_crypto_fee_activity_if_new(&pool, &zero)
        .await
        .expect("confirmed-zero insert must succeed");
    let stored_zero = fetch_crypto_fee_activity(&pool, &zero.activity_id)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_zero.attribution_status,
        CryptoFeeAttributionStatus::ConfirmedZeroFee
    );
    assert_eq!(stored_zero.fee_micros, None);
    assert_eq!(stored_zero.qty_raw, None);
}

#[tokio::test]
async fn f07_payload_shape_check_constraint_refuses_malformed_cash_fee() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("f07");

    // A CashFee row must carry fee_micros; this one deliberately omits it
    // (leaves it None) to prove the DB-level CHECK constraint (migration
    // 0083's sys_crypto_fee_activity_ledger_payload_shape_check) is real
    // fail-closed schema enforcement, not merely something this crate's own
    // Rust types happen to prevent.
    let malformed = NewCryptoFeeActivity {
        activity_id: format!("{engine_id}::malformed"),
        engine_id: engine_id.clone(),
        mode: "PAPER".to_string(),
        activity_type: "CFEE".to_string(),
        symbol: Some("BTCUSD".to_string()),
        attribution_status: CryptoFeeAttributionStatus::CashFee,
        fee_micros: None,
        qty_raw: None,
        ingested_at_utc: Utc::now(),
    };

    let result = insert_crypto_fee_activity_if_new(&pool, &malformed).await;
    assert!(
        result.is_err(),
        "F07: a CashFee row with no fee_micros must be refused by the DB CHECK constraint"
    );
}
