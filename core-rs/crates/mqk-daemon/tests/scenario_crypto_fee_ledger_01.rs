//! B6 fee economic-consumer closure: confirmed Crypto broker fees reach the
//! canonical ledger, exactly once, without inventing attribution.
//!
//! | Test | Claim                                                                  |
//! |------|------------------------------------------------------------------------|
//! | F01  | A confirmed cash fee lowers ledger cash by exactly the signed amount   |
//! | F02  | Rebuilding the ledger from the durable rows never double-charges       |
//! | F03  | An asset-denominated fee is NOT converted to cash and marks cost truth  |
//! |      | partial (never proven zero)                                             |
//! | F04  | No fee evidence at all is NOT proven-zero cost                          |
//! | F05  | A confirmed zero fee is fully attributed and moves nothing              |
//! | F06  | Two accounts of ONE deployment mode never share fees; no account => refuse |
//! | F07  | Run construction actually calls the consumer for the Crypto domain      |
//!
//! DB-backed (port 5434 test Postgres); unique deployment-mode label per test.

use chrono::Utc;
use mqk_daemon::state::crypto_fee_ledger::replay_crypto_fees_into_portfolio;
use mqk_db::{
    insert_crypto_fee_activity_if_new, verify_or_register_broker_account_authority,
    BrokerAccountAuthority, CryptoFeeAttributionStatus, NewCryptoFeeActivity,
};
use mqk_portfolio::PortfolioState;
use sqlx::PgPool;
use uuid::Uuid;

const START_CASH: i64 = 1_000_000_000_000;

async fn require_pool() -> PgPool {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!("PROOF: MQK_DATABASE_URL is not set; load-bearing proof cannot be skipped"),
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}

/// A fresh provider account under a unique deployment-mode label.
async fn fresh_account(pool: &PgPool) -> BrokerAccountAuthority {
    let u = Uuid::new_v4().simple().to_string();
    register_account(pool, &format!("m{u}")).await
}

/// A fresh provider account under exactly `mode` (two calls with the same
/// `mode` yield two DISTINCT accounts of one deployment mode).
async fn register_account(pool: &PgPool, mode: &str) -> BrokerAccountAuthority {
    let u = Uuid::new_v4().simple().to_string();
    let a = BrokerAccountAuthority::new("alpaca", &format!("fee-{u}"), mode).unwrap();
    verify_or_register_broker_account_authority(pool, &a, Utc::now())
        .await
        .unwrap();
    a
}

async fn add_fee(
    pool: &PgPool,
    key: &str,
    id: &str,
    status: CryptoFeeAttributionStatus,
    fee_micros: Option<i64>,
    qty_raw: Option<&str>,
) {
    insert_crypto_fee_activity_if_new(
        pool,
        &NewCryptoFeeActivity {
            activity_id: id.to_string(),
            broker_account_id: key.to_string(),
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            activity_type: "CFEE".to_string(),
            symbol: Some("BTCUSD".to_string()),
            attribution_status: status,
            fee_micros,
            qty_raw: qty_raw.map(str::to_string),
            ingested_at_utc: Utc::now(),
        },
    )
    .await
    .expect("insert fee");
}

#[tokio::test]
async fn f01_a_confirmed_cash_fee_lowers_cash_by_exactly_the_signed_amount() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let (mode, key) = (acct.deployment_mode().to_string(), acct.key());
    add_fee(
        &pool,
        &key,
        "a1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-50_000),
        None,
    )
    .await;
    add_fee(
        &pool,
        &key,
        "a2",
        CryptoFeeAttributionStatus::CashFee,
        Some(-12_345),
        None,
    )
    .await;

    let mut pf = PortfolioState::new(START_CASH);
    let summary = replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut pf)
        .await
        .unwrap();
    assert_eq!(pf.cash_micros, START_CASH - 50_000 - 12_345);
    assert_eq!(summary.cash_fee_count, 2);
    assert_eq!(summary.cash_fee_total_micros, -62_345);
    assert!(summary.cost_fully_attributed());
    // Each fee is its own auditable ledger entry, not a fill.
    assert_eq!(pf.ledger.len(), 2);
    assert!(pf.positions.is_empty());
}

#[tokio::test]
async fn f02_rebuilding_from_the_durable_rows_never_double_charges() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let (mode, key) = (acct.deployment_mode().to_string(), acct.key());
    add_fee(
        &pool,
        &key,
        "a1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-50_000),
        None,
    )
    .await;
    // A retried ingestion of the same activity is a DB-level no-op.
    add_fee(
        &pool,
        &key,
        "a1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-50_000),
        None,
    )
    .await;

    let mut first = PortfolioState::new(START_CASH);
    replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut first)
        .await
        .unwrap();
    let mut rebuilt = PortfolioState::new(START_CASH);
    replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut rebuilt)
        .await
        .unwrap();
    assert_eq!(first.cash_micros, START_CASH - 50_000);
    assert_eq!(rebuilt.cash_micros, first.cash_micros);
}

#[tokio::test]
async fn f03_asset_denominated_fees_are_not_invented_as_cash_and_cost_truth_is_partial() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let (mode, key) = (acct.deployment_mode().to_string(), acct.key());
    add_fee(
        &pool,
        &key,
        "asset-1",
        CryptoFeeAttributionStatus::AssetDenominatedFeeUnsupported,
        None,
        Some("-0.0002"),
    )
    .await;
    add_fee(
        &pool,
        &key,
        "cash-1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-1_000),
        None,
    )
    .await;

    let mut pf = PortfolioState::new(START_CASH);
    let summary = replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut pf)
        .await
        .unwrap();
    assert_eq!(
        pf.cash_micros,
        START_CASH - 1_000,
        "only the cash fee moves cash"
    );
    assert_eq!(summary.unattributed_asset_denominated, 1);
    assert!(
        !summary.cost_fully_attributed(),
        "an unattributed asset-denominated fee means cost truth is PARTIAL"
    );
}

#[tokio::test]
async fn f04_no_fee_evidence_is_not_proven_zero_cost() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let mode = acct.deployment_mode().to_string();
    let mut pf = PortfolioState::new(START_CASH);
    let summary = replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut pf)
        .await
        .unwrap();
    assert_eq!(pf.cash_micros, START_CASH);
    assert_eq!(summary.cash_fee_count, 0);
    assert!(
        !summary.cost_fully_attributed(),
        "absence of evidence != zero cost"
    );
}

#[tokio::test]
async fn f05_a_confirmed_zero_fee_is_fully_attributed_and_moves_nothing() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let (mode, key) = (acct.deployment_mode().to_string(), acct.key());
    add_fee(
        &pool,
        &key,
        "z1",
        CryptoFeeAttributionStatus::ConfirmedZeroFee,
        None,
        None,
    )
    .await;
    let mut pf = PortfolioState::new(START_CASH);
    let summary = replay_crypto_fees_into_portfolio(&pool, Some(&acct), &mode, &mut pf)
        .await
        .unwrap();
    assert_eq!(pf.cash_micros, START_CASH);
    assert!(summary.cost_fully_attributed());
}

#[tokio::test]
async fn f06_two_accounts_of_one_deployment_mode_never_share_fees() {
    let pool = require_pool().await;
    let mode = format!("m{}", Uuid::new_v4().simple());
    let acct_a = register_account(&pool, &mode).await;
    let acct_b = register_account(&pool, &mode).await;
    assert_eq!(acct_a.deployment_mode(), acct_b.deployment_mode());
    assert_ne!(acct_a.key(), acct_b.key());
    add_fee(
        &pool,
        &acct_a.key(),
        "a1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-7),
        None,
    )
    .await;
    add_fee(
        &pool,
        &acct_b.key(),
        "a1",
        CryptoFeeAttributionStatus::CashFee,
        Some(-9_000),
        None,
    )
    .await;

    let mut pf_a = PortfolioState::new(START_CASH);
    let sum_a = replay_crypto_fees_into_portfolio(&pool, Some(&acct_a), &mode, &mut pf_a)
        .await
        .unwrap();
    assert_eq!(
        pf_a.cash_micros,
        START_CASH - 7,
        "account B's fee never charges account A's ledger"
    );
    assert_eq!((sum_a.cash_fee_count, sum_a.cash_fee_total_micros), (1, -7));

    let mut pf_b = PortfolioState::new(START_CASH);
    let sum_b = replay_crypto_fees_into_portfolio(&pool, Some(&acct_b), &mode, &mut pf_b)
        .await
        .unwrap();
    assert_eq!(pf_b.cash_micros, START_CASH - 9_000);
    assert_eq!(
        (sum_b.cash_fee_count, sum_b.cash_fee_total_micros),
        (1, -9_000)
    );

    // No established account: the mode holds fees of some account, so the
    // replay refuses rather than guessing whose they are.
    let mut pf_none = PortfolioState::new(START_CASH);
    let refused = replay_crypto_fees_into_portfolio(&pool, None, &mode, &mut pf_none).await;
    assert!(refused.is_err(), "unresolved account must fail closed");
    assert_eq!(pf_none.cash_micros, START_CASH, "zero mutation on refusal");
}

#[tokio::test]
async fn f06b_unresolved_account_with_no_fees_under_the_mode_replays_nothing() {
    let pool = require_pool().await;
    let acct = fresh_account(&pool).await;
    let mut pf = PortfolioState::new(START_CASH);
    let summary = replay_crypto_fees_into_portfolio(&pool, None, acct.deployment_mode(), &mut pf)
        .await
        .unwrap();
    assert_eq!(pf.cash_micros, START_CASH);
    assert!(!summary.cost_fully_attributed());
}

#[test]
fn f07_run_construction_calls_the_consumer_for_the_crypto_domain() {
    let src = include_str!("../src/state/orchestrator_build.rs");
    assert!(
        src.contains("replay_crypto_fees_into_portfolio("),
        "the orchestrator build must consume the durable fee ledger for Crypto runs"
    );
}
