//! Broker-account entitlement at the REAL dispatch seam: a claimed durable
//! outbox row, the real `ExecutionOrchestrator::tick`, the real
//! `BrokerGateway`. The broker double counts `submit_order` calls so "never
//! reached the broker" is observed, not assumed.
//!
//! D1  Refusing entitlement authority: outbox row ends FAILED, the broker is
//!     never invoked, the run is not halted.
//! D2  Positive control: admitting authority: the same row reaches the broker
//!     exactly once and ends SENT.
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
            Some(code) => Err(AccountEntitlementRefusal {
                code: code.to_string(),
                detail: "test refusal".to_string(),
            }),
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

async fn cleanup(pool: &PgPool, run_id: Uuid) -> Result<()> {
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
    assert_eq!(
        outbox_status(&pool, idem).await?.as_deref(),
        Some("FAILED")
    );
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
