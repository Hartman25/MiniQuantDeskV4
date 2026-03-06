//! Patch 2 — restart quarantine for SENT rows that have no broker-map evidence.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    IntegrityGate, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

const RUN_ID_STR: &str = "29200002-0000-0000-0000-000000000000";

#[derive(Clone, Default)]
struct CountingBroker {
    submits: Arc<AtomicUsize>,
}

impl CountingBroker {
    fn submit_count(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }
}

impl BrokerAdapter for CountingBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("broker-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Ok(BrokerCancelResponse {
            broker_order_id: id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<Vec<mqk_execution::BrokerEvent>, Box<dyn std::error::Error>> {
        Ok(vec![])
    }
}

struct PassGate;

impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn is_allowed(&self) -> bool {
        true
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: requires MQK_DATABASE_URL");
            None
        }
    }
}

async fn try_pool_or_skip(url: &str) -> Result<Option<PgPool>> {
    match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
    {
        Ok(pool) => Ok(Some(pool)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "patch2-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "patch2-test".to_string(),
            config_hash: "patch2-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "patch2-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

fn make_orchestrator(
    pool: PgPool,
    run_id: Uuid,
    broker: CountingBroker,
) -> ExecutionOrchestrator<CountingBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
        run_id,
        "patch2-dispatcher",
        FixedClock::new(Utc::now()),
    )
}

#[tokio::test]
async fn restart_quarantines_sent_row_without_broker_map_and_refuses_dispatch() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "patch2-sent-ord-001";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;

    seed_running_run(&pool, run_id).await?;

    let created = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        idem,
        json!({
            "symbol": "SPY",
            "quantity": 1,
            "order_type": "market",
            "time_in_force": "day"
        }),
    )
    .await?;
    assert!(created, "outbox row must be created");

    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "patch2-dispatcher", Utc::now()).await?;
    assert_eq!(claimed.len(), 1, "must claim the pending row");

    let sent = mqk_db::outbox_mark_sent(&pool, idem, Utc::now()).await?;
    assert!(sent, "row must transition CLAIMED -> SENT");

    // Intentionally DO NOT write broker_map_upsert().
    // This is the crash window Patch 2 is supposed to quarantine.

    let broker = CountingBroker::default();
    let broker_probe = broker.clone();

    let mut orch = make_orchestrator(pool.clone(), run_id, broker);
    let err = orch
        .tick()
        .await
        .expect_err("tick must quarantine SENT row without broker-map evidence");

    let msg = err.to_string();
    assert!(
        msg.contains("RECOVERY_QUARANTINE"),
        "error must mention RECOVERY_QUARANTINE, got: {msg}"
    );
    assert!(
        msg.contains("SENT"),
        "error must mention SENT evidence, got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "run must be HALTED after quarantine"
    );

    let arm = mqk_db::load_arm_state(&pool).await?;
    assert_eq!(
        arm,
        Some((
            "DISARMED".to_string(),
            Some("RecoveryQuarantine".to_string())
        )),
        "arm state must be DISARMED / RecoveryQuarantine"
    );

    assert_eq!(
        broker_probe.submit_count(),
        0,
        "broker submit must never be reached after quarantine"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
