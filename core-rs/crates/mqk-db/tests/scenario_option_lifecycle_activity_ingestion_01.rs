//! D1 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): durable,
//! restart-safe, deduplicated options lifecycle activity ingestion
//! (migration 0084, `mqk_db::option_lifecycle_activity`).
//!
//! # Coverage
//!
//! | Test | Claim                                                               |
//! |------|-----------------------------------------------------------------------|
//! | H01  | A new (account, activity_id, activity_type) inserts; a second insert |
//! |      | of the identical triple is AlreadyExists with zero row mutation      |
//! | H02  | Cursor is None before any ingestion for a fresh (account, engine,    |
//! |      | mode, type)                                                           |
//! | H03  | ingest_option_lifecycle_activity_batch inserts every new row and      |
//! |      | advances the cursor atomically                                        |
//! | H04  | Replaying an identical batch is a pure no-op                          |
//! | H05  | The DB-level price-shape CHECK constraint refuses an OPTRD row with   |
//! |      | no price, and refuses an OPEXC row carrying a price; the symbol-shape |
//! |      | CHECK refuses an OPEXC row with no option_symbol and an OPTRD row     |
//! |      | with no underlying_symbol_raw                                         |
//! | H06  | find_paired_trade_activity finds an OPTRD sharing the SAME             |
//! |      | activity_id as an OPEXC/OPASN row, and returns None when no pairing   |
//! |      | exists yet                                                            |
//! | H07  | insert_applied_option_lifecycle_effect_if_new is idempotent on        |
//! |      | lifecycle_activity_id -- a second apply attempt is a checked no-op    |
//! | H08  | D1 correction: an OPEXC and its paired OPTRD sharing the IDENTICAL    |
//! |      | activity_id coexist as two distinct rows (the confirmed 0084          |
//! |      | collision this migration closes)                                      |
//! | H09  | D1 correction: two different broker accounts sharing the same         |
//! |      | activity_id never collide, and never share a cursor watermark         |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use chrono::Utc;
use mqk_db::option_lifecycle_activity::{
    fetch_applied_option_lifecycle_effect, fetch_option_lifecycle_activity,
    fetch_option_lifecycle_ingestion_cursor, find_paired_trade_activity,
    ingest_option_lifecycle_activity_batch, insert_applied_option_lifecycle_effect_if_new,
    insert_option_lifecycle_activity_if_new, AppliedOptionLifecycleEffect,
    InsertAppliedOptionLifecycleEffectOutcome, InsertOptionLifecycleActivityOutcome,
    NewOptionLifecycleActivity, OptionLifecycleActivityType, OptionLifecycleIngestionBatchOutcome,
};
use sqlx::PgPool;
use uuid::Uuid;

const TEST_BROKER_ACCOUNT_ID: &str = "test-alpaca-key-id";

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

fn test_engine_id(label: &str) -> String {
    format!("test-option-lifecycle-{}-{}", label, Uuid::new_v4())
}

fn lifecycle_activity(
    activity_id: &str,
    broker_account_id: &str,
    engine_id: &str,
    activity_type: OptionLifecycleActivityType,
    option_symbol: &str,
    activity_date: &str,
    qty_raw: &str,
) -> NewOptionLifecycleActivity {
    NewOptionLifecycleActivity {
        activity_id: activity_id.to_string(),
        broker_account_id: broker_account_id.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type,
        option_symbol: Some(option_symbol.to_string()),
        underlying_symbol_raw: None,
        activity_date: activity_date.to_string(),
        qty_raw: qty_raw.to_string(),
        price_raw: None,
        net_amount_raw: "0".to_string(),
        ingested_at_utc: Utc::now(),
    }
}

/// Shares `activity_id` with its lifecycle sibling by construction — the
/// caller must pass the SAME `activity_id`, per D1's corrected pairing
/// contract.
fn paired_trade_activity(
    activity_id: &str,
    broker_account_id: &str,
    engine_id: &str,
    underlying_symbol_raw: &str,
    activity_date: &str,
    qty_raw: &str,
    price_raw: &str,
) -> NewOptionLifecycleActivity {
    NewOptionLifecycleActivity {
        activity_id: activity_id.to_string(),
        broker_account_id: broker_account_id.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type: OptionLifecycleActivityType::PairedTrade,
        option_symbol: None,
        underlying_symbol_raw: Some(underlying_symbol_raw.to_string()),
        activity_date: activity_date.to_string(),
        qty_raw: qty_raw.to_string(),
        price_raw: Some(price_raw.to_string()),
        net_amount_raw: "0".to_string(),
        ingested_at_utc: Utc::now(),
    }
}

#[tokio::test]
async fn h01_duplicate_activity_id_is_dedup_no_mutation() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h01");
    let activity_id = format!("{engine_id}::act-1");

    let a = lifecycle_activity(
        &activity_id,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-2",
    );
    let first = insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .expect("first insert must succeed");
    assert_eq!(first, InsertOptionLifecycleActivityOutcome::Inserted);

    let second = insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .expect("second insert must not error");
    assert_eq!(second, InsertOptionLifecycleActivityOutcome::AlreadyExists);

    let stored = fetch_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &activity_id,
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed")
    .expect("row must exist");
    assert_eq!(stored.qty_raw, "-2");
}

#[tokio::test]
async fn h02_cursor_absent_before_any_ingestion() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h02");

    let cursor = fetch_option_lifecycle_ingestion_cursor(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed");
    assert_eq!(cursor, None);
}

#[tokio::test]
async fn h03_batch_inserts_and_advances_cursor_atomically() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h03");
    let a1 = format!("{engine_id}::act-1");
    let a2 = format!("{engine_id}::act-2");

    let batch = vec![
        lifecycle_activity(
            &a1,
            TEST_BROKER_ACCOUNT_ID,
            &engine_id,
            OptionLifecycleActivityType::Exercise,
            "AAPL260619C00200000",
            "2026-06-19",
            "-1",
        ),
        lifecycle_activity(
            &a2,
            TEST_BROKER_ACCOUNT_ID,
            &engine_id,
            OptionLifecycleActivityType::Exercise,
            "AAPL260619C00200000",
            "2026-06-20",
            "-1",
        ),
    ];
    let outcome = ingest_option_lifecycle_activity_batch(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Exercise,
        &batch,
        &a2,
        Utc::now(),
    )
    .await
    .expect("batch ingest must succeed");
    assert_eq!(
        outcome,
        OptionLifecycleIngestionBatchOutcome {
            newly_inserted: 2,
            already_existed: 0,
        }
    );

    let cursor = fetch_option_lifecycle_ingestion_cursor(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed");
    assert_eq!(cursor, Some(a2));
}

#[tokio::test]
async fn h04_replaying_the_identical_batch_is_a_pure_noop() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h04");
    let a1 = format!("{engine_id}::act-1");

    let batch = vec![lifecycle_activity(
        &a1,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Expiration,
        "AAPL260619C00200000",
        "2026-06-19",
        "-1",
    )];
    ingest_option_lifecycle_activity_batch(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Expiration,
        &batch,
        &a1,
        Utc::now(),
    )
    .await
    .expect("first ingest must succeed");

    let replay = ingest_option_lifecycle_activity_batch(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Expiration,
        &batch,
        &a1,
        Utc::now(),
    )
    .await
    .expect("replay ingest must succeed");
    assert_eq!(
        replay,
        OptionLifecycleIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 1,
        },
        "H04: replaying an already-ingested batch must apply zero new economic effect"
    );
}

#[tokio::test]
async fn h05_shape_check_constraints_enforced() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h05");

    // OPTRD with no price must be refused.
    let mut malformed_optrd = paired_trade_activity(
        &format!("{engine_id}::optrd-bad"),
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "200",
        "0",
    );
    malformed_optrd.price_raw = None;
    let result = insert_option_lifecycle_activity_if_new(&pool, &malformed_optrd).await;
    assert!(
        result.is_err(),
        "H05: an OPTRD row with no price must be refused by the DB CHECK constraint"
    );

    // OPEXC carrying a price must be refused.
    let mut malformed_opexc = lifecycle_activity(
        &format!("{engine_id}::opexc-bad"),
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-1",
    );
    malformed_opexc.price_raw = Some("200".to_string());
    let result = insert_option_lifecycle_activity_if_new(&pool, &malformed_opexc).await;
    assert!(
        result.is_err(),
        "H05: an OPEXC row carrying a price must be refused by the DB CHECK constraint"
    );

    // OPEXC with no option_symbol must be refused.
    let mut no_symbol_opexc = lifecycle_activity(
        &format!("{engine_id}::opexc-nosym"),
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-1",
    );
    no_symbol_opexc.option_symbol = None;
    let result = insert_option_lifecycle_activity_if_new(&pool, &no_symbol_opexc).await;
    assert!(
        result.is_err(),
        "H05: an OPEXC row with no option_symbol must be refused by the DB CHECK constraint"
    );

    // OPTRD with no underlying_symbol_raw must be refused.
    let mut no_underlying_optrd = paired_trade_activity(
        &format!("{engine_id}::optrd-nosym"),
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "200",
        "200.00",
    );
    no_underlying_optrd.underlying_symbol_raw = None;
    let result = insert_option_lifecycle_activity_if_new(&pool, &no_underlying_optrd).await;
    assert!(
        result.is_err(),
        "H05: an OPTRD row with no underlying_symbol_raw must be refused by the DB CHECK constraint"
    );
}

#[tokio::test]
async fn h06_paired_trade_lookup_finds_match_and_returns_none_when_absent() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h06");
    let activity_id = format!("{engine_id}::shared");

    let opexc = lifecycle_activity(
        &activity_id,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-2",
    );
    insert_option_lifecycle_activity_if_new(&pool, &opexc)
        .await
        .unwrap();

    // No paired OPTRD ingested yet -- must be None, never fabricated.
    let before = find_paired_trade_activity(&pool, TEST_BROKER_ACCOUNT_ID, &activity_id)
        .await
        .expect("lookup must succeed");
    assert_eq!(
        before, None,
        "H06: no paired trade evidence exists yet; must be None"
    );

    let optrd = paired_trade_activity(
        &activity_id,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "200",
        "200.00",
    );
    insert_option_lifecycle_activity_if_new(&pool, &optrd)
        .await
        .unwrap();

    let after = find_paired_trade_activity(&pool, TEST_BROKER_ACCOUNT_ID, &activity_id)
        .await
        .expect("lookup must succeed")
        .expect("paired trade must now be found");
    assert_eq!(after.price_raw.as_deref(), Some("200.00"));
    assert_eq!(after.qty_raw, "200");
    assert_eq!(after.underlying_symbol_raw.as_deref(), Some("AAPL"));
}

#[tokio::test]
async fn h07_applied_effect_insert_is_idempotent_on_lifecycle_activity_id() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h07");
    let lifecycle_activity_id = format!("{engine_id}::opexc");

    let effect = AppliedOptionLifecycleEffect {
        lifecycle_activity_id: lifecycle_activity_id.clone(),
        broker_account_id: TEST_BROKER_ACCOUNT_ID.to_string(),
        engine_id: engine_id.clone(),
        mode: "PAPER".to_string(),
        option_symbol: "AAPL260619C00200000".to_string(),
        underlying_symbol: Some("AAPL".to_string()),
        option_contracts_removed_raw: "2".to_string(),
        underlying_shares_delivered_raw: Some("200".to_string()),
        cash_effect_micros: Some(40_000_000_000),
        applied_at_utc: Utc::now(),
    };

    let first = insert_applied_option_lifecycle_effect_if_new(&pool, &effect)
        .await
        .expect("first apply must succeed");
    assert_eq!(first, InsertAppliedOptionLifecycleEffectOutcome::Applied);

    // Re-attempt with a DIFFERENT cash_effect_micros -- if idempotency were
    // broken, this would silently overwrite the original applied evidence.
    let mut replay_effect = effect.clone();
    replay_effect.cash_effect_micros = Some(999);
    let second = insert_applied_option_lifecycle_effect_if_new(&pool, &replay_effect)
        .await
        .expect("second apply must not error");
    assert_eq!(
        second,
        InsertAppliedOptionLifecycleEffectOutcome::AlreadyApplied
    );

    let stored = fetch_applied_option_lifecycle_effect(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &lifecycle_activity_id,
    )
    .await
    .expect("fetch must succeed")
    .expect("row must exist");
    assert_eq!(
        stored.cash_effect_micros,
        Some(40_000_000_000),
        "H07: the ORIGINAL applied effect must survive; a duplicate apply must never overwrite it"
    );
}

#[tokio::test]
async fn h08_opexc_and_its_paired_optrd_sharing_the_identical_id_coexist() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h08");
    // The identical id -- exactly the collision Alpaca's own docs confirm
    // and 0084's activity_id-only PRIMARY KEY could not represent.
    let shared_id = format!("{engine_id}::shared");

    let opexc = lifecycle_activity(
        &shared_id,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-2",
    );
    let opexc_outcome = insert_option_lifecycle_activity_if_new(&pool, &opexc)
        .await
        .expect("OPEXC insert must succeed");
    assert_eq!(
        opexc_outcome,
        InsertOptionLifecycleActivityOutcome::Inserted
    );

    let optrd = paired_trade_activity(
        &shared_id,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "200",
        "200.00",
    );
    let optrd_outcome = insert_option_lifecycle_activity_if_new(&pool, &optrd)
        .await
        .expect("OPTRD insert must succeed");
    assert_eq!(
        optrd_outcome,
        InsertOptionLifecycleActivityOutcome::Inserted,
        "H08: the OPTRD row sharing the OPEXC row's exact activity_id must be a genuinely NEW \
         row, never dropped as a false duplicate"
    );

    let stored_opexc = fetch_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &shared_id,
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed")
    .expect("OPEXC row must exist");
    let stored_optrd = fetch_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &shared_id,
        OptionLifecycleActivityType::PairedTrade,
    )
    .await
    .expect("fetch must succeed")
    .expect("OPTRD row must exist");

    assert_eq!(stored_opexc.qty_raw, "-2");
    assert_eq!(stored_optrd.qty_raw, "200");
    assert_eq!(
        stored_opexc.activity_id, stored_optrd.activity_id,
        "H08: both rows share the identical activity_id by construction"
    );
}

#[tokio::test]
async fn h09_two_broker_accounts_sharing_an_activity_id_never_collide_or_share_a_cursor() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("h09");
    let shared_activity_id = format!("{engine_id}::shared");

    let account_a = lifecycle_activity(
        &shared_activity_id,
        "account-A",
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-1",
    );
    let outcome_a = insert_option_lifecycle_activity_if_new(&pool, &account_a)
        .await
        .expect("account A insert must succeed");
    assert_eq!(outcome_a, InsertOptionLifecycleActivityOutcome::Inserted);

    let account_b = lifecycle_activity(
        &shared_activity_id,
        "account-B",
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        "AAPL260619C00200000",
        "2026-06-19",
        "-1",
    );
    let outcome_b = insert_option_lifecycle_activity_if_new(&pool, &account_b)
        .await
        .expect("account B insert must succeed");
    assert_eq!(
        outcome_b,
        InsertOptionLifecycleActivityOutcome::Inserted,
        "H09: account B's activity must be genuinely new, not dropped as a false duplicate of \
         account A's identically-numbered activity"
    );

    // Advance only account A's cursor.
    ingest_option_lifecycle_activity_batch(
        &pool,
        "account-A",
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Exercise,
        &[account_a],
        &shared_activity_id,
        Utc::now(),
    )
    .await
    .expect("account A batch ingest must succeed");

    let cursor_b = fetch_option_lifecycle_ingestion_cursor(
        &pool,
        "account-B",
        &engine_id,
        "PAPER",
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("cursor fetch must succeed");
    assert_eq!(
        cursor_b, None,
        "H09: account B's cursor must never read account A's watermark"
    );
}
