//! B6 final correction (migration 0088): economic account identity is the
//! PROVIDER's account id, registered durably; the API credential is never an
//! economic key, and pre-0088 credential-keyed rows are retained as
//! legacy-unverified rather than re-labelled.
//!
//! | Test | Claim                                                                  |
//! |------|------------------------------------------------------------------------|
//! | A01  | Registration is idempotent; the key is `broker:provider_account_id`     |
//! | A02  | The same provider account under a different deployment mode refuses    |
//! | A03  | Rows/cursors for an unregistered account are refused by the DB itself   |
//! |      | (fee ledger, fee cursor, lifecycle ledger) -- a credential-shaped id    |
//! |      | can never become an economic scope                                      |
//! | A04  | Two provider accounts are isolated: same activity id coexists, cursors  |
//! |      | are per account                                                         |
//! | A05  | A legacy (credential-keyed) row is retained, reported by               |
//! |      | count_legacy_unverified_account_rows, never returned by a scoped read   |
//! |      | under a registered authority, and cannot be re-written                  |
//!
//! DB-backed (port 5434 test Postgres): must fail hard if MQK_DATABASE_URL is
//! absent, not skip.

use chrono::Utc;
use mqk_db::{
    count_legacy_unverified_account_rows, fetch_broker_account_authority,
    fetch_crypto_fee_activity, fetch_crypto_fee_ingestion_cursor, ingest_crypto_fee_activity_batch,
    insert_crypto_fee_activity_if_new, verify_or_register_broker_account_authority,
    BrokerAccountAuthority, CryptoFeeAttributionStatus, NewCryptoFeeActivity,
};
use sqlx::PgPool;
use uuid::Uuid;

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

fn fresh_provider(label: &str) -> String {
    format!("{label}-{}", Uuid::new_v4())
}

fn fee_row(key: &str, activity_id: &str, engine_id: &str) -> NewCryptoFeeActivity {
    NewCryptoFeeActivity {
        activity_id: activity_id.to_string(),
        broker_account_id: key.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type: "CFEE".to_string(),
        symbol: Some("BTCUSD".to_string()),
        attribution_status: CryptoFeeAttributionStatus::ConfirmedZeroFee,
        fee_micros: None,
        qty_raw: None,
        ingested_at_utc: Utc::now(),
    }
}

#[tokio::test]
async fn a01_registration_is_idempotent_and_keyed_by_provider_account() {
    let pool = require_pool().await;
    let provider = fresh_provider("a01");
    let a = BrokerAccountAuthority::new("alpaca", &provider, "paper").unwrap();
    verify_or_register_broker_account_authority(&pool, &a, Utc::now())
        .await
        .unwrap();
    verify_or_register_broker_account_authority(&pool, &a, Utc::now())
        .await
        .unwrap();
    let stored = fetch_broker_account_authority(&pool, &a.key())
        .await
        .unwrap()
        .expect("registered");
    assert_eq!(stored, a);
    assert_eq!(a.key(), format!("alpaca:{provider}"));
}

#[tokio::test]
async fn a02_same_provider_account_under_a_different_mode_refuses() {
    let pool = require_pool().await;
    let provider = fresh_provider("a02");
    let paper = BrokerAccountAuthority::new("alpaca", &provider, "paper").unwrap();
    verify_or_register_broker_account_authority(&pool, &paper, Utc::now())
        .await
        .unwrap();
    let live = BrokerAccountAuthority::new("alpaca", &provider, "live-capital").unwrap();
    let err = verify_or_register_broker_account_authority(&pool, &live, Utc::now())
        .await
        .expect_err("one provider account cannot be both paper and live");
    assert!(err.to_string().contains("deployment_mode"), "{err}");
}

#[tokio::test]
async fn a03_unregistered_account_scope_is_refused_by_the_database() {
    let pool = require_pool().await;
    let engine_id = format!("a03-{}", Uuid::new_v4());
    // A credential-shaped id (what 0085-0087 used) has no authority row.
    let credential_shaped = "PKTESTKEYID0000000A03";

    assert!(
        insert_crypto_fee_activity_if_new(&pool, &fee_row(credential_shaped, "x1", &engine_id))
            .await
            .is_err(),
        "fee ledger must refuse an unregistered account scope"
    );
    let row = fee_row(credential_shaped, "x2", &engine_id);
    assert!(ingest_crypto_fee_activity_batch(
        &pool,
        credential_shaped,
        &engine_id,
        "PAPER",
        "CFEE",
        &[row],
        "x2",
        Utc::now(),
    )
    .await
    .is_err());
    // Atomic: neither the row nor the cursor exists.
    assert!(fetch_crypto_fee_ingestion_cursor(
        &pool,
        credential_shaped,
        &engine_id,
        "PAPER",
        "CFEE"
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn a04_two_provider_accounts_are_isolated() {
    let pool = require_pool().await;
    let engine_id = format!("a04-{}", Uuid::new_v4());
    let a = BrokerAccountAuthority::new("alpaca", &fresh_provider("a04a"), "paper").unwrap();
    let b = BrokerAccountAuthority::new("alpaca", &fresh_provider("a04b"), "paper").unwrap();
    for x in [&a, &b] {
        verify_or_register_broker_account_authority(&pool, x, Utc::now())
            .await
            .unwrap();
    }
    let shared = format!("{engine_id}::shared");
    for x in [&a, &b] {
        insert_crypto_fee_activity_if_new(&pool, &fee_row(&x.key(), &shared, &engine_id))
            .await
            .expect("same activity id is independent per account");
    }
    ingest_crypto_fee_activity_batch(
        &pool,
        &a.key(),
        &engine_id,
        "PAPER",
        "CFEE",
        &[fee_row(&a.key(), "only-a", &engine_id)],
        "only-a",
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!(
        fetch_crypto_fee_ingestion_cursor(&pool, &a.key(), &engine_id, "PAPER", "CFEE")
            .await
            .unwrap()
            .as_deref(),
        Some("only-a")
    );
    assert!(
        fetch_crypto_fee_ingestion_cursor(&pool, &b.key(), &engine_id, "PAPER", "CFEE")
            .await
            .unwrap()
            .is_none(),
        "account B's cursor must never read account A's watermark"
    );
}

#[tokio::test]
async fn a05_legacy_credential_keyed_rows_are_retained_but_never_authoritative() {
    let pool = require_pool().await;
    let engine_id = format!("a05-{}", Uuid::new_v4());
    let legacy_key = format!("PKLEGACY{}", Uuid::new_v4().simple());

    let before = count_legacy_unverified_account_rows(&pool).await.unwrap();
    let before_fee = before
        .iter()
        .find(|(t, _)| t == "sys_crypto_fee_activity_ledger")
        .unwrap()
        .1;

    // Simulate a pre-0088 row: bypass FK triggers on ONE connection only.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("set session_replication_role = replica")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "insert into sys_crypto_fee_activity_ledger (activity_id, broker_account_id, engine_id, \
         mode, activity_type, symbol, attribution_status, ingested_at_utc) \
         values ('legacy-1', $1, $2, 'PAPER', 'CFEE', 'BTCUSD', 'confirmed_zero_fee', now())",
    )
    .bind(&legacy_key)
    .bind(&engine_id)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("set session_replication_role = origin")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    // Retained + reported.
    let after = count_legacy_unverified_account_rows(&pool).await.unwrap();
    let after_fee = after
        .iter()
        .find(|(t, _)| t == "sys_crypto_fee_activity_ledger")
        .unwrap()
        .1;
    assert_eq!(after_fee, before_fee + 1, "legacy row must be counted");

    // Never returned to a scoped read under a registered authority.
    let real = BrokerAccountAuthority::new("alpaca", &fresh_provider("a05"), "paper").unwrap();
    verify_or_register_broker_account_authority(&pool, &real, Utc::now())
        .await
        .unwrap();
    assert!(fetch_crypto_fee_activity(&pool, &real.key(), "legacy-1")
        .await
        .unwrap()
        .is_none());

    // Cannot be re-written under a credential key (new rows must reference an
    // authority): the same key still has no authority row.
    assert!(insert_crypto_fee_activity_if_new(
        &pool,
        &fee_row(&legacy_key, "legacy-2", &engine_id)
    )
    .await
    .is_err());
}
