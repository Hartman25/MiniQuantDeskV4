//! 0092: durable provider-account binding of Paper portfolio snapshots.
//!
//! All tests require `MQK_DATABASE_URL` (disposable test Postgres) and are
//! `#[ignore]`; every assertion observes real rows, and a missing DB URL fails
//! loudly instead of skipping silently.
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_paper_portfolio_snapshot_account_01 -- --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    bind_paper_portfolio_snapshot_account, fetch_paper_portfolio_snapshot_account,
    insert_or_confirm_paper_portfolio_snapshot, insert_run,
    verify_or_register_broker_account_authority, BindSnapshotAccountOutcome,
    BrokerAccountAuthority, InsertPaperPortfolioSnapshotOutcome, NewPaperPortfolioSnapshot,
    NewRun, PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
};
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    mqk_db::testkit_db_pool()
        .await
        .expect("these tests require MQK_DATABASE_URL (disposable test Postgres)")
}

fn id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.snapshot-account-binding.v1|{seed}").as_bytes(),
    )
}

fn authority(account: &str, mode: &str) -> BrokerAccountAuthority {
    BrokerAccountAuthority::new("alpaca", account, mode).unwrap()
}

async fn fixture_snapshot(pool: &sqlx::PgPool, seed: &str) -> Uuid {
    let run_id = id(&format!("run-{seed}"));
    let snapshot_id = id(&format!("snap-{seed}"));
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshot_account where snapshot_id = $1")
        .bind(snapshot_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where snapshot_id = $1")
        .bind(snapshot_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-snapshot-account-binding".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .unwrap();
    let outcome = insert_or_confirm_paper_portfolio_snapshot(
        pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
    ));
    snapshot_id
}

/// Removes the fixture so global "latest snapshot" assertions elsewhere in the
/// shared test DB are not perturbed by this file's far-future dates.
async fn cleanup(pool: &sqlx::PgPool, seed: &str) {
    let run_id = id(&format!("run-{seed}"));
    let snapshot_id = id(&format!("snap-{seed}"));
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshot_account where snapshot_id = $1")
        .bind(snapshot_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where snapshot_id = $1")
        .bind(snapshot_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn register(pool: &sqlx::PgPool, a: &BrokerAccountAuthority) {
    verify_or_register_broker_account_authority(pool, a, Utc::now())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn unbound_snapshot_reads_as_account_unverified() {
    let pool = pool().await;
    let snapshot_id = fixture_snapshot(&pool, "unbound").await;
    assert_eq!(
        fetch_paper_portfolio_snapshot_account(&pool, snapshot_id)
            .await
            .unwrap(),
        None
    );
    cleanup(&pool, "unbound").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn bind_is_set_once_idempotent_and_conflicts_on_a_different_account() {
    let pool = pool().await;
    let snapshot_id = fixture_snapshot(&pool, "bind").await;
    let a = authority("bbbbbbbb-0000-4000-8000-0000000000a1", "paper");
    let b = authority("bbbbbbbb-0000-4000-8000-0000000000b2", "paper");
    register(&pool, &a).await;
    register(&pool, &b).await;

    assert_eq!(
        bind_paper_portfolio_snapshot_account(&pool, snapshot_id, &a, Utc::now())
            .await
            .unwrap(),
        BindSnapshotAccountOutcome::Bound
    );
    assert_eq!(
        fetch_paper_portfolio_snapshot_account(&pool, snapshot_id)
            .await
            .unwrap(),
        Some(a.key())
    );
    assert_eq!(
        bind_paper_portfolio_snapshot_account(&pool, snapshot_id, &a, Utc::now())
            .await
            .unwrap(),
        BindSnapshotAccountOutcome::AlreadyBound
    );
    assert_eq!(
        bind_paper_portfolio_snapshot_account(&pool, snapshot_id, &b, Utc::now())
            .await
            .unwrap(),
        BindSnapshotAccountOutcome::Conflict {
            existing_authority_key: a.key()
        }
    );
    assert_eq!(
        fetch_paper_portfolio_snapshot_account(&pool, snapshot_id)
            .await
            .unwrap(),
        Some(a.key()),
        "a conflicting bind must not overwrite"
    );
    cleanup(&pool, "bind").await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn bind_requires_a_registered_authority_and_an_existing_snapshot() {
    let pool = pool().await;
    let snapshot_id = fixture_snapshot(&pool, "fk").await;
    let unregistered = authority("bbbbbbbb-0000-4000-8000-0000000000c3", "paper");
    sqlx::query("delete from sys_broker_account_authority where authority_key = $1")
        .bind(unregistered.key())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        bind_paper_portfolio_snapshot_account(&pool, snapshot_id, &unregistered, Utc::now())
            .await
            .is_err(),
        "an unregistered authority cannot be bound"
    );
    assert_eq!(
        fetch_paper_portfolio_snapshot_account(&pool, snapshot_id)
            .await
            .unwrap(),
        None
    );

    let registered = authority("bbbbbbbb-0000-4000-8000-0000000000d4", "paper");
    register(&pool, &registered).await;
    assert!(
        bind_paper_portfolio_snapshot_account(&pool, id("no-such-snapshot"), &registered, Utc::now())
            .await
            .is_err(),
        "a nonexistent snapshot cannot be bound"
    );
    cleanup(&pool, "fk").await;
}
