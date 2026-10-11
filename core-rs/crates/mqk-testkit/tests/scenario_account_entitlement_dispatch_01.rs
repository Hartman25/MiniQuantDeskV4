//! Broker-account entitlement at the REAL dispatch seam: a claimed durable
//! outbox row, the real `ExecutionOrchestrator::tick`, the real
//! `BrokerGateway`. The broker double counts `submit_order` calls so "never
//! reached the broker" is observed, not assumed.
//!
//! D1  Refusing entitlement authority: outbox row ends FAILED, the broker is
//!     never invoked, the run is not halted.
//! D2  Positive control: admitting authority: the same row reaches the broker
//!     exactly once and ends SENT.
//! D3  Durable provenance: the refusal is recorded as ONE
//!     `ORDER_ACCOUNT_ENTITLEMENT_REFUSED` audit event carrying run, outbox and
//!     order identity, instrument, asset class, the exact refusal code/detail
//!     and the authority's context; replay (a second tick, and a direct
//!     re-insert of the same deterministic event) adds no second row.
//! D4  Persistence failure: when the audit insert fails (forced with a trigger
//!     on the disposable DB) the order is STILL refused, FAILED, never sent,
//!     and no audit row exists.
//!
//! DB-backed and `#[ignore]`: requires `MQK_DATABASE_URL` and fails loudly
//! without it. Run with `-- --include-ignored --test-threads=1`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::{
    AccountEntitlementRefusal, AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError,
    BrokerEvent, BrokerGateway, BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, IntegrityGate, ReconcileGate,
    RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

const D1_RUN_ID: &str = "29a00001-0000-0000-0000-000000000000";
const D2_RUN_ID: &str = "29a00002-0000-0000-0000-000000000000";
const D3_RUN_ID: &str = "29a00003-0000-0000-0000-000000000000";
const D4_RUN_ID: &str = "29a00004-0000-0000-0000-000000000000";

struct PassGate;
impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

struct CountingBroker {
    refuse_with: Option<&'static str>,
    submits: Arc<AtomicU32>,
}

impl BrokerAdapter for CountingBroker {
    fn admit_account_entitlement(
        &self,
        _asset_class: Option<AssetClass>,
    ) -> Result<(), AccountEntitlementRefusal> {
        match self.refuse_with {
            Some(code) => Err(AccountEntitlementRefusal::new(code, "test refusal")
                .with_context("observed_provider_account_id", "acct-test")
                .with_context("bound_provider_account_id", "none")),
            None => Ok(()),
        }
    }
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _t: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 0,
            status: "ok".to_string(),
        })
    }
    fn cancel_order(
        &self,
        id: &str,
        _t: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        Ok(BrokerCancelResponse {
            broker_order_id: id.to_string(),
            cancelled_at: 0,
            status: "ok".to_string(),
        })
    }
    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _t: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "ok".to_string(),
        })
    }
    fn fetch_events(
        &self,
        _c: Option<&str>,
        _t: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

async fn pool() -> PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .expect("requires MQK_DATABASE_URL (disposable test Postgres)");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}

async fn drop_audit_failure_trigger(pool: &PgPool) {
    let _ = sqlx::query("drop trigger if exists test_fail_entitlement_audit on audit_events")
        .execute(pool)
        .await;
}

async fn cleanup(pool: &PgPool, run_id: Uuid) -> Result<()> {
    drop_audit_failure_trigger(pool).await;
    sqlx::query("delete from broker_order_map where internal_id like 'ent-d%'")
        .execute(pool)
        .await?;
    sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    sqlx::query("delete from runtime_leader_lease")
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "entitlement-dispatch-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

fn orchestrator(
    pool: PgPool,
    run_id: Uuid,
    broker: CountingBroker,
) -> ExecutionOrchestrator<CountingBroker, PassGate, PassGate, PassGate, FixedClock> {
    ExecutionOrchestrator::new(
        pool,
        BrokerGateway::for_test(broker, PassGate, PassGate, PassGate),
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(0),
        run_id,
        "entitlement-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

async fn outbox_status(pool: &PgPool, idem_key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
            .bind(idem_key)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(s,)| s))
}

async fn enqueue(pool: &PgPool, run_id: Uuid, idem: &str) -> Result<()> {
    let created = mqk_db::outbox_enqueue(
        pool,
        run_id,
        idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    assert!(created, "outbox row must be created");
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn d1_refusing_entitlement_fails_the_outbox_row_and_never_reaches_the_broker() -> Result<()> {
    let pool = pool().await;
    let run_id: Uuid = D1_RUN_ID.parse().unwrap();
    let idem = "ent-d1-ord-001";
    cleanup(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue(&pool, run_id, idem).await?;

    let submits = Arc::new(AtomicU32::new(0));
    let mut orch = orchestrator(
        pool.clone(),
        run_id,
        CountingBroker {
            refuse_with: Some("account_trading_blocked"),
            submits: Arc::clone(&submits),
        },
    );
    let err = orch
        .tick()
        .await
        .expect_err("an entitlement refusal must fail the tick");
    assert!(
        err.to_string().contains("account_trading_blocked"),
        "refusal code must surface: {err}"
    );
    assert_eq!(outbox_status(&pool, idem).await?.as_deref(), Some("FAILED"));
    assert_eq!(
        submits.load(Ordering::SeqCst),
        0,
        "the broker must never be invoked"
    );
    assert!(!matches!(
        mqk_db::fetch_run(&pool, run_id).await?.status,
        mqk_db::RunStatus::Halted
    ));

    orch.release_runtime_leadership().await?;
    cleanup(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn d2_admitting_entitlement_reaches_the_broker_exactly_once() -> Result<()> {
    let pool = pool().await;
    let run_id: Uuid = D2_RUN_ID.parse().unwrap();
    let idem = "ent-d2-ord-001";
    cleanup(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue(&pool, run_id, idem).await?;

    let submits = Arc::new(AtomicU32::new(0));
    let mut orch = orchestrator(
        pool.clone(),
        run_id,
        CountingBroker {
            refuse_with: None,
            submits: Arc::clone(&submits),
        },
    );
    let _ = orch.tick().await;
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert_eq!(outbox_status(&pool, idem).await?.as_deref(), Some("SENT"));

    orch.release_runtime_leadership().await?;
    cleanup(&pool, run_id).await?;
    Ok(())
}

async fn audit_rows(pool: &PgPool, run_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "select payload from audit_events \
          where run_id = $1 and topic = 'execution' \
            and event_type = 'ORDER_ACCOUNT_ENTITLEMENT_REFUSED' order by ts_utc, event_id",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn d3_refusal_is_durably_recorded_once_with_exact_provenance() -> Result<()> {
    let pool = pool().await;
    let run_id: Uuid = D3_RUN_ID.parse().unwrap();
    let idem = "ent-d3-ord-001";
    cleanup(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue(&pool, run_id, idem).await?;

    let submits = Arc::new(AtomicU32::new(0));
    let mut orch = orchestrator(
        pool.clone(),
        run_id,
        CountingBroker {
            refuse_with: Some("account_trading_blocked"),
            submits: Arc::clone(&submits),
        },
    );
    let err = orch.tick().await.expect_err("refusal fails the tick");
    assert!(err.to_string().contains("account_trading_blocked"));
    assert_eq!(submits.load(Ordering::SeqCst), 0);

    let rows = audit_rows(&pool, run_id).await;
    assert_eq!(
        rows.len(),
        1,
        "exactly one durable refusal record: {rows:?}"
    );
    let p = &rows[0];
    let outbox_id: i64 =
        sqlx::query_scalar("select outbox_id from oms_outbox where idempotency_key = $1")
            .bind(idem)
            .fetch_one(&pool)
            .await?;
    assert_eq!(p["schema_version"], 1);
    assert_eq!(p["run_id"], run_id.to_string());
    assert_eq!(p["order_id"], idem);
    assert_eq!(p["outbox_id"], outbox_id);
    assert_eq!(p["symbol"], "SPY");
    assert_eq!(p["side"], "buy");
    assert_eq!(p["asset_class"], "equity");
    assert_eq!(p["refusal_code"], "account_trading_blocked");
    assert_eq!(p["refusal_detail"], "test refusal");
    assert_eq!(p["context"]["observed_provider_account_id"], "acct-test");
    assert_eq!(p["context"]["bound_provider_account_id"], "none");
    let text = p.to_string().to_ascii_lowercase();
    for forbidden in ["secret", "api_key", "password", "token"] {
        assert!(
            !text.contains(forbidden),
            "no credential material in {text}"
        );
    }

    // Replay 1: another tick does not re-claim the FAILED row or add a record.
    let _ = orch.tick().await;
    assert_eq!(audit_rows(&pool, run_id).await.len(), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 0);

    // Replay 2: re-inserting the same deterministic event is a no-op.
    let event_id: Uuid = sqlx::query_scalar(
        "select event_id from audit_events \
          where run_id = $1 and event_type = 'ORDER_ACCOUNT_ENTITLEMENT_REFUSED'",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await?;
    let again = mqk_db::NewAuditEvent {
        event_id,
        run_id,
        ts_utc: Utc::now(),
        topic: "execution".to_string(),
        event_type: "ORDER_ACCOUNT_ENTITLEMENT_REFUSED".to_string(),
        payload: json!({"replayed": true}),
        hash_prev: None,
        hash_self: None,
    };
    assert!(!mqk_db::insert_audit_event_if_absent(&pool, &again).await?);
    let rows = audit_rows(&pool, run_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["refusal_code"], "account_trading_blocked",
        "existing row untouched"
    );

    // An event for a run that does not exist cannot be written: the foreign
    // key makes a persistence error loud, never silent.
    let orphan = mqk_db::NewAuditEvent {
        run_id: Uuid::from_u128(0xdead),
        event_id: Uuid::from_u128(0xbeef),
        ..again
    };
    assert!(mqk_db::insert_audit_event_if_absent(&pool, &orphan)
        .await
        .is_err());

    orch.release_runtime_leadership().await?;
    cleanup(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn d4_audit_persistence_failure_never_weakens_the_refusal() -> Result<()> {
    let pool = pool().await;
    let run_id: Uuid = D4_RUN_ID.parse().unwrap();
    let idem = "ent-d4-ord-001";
    cleanup(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue(&pool, run_id, idem).await?;

    sqlx::query(
        "create or replace function test_fail_entitlement_audit() returns trigger \
         language plpgsql as $$ begin \
           if new.event_type = 'ORDER_ACCOUNT_ENTITLEMENT_REFUSED' then \
             raise exception 'test: forced audit persistence failure'; end if; \
           return new; end $$",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "create trigger test_fail_entitlement_audit before insert on audit_events \
         for each row execute function test_fail_entitlement_audit()",
    )
    .execute(&pool)
    .await?;

    let submits = Arc::new(AtomicU32::new(0));
    let mut orch = orchestrator(
        pool.clone(),
        run_id,
        CountingBroker {
            refuse_with: Some("account_blocked"),
            submits: Arc::clone(&submits),
        },
    );
    let err = orch.tick().await.expect_err("still refused");
    assert!(err.to_string().contains("account_blocked"), "{err}");
    drop_audit_failure_trigger(&pool).await;

    assert_eq!(outbox_status(&pool, idem).await?.as_deref(), Some("FAILED"));
    assert_eq!(submits.load(Ordering::SeqCst), 0, "broker never invoked");
    assert!(
        audit_rows(&pool, run_id).await.is_empty(),
        "no record could be written"
    );

    orch.release_runtime_leadership().await?;
    cleanup(&pool, run_id).await?;
    Ok(())
}
