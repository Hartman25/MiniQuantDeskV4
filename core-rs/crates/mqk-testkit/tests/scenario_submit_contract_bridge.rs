//! Scenario: Submit contract bridge ΓÇö runtime -> execution -> broker
//!
//! These tests prove that the submit contract survives crate boundaries:
//! - runtime outbox JSON decoding/building
//! - execution broker request contract
//! - broker adapter invocation semantics

use anyhow::Result;
use chrono::Utc;
use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate, Side,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct CapturedSubmits {
    requests: Vec<BrokerSubmitRequest>,
}

struct CaptureBroker {
    captured: Arc<Mutex<CapturedSubmits>>,
}

impl BrokerAdapter for CaptureBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        self.captured
            .lock()
            .expect("poisoned")
            .requests
            .push(req.clone());
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("broker-{}", req.order_id),
            submitted_at: 0,
            status: "accepted".to_string(),
        })
    }

    fn cancel_order(
        &self,
        id: &str,
        _token: &BrokerInvokeToken,
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
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

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

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: requires MQK_DATABASE_URL");
            None
        }
    }
}

async fn connect_pool(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "submit-contract-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "submit-contract-test".to_string(),
            config_hash: "submit-contract-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "submit-contract-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn cleanup(pool: &PgPool, run_id: Uuid, adapter_id: &str) -> Result<()> {
    sqlx::query("delete from broker_event_cursor where adapter_id = $1")
        .bind(adapter_id)
        .execute(pool)
        .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn run_single_tick_with_order(order_json: serde_json::Value) -> Result<BrokerSubmitRequest> {
    let Some(url) = db_url_or_skip() else {
        return Err(anyhow::anyhow!("skip"));
    };

    let pool = connect_pool(&url).await?;
    let run_id = Uuid::new_v4();
    let adapter_id = format!("submit-contract-{}", run_id);

    cleanup(&pool, run_id, &adapter_id).await?;
    seed_running_run(&pool, run_id).await?;

    let idempotency_key = format!("ord-{}", run_id.simple());
    mqk_db::outbox_enqueue(&pool, run_id, &idempotency_key, order_json).await?;

    let captured = Arc::new(Mutex::new(CapturedSubmits::default()));
    let broker = CaptureBroker {
        captured: Arc::clone(&captured),
    };

    let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);
    let mut orch = ExecutionOrchestrator::new(
        pool.clone(),
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
        run_id,
        "submit-contract-dispatcher",
        adapter_id.clone(),
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    );

    let tick_result = orch.tick().await;
    cleanup(&pool, run_id, &adapter_id).await?;
    tick_result?;

    let captured = captured.lock().expect("poisoned");
    let req = captured
        .requests
        .first()
        .cloned()
        .expect("exactly one submit should be sent");
    Ok(req)
}

#[tokio::test]
async fn invariant_submit_path_explicit_side_keeps_direction_with_positive_qty_and_defaults() {
    let req = match run_single_tick_with_order(json!({
        "symbol": "SPY",
        "side": "buy",
        "quantity": -7
    }))
    .await
    {
        Ok(req) => req,
        Err(err) if err.to_string() == "skip" => return,
        Err(err) => panic!("test setup failed: {err}"),
    };

    assert!(matches!(req.side, Side::Buy));
    assert_eq!(
        req.quantity, 7,
        "quantity must be positive in broker request"
    );
    assert_eq!(req.order_type, "market", "legacy default must be preserved");
    assert_eq!(req.time_in_force, "day", "legacy default must be preserved");
    assert_eq!(req.symbol, "SPY");
}

#[tokio::test]
async fn invariant_submit_path_legacy_signed_quantity_without_side_maps_to_sell() {
    let req = match run_single_tick_with_order(json!({
        "symbol": "QQQ",
        "quantity": -9
    }))
    .await
    {
        Ok(req) => req,
        Err(err) if err.to_string() == "skip" => return,
        Err(err) => panic!("test setup failed: {err}"),
    };

    assert!(matches!(req.side, Side::Sell));
    assert_eq!(req.quantity, 9, "quantity must be absolute on submit");
    assert_eq!(req.order_type, "market");
    assert_eq!(req.time_in_force, "day");
    assert_eq!(req.symbol, "QQQ");
}
