//! Durable provider-account binding of accepted Paper snapshots, through the
//! real acceptance seam (`accept_external_broker_snapshot_for_test`).
//!
//! DB-backed and `#[ignore]`: requires `MQK_DATABASE_URL` and fails loudly
//! without it (no silent skip). Run with `-- --include-ignored --test-threads=1`.

use chrono::{TimeZone, Utc};
use mqk_broker_alpaca::AccountEntitlementEvidence;
use mqk_daemon::state::{AppState, BrokerKind};
use mqk_schemas::{BrokerAccount, BrokerSnapshot};
use uuid::Uuid;

const ACCT: &str = "cccccccc-0000-4000-8000-000000000001";

fn run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.snapshot-provider-account-binding.v1|{seed}").as_bytes(),
    )
}

fn snapshot(captured_at_utc: chrono::DateTime<Utc>) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc,
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "100000.00".to_string(),
            currency: "USD".to_string(),
            buying_power: None,
            daytrading_buying_power: None,
        },
        orders: vec![],
        fills: vec![],
        positions: vec![],
    }
}

fn snapshot_id(captured_at_utc: chrono::DateTime<Utc>, run_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk.paper-portfolio-snapshot.v1|{}|{}|{}",
            captured_at_utc.to_rfc3339(),
            run_id,
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        )
        .as_bytes(),
    )
}

/// Removes this run's rows so global "latest snapshot" assertions elsewhere
/// in the shared test DB are never perturbed by this file's far-future dates.
async fn cleanup(pool: &sqlx::PgPool, run: Uuid) {
    for stmt in [
        "delete from sys_paper_portfolio_snapshot_account where snapshot_id in (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
        "delete from sys_paper_portfolio_accounting_state where run_id = $1",
        "delete from sys_paper_portfolio_snapshots where run_id = $1",
        "delete from runs where run_id = $1",
    ] {
        let _ = sqlx::query(stmt).bind(run).execute(pool).await;
    }
}

async fn fixture(seed: &str) -> (AppState, sqlx::PgPool, Uuid) {
    let pool = mqk_db::testkit_db_pool()
        .await
        .expect("requires MQK_DATABASE_URL (disposable test Postgres)");
    let run = run_id(seed);
    cleanup(&pool, run).await;
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run,
            engine_id: "test-snapshot-provider-account".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 4, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .unwrap();
    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());
    (state, pool, run)
}

fn evidence(id: &str) -> AccountEntitlementEvidence {
    AccountEntitlementEvidence::from_account_json(&serde_json::json!({
        "id": id, "status": "ACTIVE", "trading_blocked": false,
        "account_blocked": false, "trade_suspended_by_user": false
    }))
}

async fn register_paper(pool: &sqlx::PgPool, id: &str) {
    let authority = mqk_db::BrokerAccountAuthority::new(
        "alpaca",
        id,
        mqk_daemon::state::DeploymentMode::Paper.as_api_label(),
    )
    .unwrap();
    mqk_db::verify_or_register_broker_account_authority(pool, &authority, Utc::now())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn proven_snapshot_is_durably_bound_to_the_provider_account() {
    let (state, pool, run) = fixture("proven").await;
    register_paper(&pool, ACCT).await;
    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    state.broker_account_evidence.observe(evidence(ACCT), at);
    state.broker_account_evidence.pin_provider_account_id(ACCT);

    state
        .accept_external_broker_snapshot_for_test(snapshot(at), Some(run), None)
        .await;

    // Positive DB signal: the snapshot row exists AND its binding row exists.
    assert!(
        mqk_db::fetch_paper_portfolio_snapshot_by_id(&pool, snapshot_id(at, run))
            .await
            .unwrap()
            .is_some(),
        "snapshot must be persisted"
    );
    assert_eq!(
        mqk_db::fetch_paper_portfolio_snapshot_account(&pool, snapshot_id(at, run))
            .await
            .unwrap(),
        Some(format!("alpaca:{ACCT}"))
    );
    cleanup(&pool, run).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn unproven_account_leaves_the_snapshot_account_unverified() {
    for (seed, observed_offset_secs, pin) in [
        ("other-instant", 7, Some(ACCT)), // observation is not the capturing one
        ("unpinned", 0, None),            // run not bound to an account
        ("drifted", 0, Some("dddddddd-0000-4000-8000-000000000009")),
    ] {
        let (state, pool, run) = fixture(seed).await;
        register_paper(&pool, ACCT).await;
        let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 5, 0).unwrap();
        state.broker_account_evidence.observe(
            evidence(ACCT),
            at + chrono::Duration::seconds(observed_offset_secs),
        );
        if let Some(pin) = pin {
            state.broker_account_evidence.pin_provider_account_id(pin);
        }
        state
            .accept_external_broker_snapshot_for_test(snapshot(at), Some(run), None)
            .await;
        assert!(
            mqk_db::fetch_paper_portfolio_snapshot_by_id(&pool, snapshot_id(at, run))
                .await
                .unwrap()
                .is_some(),
            "{seed}: snapshot itself must still persist"
        );
        assert_eq!(
            mqk_db::fetch_paper_portfolio_snapshot_account(&pool, snapshot_id(at, run))
                .await
                .unwrap(),
            None,
            "{seed}: an unproven account must not be bound"
        );
        cleanup(&pool, run).await;
    }
}
