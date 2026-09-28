//! B6 correction (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
//! `mqk_daemon::state::crypto_fee_ingestion::ingest_crypto_fee_activities_once`
//! is the actual production-safe ingestion caller B6's original commit left
//! unbuilt (durable ledger/cursor existed with zero caller). These tests
//! prove the CALLER itself — not just the DB layer B6's own
//! `scenario_crypto_fee_activity_ingestion_01.rs` already proved — is
//! genuinely restart-safe end-to-end: the cursor is read fresh from the DB
//! on every call and threaded into the fetcher's `after_id`, a normalize
//! failure fails the whole attempt closed before any DB write, and an
//! overlapping re-fetch after a simulated restart produces zero duplicate
//! economic effect.
//!
//! # Coverage
//!
//! | Test  | Claim                                                              |
//! |-------|---------------------------------------------------------------------|
//! | G01   | An empty fetch result is a pure no-op: zero DB writes, no cursor    |
//! |       | row created                                                          |
//! | G02   | A fresh batch is ingested and the cursor advances to the last       |
//! |       | activity's id — all three attribution shapes recognized             |
//! | G03   | A second call reads the PERSISTED cursor from the DB (not an        |
//! |       | in-memory value) and passes it as `after_id` — proves restart       |
//! |       | safety through the real caller, not merely the DB layer             |
//! | G04   | Replaying an overlapping batch after a simulated restart produces   |
//! |       | zero newly_inserted for the already-ingested activities             |
//! | G05   | A normalize failure (unparseable net_amount) fails the whole        |
//! |       | attempt closed: `Err`, zero DB writes, cursor unchanged             |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres) — load-bearing, must fail hard if
//! `MQK_DATABASE_URL` is absent, not skip. No real Alpaca call anywhere:
//! every fetcher here is a fake `CryptoFeeActivityFetcher` implementation.

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaFeeActivity;
use mqk_daemon::state::crypto_fee_ingestion::ingest_crypto_fee_activities_once;
use mqk_daemon::state::CryptoFeeActivityFetcher;
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

fn test_engine_id(label: &str) -> String {
    format!("test-crypto-fee-caller-{}-{}", label, Uuid::new_v4())
}

fn cash_fee_activity(id: &str, symbol: &str, net_amount: &str) -> AlpacaFeeActivity {
    AlpacaFeeActivity {
        id: id.to_string(),
        activity_type: "CFEE".to_string(),
        date: None,
        net_amount: net_amount.to_string(),
        description: None,
        symbol: Some(symbol.to_string()),
        qty: None,
        price: None,
        status: None,
    }
}

fn asset_denominated_activity(id: &str, symbol: &str, qty: &str) -> AlpacaFeeActivity {
    AlpacaFeeActivity {
        id: id.to_string(),
        activity_type: "CFEE".to_string(),
        date: None,
        net_amount: "0".to_string(),
        description: None,
        symbol: Some(symbol.to_string()),
        qty: Some(qty.to_string()),
        price: None,
        status: None,
    }
}

fn confirmed_zero_fee_activity(id: &str, symbol: &str) -> AlpacaFeeActivity {
    AlpacaFeeActivity {
        id: id.to_string(),
        activity_type: "CFEE".to_string(),
        date: None,
        net_amount: "0".to_string(),
        description: None,
        symbol: Some(symbol.to_string()),
        qty: None,
        price: None,
        status: None,
    }
}

fn malformed_activity(id: &str) -> AlpacaFeeActivity {
    AlpacaFeeActivity {
        id: id.to_string(),
        activity_type: "CFEE".to_string(),
        date: None,
        net_amount: "not-a-number".to_string(),
        description: None,
        symbol: None,
        qty: None,
        price: None,
        status: None,
    }
}

/// Always returns the same fixed batch, regardless of `after_id`.
struct FixedFetcher(Vec<AlpacaFeeActivity>);

impl CryptoFeeActivityFetcher for FixedFetcher {
    fn fetch_fee_activities_since(
        &self,
        _activity_type: &str,
        _after_id: Option<&str>,
    ) -> Result<Vec<AlpacaFeeActivity>, String> {
        Ok(self.0.clone())
    }
}

/// Asserts the exact `after_id` the caller passes (the G03 restart-safety
/// proof), then returns a fixed batch.
struct AssertingFetcher {
    expected_after_id: Option<String>,
    activities: Vec<AlpacaFeeActivity>,
}

impl CryptoFeeActivityFetcher for AssertingFetcher {
    fn fetch_fee_activities_since(
        &self,
        _activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaFeeActivity>, String> {
        assert_eq!(
            after_id,
            self.expected_after_id.as_deref(),
            "G03: the caller must read the persisted cursor from the DB and pass it as \
             after_id -- a stale/absent cursor here means restart safety is broken"
        );
        Ok(self.activities.clone())
    }
}

#[tokio::test]
async fn g01_empty_fetch_is_pure_noop() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("g01");
    let fetcher = FixedFetcher(vec![]);

    let outcome =
        ingest_crypto_fee_activities_once(&pool, &fetcher, &engine_id, "PAPER", "CFEE", Utc::now())
            .await
            .expect("empty fetch must not error");
    assert_eq!(outcome.newly_inserted, 0);
    assert_eq!(outcome.already_existed, 0);

    let cursor = mqk_db::fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("cursor fetch must succeed");
    assert_eq!(
        cursor, None,
        "G01: an empty fetch must never create a cursor row"
    );
}

#[tokio::test]
async fn g02_fresh_batch_ingests_and_advances_cursor_all_three_shapes() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("g02");
    let a1 = format!("{engine_id}::cash");
    let a2 = format!("{engine_id}::asset");
    let a3 = format!("{engine_id}::zero");

    let fetcher = FixedFetcher(vec![
        cash_fee_activity(&a1, "BTCUSD", "-0.05"),
        asset_denominated_activity(&a2, "ETHUSD", "-0.0002"),
        confirmed_zero_fee_activity(&a3, "BTCUSD"),
    ]);

    let outcome =
        ingest_crypto_fee_activities_once(&pool, &fetcher, &engine_id, "PAPER", "CFEE", Utc::now())
            .await
            .expect("ingest must succeed");
    assert_eq!(outcome.newly_inserted, 3);
    assert_eq!(outcome.already_existed, 0);

    let cursor = mqk_db::fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("cursor fetch must succeed");
    assert_eq!(
        cursor,
        Some(a3.clone()),
        "G02: cursor must advance to the last (ascending-order) activity id"
    );

    let stored_cash = mqk_db::fetch_crypto_fee_activity(&pool, &a1)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_cash.attribution_status,
        mqk_db::CryptoFeeAttributionStatus::CashFee
    );
    assert_eq!(stored_cash.fee_micros, Some(-50_000));

    let stored_asset = mqk_db::fetch_crypto_fee_activity(&pool, &a2)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_asset.attribution_status,
        mqk_db::CryptoFeeAttributionStatus::AssetDenominatedFeeUnsupported
    );
    assert_eq!(stored_asset.qty_raw.as_deref(), Some("-0.0002"));

    let stored_zero = mqk_db::fetch_crypto_fee_activity(&pool, &a3)
        .await
        .expect("fetch must succeed")
        .expect("row must exist");
    assert_eq!(
        stored_zero.attribution_status,
        mqk_db::CryptoFeeAttributionStatus::ConfirmedZeroFee
    );
}

#[tokio::test]
async fn g03_second_call_reads_persisted_cursor_and_threads_it_as_after_id() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("g03");
    let a1 = format!("{engine_id}::first");
    let a2 = format!("{engine_id}::second");

    let first_fetcher = FixedFetcher(vec![cash_fee_activity(&a1, "BTCUSD", "-0.01")]);
    ingest_crypto_fee_activities_once(
        &pool,
        &first_fetcher,
        &engine_id,
        "PAPER",
        "CFEE",
        Utc::now(),
    )
    .await
    .expect("first ingest must succeed");

    // Simulate a fresh process (or a later scheduled tick): a brand new
    // fetcher instance, which has no way to know the prior in-memory state
    // -- if the caller re-reads the DB cursor correctly, this fetcher's own
    // assertion inside fetch_fee_activities_since will pass.
    let second_fetcher = AssertingFetcher {
        expected_after_id: Some(a1.clone()),
        activities: vec![cash_fee_activity(&a2, "ETHUSD", "-0.02")],
    };
    let outcome = ingest_crypto_fee_activities_once(
        &pool,
        &second_fetcher,
        &engine_id,
        "PAPER",
        "CFEE",
        Utc::now(),
    )
    .await
    .expect("second ingest must succeed");
    assert_eq!(outcome.newly_inserted, 1);

    let cursor = mqk_db::fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("cursor fetch must succeed");
    assert_eq!(cursor, Some(a2));
}

#[tokio::test]
async fn g04_overlapping_replay_after_restart_produces_zero_duplicate_effect() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("g04");
    let a1 = format!("{engine_id}::act1");
    let a2 = format!("{engine_id}::act2");

    let batch = vec![
        cash_fee_activity(&a1, "BTCUSD", "-0.01"),
        cash_fee_activity(&a2, "BTCUSD", "-0.02"),
    ];

    let first_fetcher = FixedFetcher(batch.clone());
    let first = ingest_crypto_fee_activities_once(
        &pool,
        &first_fetcher,
        &engine_id,
        "PAPER",
        "CFEE",
        Utc::now(),
    )
    .await
    .expect("first ingest must succeed");
    assert_eq!(first.newly_inserted, 2);

    // A defensive/naive caller re-fetches the SAME overlapping range after a
    // simulated restart (e.g. it re-fetched from an earlier watermark than
    // strictly necessary). The DB-level activity_id dedup must make this a
    // pure no-op regardless of what after_id this second fetcher used.
    let second_fetcher = FixedFetcher(batch);
    let second = ingest_crypto_fee_activities_once(
        &pool,
        &second_fetcher,
        &engine_id,
        "PAPER",
        "CFEE",
        Utc::now(),
    )
    .await
    .expect("replay ingest must succeed");
    assert_eq!(
        second.newly_inserted, 0,
        "G04: replaying an overlapping already-ingested batch must apply zero new economic effect"
    );
    assert_eq!(second.already_existed, 2);
}

#[tokio::test]
async fn g05_normalize_failure_fails_closed_before_any_db_write() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("g05");
    let good_id = format!("{engine_id}::good");
    let bad_id = format!("{engine_id}::bad");

    // A batch mixing one valid activity and one with an unparseable
    // net_amount -- the whole attempt must fail closed, not partially apply
    // the valid one.
    let fetcher = FixedFetcher(vec![
        cash_fee_activity(&good_id, "BTCUSD", "-0.01"),
        malformed_activity(&bad_id),
    ]);

    let result =
        ingest_crypto_fee_activities_once(&pool, &fetcher, &engine_id, "PAPER", "CFEE", Utc::now())
            .await;
    assert!(
        result.is_err(),
        "G05: a normalize failure anywhere in the batch must fail the whole attempt"
    );

    let cursor = mqk_db::fetch_crypto_fee_ingestion_cursor(&pool, &engine_id, "PAPER", "CFEE")
        .await
        .expect("cursor fetch must succeed");
    assert_eq!(
        cursor, None,
        "G05: the cursor must never advance when the attempt failed closed"
    );
    let stored = mqk_db::fetch_crypto_fee_activity(&pool, &good_id)
        .await
        .expect("fetch must succeed");
    assert!(
        stored.is_none(),
        "G05: the valid activity in a failed batch must never be partially ingested"
    );
}
