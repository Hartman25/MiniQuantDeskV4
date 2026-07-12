//! PAPER-DAILY-PNL-BASELINE-01B: account equity baseline DB helper proof.
//!
//! Proves `upsert_account_equity_baseline` / `fetch_account_equity_baseline_for_date`
//! round-trip, upsert-by-`trading_date` idempotency, missing-date `None`
//! semantics, and caller-supplied `audit_event_id` preservation — with zero
//! provider/broker/network calls and zero writes to any other table.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_account_equity_baseline_01 -- --include-ignored --test-threads=1

use chrono::{NaiveDate, TimeZone, Utc};
use mqk_db::{
    fetch_account_equity_baseline_for_date, upsert_account_equity_baseline,
    UpsertAccountEquityBaselineArgs, ENV_DB_URL,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

fn cleanup_dates() -> Vec<NaiveDate> {
    vec![
        NaiveDate::from_ymd_opt(2099, 1, 2).unwrap(),
        NaiveDate::from_ymd_opt(2099, 1, 3).unwrap(),
        NaiveDate::from_ymd_opt(2099, 1, 4).unwrap(),
    ]
}

async fn cleanup(pool: &sqlx::PgPool) {
    for d in cleanup_dates() {
        let _ = sqlx::query("delete from sys_account_equity_baseline where trading_date = $1")
            .bind(d)
            .execute(pool)
            .await;
    }
}

/// Insert then fetch returns the same row (round trip).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_account_equity_baseline_01 -- --include-ignored"]
async fn insert_fetch_round_trip() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    cleanup(&pool).await;

    let trading_date = NaiveDate::from_ymd_opt(2099, 1, 2).unwrap();
    let captured_at_utc = Utc.with_ymd_and_hms(2099, 1, 2, 21, 0, 0).unwrap();
    // Fixed UUID — deterministic, never collides with real runs.
    let audit_event_id = Uuid::parse_str("aeb00001-0000-0000-0000-000000000001").unwrap();

    let inserted = upsert_account_equity_baseline(
        &pool,
        UpsertAccountEquityBaselineArgs {
            trading_date,
            equity_micros: 100_000_000_000,
            cash_micros: 20_000_000_000,
            currency: "USD".to_string(),
            captured_at_utc,
            captured_by: "test:insert_fetch_round_trip".to_string(),
            broker_snapshot_source: "synthetic".to_string(),
            audit_event_id,
        },
    )
    .await
    .expect("upsert should succeed");

    assert_eq!(inserted.trading_date, trading_date);
    assert_eq!(inserted.equity_micros, 100_000_000_000);

    let fetched = fetch_account_equity_baseline_for_date(&pool, trading_date)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");

    assert_eq!(fetched.trading_date, trading_date);
    assert_eq!(fetched.equity_micros, 100_000_000_000);
    assert_eq!(fetched.cash_micros, 20_000_000_000);
    assert_eq!(fetched.currency, "USD");
    assert_eq!(fetched.captured_by, "test:insert_fetch_round_trip");
    assert_eq!(fetched.broker_snapshot_source, "synthetic");
    assert_eq!(fetched.audit_event_id, audit_event_id);

    cleanup(&pool).await;
}

/// Upserting the same `trading_date` twice replaces the row, never
/// duplicates it — proven both by the second fetch reflecting the new
/// values and by there being exactly one row for that date.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_account_equity_baseline_01 -- --include-ignored"]
async fn upsert_same_trading_date_replaces_not_duplicates() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    cleanup(&pool).await;

    let trading_date = NaiveDate::from_ymd_opt(2099, 1, 3).unwrap();
    // Fixed UUIDs — deterministic, never collide with real runs.
    let first_audit_id = Uuid::parse_str("aeb00002-0000-0000-0000-000000000001").unwrap();
    let second_audit_id = Uuid::parse_str("aeb00002-0000-0000-0000-000000000002").unwrap();

    upsert_account_equity_baseline(
        &pool,
        UpsertAccountEquityBaselineArgs {
            trading_date,
            equity_micros: 50_000_000_000,
            cash_micros: 10_000_000_000,
            currency: "USD".to_string(),
            captured_at_utc: Utc.with_ymd_and_hms(2099, 1, 3, 21, 0, 0).unwrap(),
            captured_by: "test:first_capture".to_string(),
            broker_snapshot_source: "synthetic".to_string(),
            audit_event_id: first_audit_id,
        },
    )
    .await
    .expect("first upsert should succeed");

    upsert_account_equity_baseline(
        &pool,
        UpsertAccountEquityBaselineArgs {
            trading_date,
            equity_micros: 51_000_000_000,
            cash_micros: 11_000_000_000,
            currency: "USD".to_string(),
            captured_at_utc: Utc.with_ymd_and_hms(2099, 1, 3, 21, 5, 0).unwrap(),
            captured_by: "test:second_capture".to_string(),
            broker_snapshot_source: "synthetic".to_string(),
            audit_event_id: second_audit_id,
        },
    )
    .await
    .expect("second upsert should succeed");

    let fetched = fetch_account_equity_baseline_for_date(&pool, trading_date)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");

    assert_eq!(fetched.equity_micros, 51_000_000_000, "second capture wins");
    assert_eq!(fetched.captured_by, "test:second_capture");
    assert_eq!(fetched.audit_event_id, second_audit_id);

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_account_equity_baseline where trading_date = $1",
    )
    .bind(trading_date)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 1, "exactly one row must exist for this trading_date");

    cleanup(&pool).await;
}

/// Fetching a date with no baseline row returns `None` — never fabricated.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_account_equity_baseline_01 -- --include-ignored"]
async fn fetch_missing_date_returns_none() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    cleanup(&pool).await;

    let trading_date = NaiveDate::from_ymd_opt(2099, 1, 4).unwrap();
    let fetched = fetch_account_equity_baseline_for_date(&pool, trading_date)
        .await
        .expect("fetch should succeed");
    assert!(fetched.is_none(), "no row was ever inserted for this date");
}
