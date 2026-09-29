//! D1 (V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): the actual
//! production-safe options-lifecycle ingestion caller B6's own analogous
//! `scenario_crypto_fee_activity_ingestion_caller_01.rs` pattern proves for
//! Crypto fees -- this proves the equivalent for options lifecycle.
//!
//! # Coverage
//!
//! | Test | Claim                                                               |
//! |------|-----------------------------------------------------------------------|
//! | L01  | An empty fetch result is a pure no-op                                 |
//! | L02  | A fresh OPEXC batch ingests and the cursor advances                   |
//! | L03  | A second call reads the PERSISTED cursor from the DB and threads it   |
//! |      | as `after_id`                                                          |
//! | L04  | A normalize failure (missing symbol) fails the whole attempt closed   |
//! | L05  | Ingesting OPEXC then OPTRD sharing the identical activity_id through  |
//! |      | the real caller stores BOTH rows -- the confirmed 0084 collision      |
//! |      | this migration closes, proven end-to-end through the production path |
//! | L06  | Two accounts sharing an activity_id never collide through the real    |
//! |      | caller                                                                 |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres) — load-bearing, must fail hard if
//! `MQK_DATABASE_URL` is absent, not skip. No real Alpaca call anywhere:
//! every fetcher here is a fake `OptionLifecycleActivityFetcher`.

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_daemon::state::option_lifecycle_ingestion::ingest_option_lifecycle_activities_once;
use mqk_daemon::state::OptionLifecycleActivityFetcher;
use sqlx::PgPool;
use uuid::Uuid;

const TEST_PROVIDER_ACCOUNT_ID: &str = "test-acct-primary";
/// Economic-account key (`{broker}:{provider_account_id}`), never a credential.
const TEST_BROKER_ACCOUNT_ID: &str = "alpaca:test-acct-primary";

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
    format!("test-option-lifecycle-caller-{}-{}", label, Uuid::new_v4())
}

fn opexc_activity(id: &str, option_symbol: &str, qty: &str) -> AlpacaOptionLifecycleActivity {
    AlpacaOptionLifecycleActivity {
        id: id.to_string(),
        activity_type: "OPEXC".to_string(),
        date: Some("2026-06-19".to_string()),
        net_amount: "0".to_string(),
        description: Some("Option Exercise".to_string()),
        symbol: Some(option_symbol.to_string()),
        qty: Some(qty.to_string()),
        price: None,
        status: Some("executed".to_string()),
        group_id: None,
        ref_id: None,
    }
}

fn optrd_activity(
    id: &str,
    underlying: &str,
    qty: &str,
    price: &str,
    net_amount: &str,
) -> AlpacaOptionLifecycleActivity {
    AlpacaOptionLifecycleActivity {
        id: id.to_string(),
        activity_type: "OPTRD".to_string(),
        date: Some("2026-06-19".to_string()),
        net_amount: net_amount.to_string(),
        description: Some("Option Trade".to_string()),
        symbol: Some(underlying.to_string()),
        qty: Some(qty.to_string()),
        price: Some(price.to_string()),
        status: Some("executed".to_string()),
        group_id: None,
        ref_id: None,
    }
}

fn malformed_missing_symbol(id: &str) -> AlpacaOptionLifecycleActivity {
    AlpacaOptionLifecycleActivity {
        id: id.to_string(),
        activity_type: "OPEXC".to_string(),
        date: Some("2026-06-19".to_string()),
        net_amount: "0".to_string(),
        description: None,
        symbol: None,
        qty: Some("-1".to_string()),
        price: None,
        status: Some("executed".to_string()),
        group_id: None,
        ref_id: None,
    }
}

struct FixedFetcher {
    broker_account_id: String,
    activities: Vec<AlpacaOptionLifecycleActivity>,
}

impl OptionLifecycleActivityFetcher for FixedFetcher {
    fn fetch_option_lifecycle_activities_since(
        &self,
        _activity_type: &str,
        _after_id: Option<&str>,
    ) -> Result<Vec<AlpacaOptionLifecycleActivity>, String> {
        Ok(self.activities.clone())
    }

    fn broker_account_authority(&self) -> Result<mqk_db::BrokerAccountAuthority, String> {
        mqk_db::BrokerAccountAuthority::new("alpaca", &self.broker_account_id, "paper")
            .map_err(|e| e.to_string())
    }
}

struct AssertingFetcher {
    broker_account_id: String,
    expected_after_id: Option<String>,
    activities: Vec<AlpacaOptionLifecycleActivity>,
}

impl OptionLifecycleActivityFetcher for AssertingFetcher {
    fn fetch_option_lifecycle_activities_since(
        &self,
        _activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaOptionLifecycleActivity>, String> {
        assert_eq!(
            after_id,
            self.expected_after_id.as_deref(),
            "L03: the caller must read the persisted cursor from the DB and pass it as \
             after_id -- a stale/absent cursor here means restart safety is broken"
        );
        Ok(self.activities.clone())
    }

    fn broker_account_authority(&self) -> Result<mqk_db::BrokerAccountAuthority, String> {
        mqk_db::BrokerAccountAuthority::new("alpaca", &self.broker_account_id, "paper")
            .map_err(|e| e.to_string())
    }
}

#[tokio::test]
async fn l01_empty_fetch_is_pure_noop() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l01");
    let fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![],
    };

    let outcome = ingest_option_lifecycle_activities_once(
        &pool,
        &fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("empty fetch must not error");
    assert_eq!(outcome.newly_inserted, 0);
    assert_eq!(outcome.already_existed, 0);

    let cursor = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_ingestion_cursor(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        mqk_db::option_lifecycle_activity::OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("cursor fetch must succeed");
    assert_eq!(
        cursor, None,
        "L01: an empty fetch must never create a cursor row"
    );
}

#[tokio::test]
async fn l02_fresh_opexc_batch_ingests_and_advances_cursor() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l02");
    let a1 = format!("{engine_id}::act1");

    let fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![opexc_activity(&a1, "AAPL260619C00200000", "-2")],
    };

    let outcome = ingest_option_lifecycle_activities_once(
        &pool,
        &fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("ingest must succeed");
    assert_eq!(outcome.newly_inserted, 1);

    let cursor = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_ingestion_cursor(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        mqk_db::option_lifecycle_activity::OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("cursor fetch must succeed");
    assert_eq!(cursor, Some(a1.clone()));

    let stored = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &a1,
        mqk_db::option_lifecycle_activity::OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed")
    .expect("row must exist");
    assert_eq!(stored.option_symbol.as_deref(), Some("AAPL260619C00200000"));
    assert_eq!(stored.qty_raw, "-2");
}

#[tokio::test]
async fn l03_second_call_reads_persisted_cursor_and_threads_it_as_after_id() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l03");
    let a1 = format!("{engine_id}::first");
    let a2 = format!("{engine_id}::second");

    let first_fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![opexc_activity(&a1, "AAPL260619C00200000", "-1")],
    };
    ingest_option_lifecycle_activities_once(
        &pool,
        &first_fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("first ingest must succeed");

    let second_fetcher = AssertingFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        expected_after_id: Some(a1.clone()),
        activities: vec![opexc_activity(&a2, "AAPL260619C00200000", "-1")],
    };
    let outcome = ingest_option_lifecycle_activities_once(
        &pool,
        &second_fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("second ingest must succeed");
    assert_eq!(outcome.newly_inserted, 1);
}

#[tokio::test]
async fn l04_normalize_failure_fails_closed_before_any_db_write() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l04");
    let good_id = format!("{engine_id}::good");
    let bad_id = format!("{engine_id}::bad");

    let fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![
            opexc_activity(&good_id, "AAPL260619C00200000", "-1"),
            malformed_missing_symbol(&bad_id),
        ],
    };

    let result = ingest_option_lifecycle_activities_once(
        &pool,
        &fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await;
    assert!(
        result.is_err(),
        "L04: a normalize failure anywhere in the batch must fail the whole attempt"
    );

    let cursor = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_ingestion_cursor(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        mqk_db::option_lifecycle_activity::OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("cursor fetch must succeed");
    assert_eq!(
        cursor, None,
        "L04: the cursor must never advance when the attempt failed closed"
    );
    let stored = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &good_id,
        mqk_db::option_lifecycle_activity::OptionLifecycleActivityType::Exercise,
    )
    .await
    .expect("fetch must succeed");
    assert!(
        stored.is_none(),
        "L04: the valid activity in a failed batch must never be partially ingested"
    );
}

#[tokio::test]
async fn l05_opexc_then_optrd_sharing_the_identical_id_both_persist_through_the_real_caller() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l05");
    let shared_id = format!("{engine_id}::shared");

    let opexc_fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![opexc_activity(&shared_id, "AAPL260619C00200000", "-2")],
    };
    let opexc_outcome = ingest_option_lifecycle_activities_once(
        &pool,
        &opexc_fetcher,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("OPEXC ingest must succeed");
    assert_eq!(opexc_outcome.newly_inserted, 1);

    let optrd_fetcher = FixedFetcher {
        broker_account_id: TEST_PROVIDER_ACCOUNT_ID.to_string(),
        activities: vec![optrd_activity(
            &shared_id, "AAPL", "200", "200.00", "-40000",
        )],
    };
    let optrd_outcome = ingest_option_lifecycle_activities_once(
        &pool,
        &optrd_fetcher,
        &engine_id,
        "PAPER",
        "OPTRD",
        Utc::now(),
    )
    .await
    .expect("OPTRD ingest must succeed");
    assert_eq!(
        optrd_outcome.newly_inserted, 1,
        "L05: the OPTRD row sharing the OPEXC row's exact activity_id must be genuinely new, \
         never dropped as a false duplicate through the real production caller"
    );

    let paired = mqk_db::option_lifecycle_activity::find_paired_trade_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &shared_id,
    )
    .await
    .expect("lookup must succeed")
    .expect("paired OPTRD must be found");
    assert_eq!(paired.underlying_symbol_raw.as_deref(), Some("AAPL"));
    assert_eq!(paired.net_amount_raw, "-40000");
}

#[tokio::test]
async fn l06_two_accounts_sharing_an_activity_id_never_collide_through_the_real_caller() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("l06");
    let shared_id = format!("{engine_id}::shared");

    let fetcher_a = FixedFetcher {
        broker_account_id: "acct-a".to_string(),
        activities: vec![opexc_activity(&shared_id, "AAPL260619C00200000", "-1")],
    };
    let outcome_a = ingest_option_lifecycle_activities_once(
        &pool,
        &fetcher_a,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("account A ingest must succeed");
    assert_eq!(outcome_a.newly_inserted, 1);

    let fetcher_b = FixedFetcher {
        broker_account_id: "acct-b".to_string(),
        activities: vec![opexc_activity(&shared_id, "AAPL260619C00200000", "-1")],
    };
    let outcome_b = ingest_option_lifecycle_activities_once(
        &pool,
        &fetcher_b,
        &engine_id,
        "PAPER",
        "OPEXC",
        Utc::now(),
    )
    .await
    .expect("account B ingest must succeed");
    assert_eq!(
        outcome_b.newly_inserted, 1,
        "L06: account B's activity must be genuinely new, not dropped as a false duplicate of \
         account A's identically-numbered activity"
    );
}
