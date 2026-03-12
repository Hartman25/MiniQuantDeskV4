//! BRK-02R — Hostile broker harness proofs.
//!
//! Adds deterministic adversarial broker scenarios that validate runtime safety
//! invariants under ugly lifecycle behavior.

use anyhow::Result;
use chrono::Utc;
use mqk_db::FixedClock;
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
    BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate, Side,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const RUN_ID_STR: &str = "02020001-0000-0000-0000-000000000000";

#[derive(Clone)]
struct HostileBroker {
    state: Arc<Mutex<BrokerState>>,
}

struct BrokerState {
    scenario: HostileScenario,
    submit_mode: SubmitMode,
    fetch_calls: usize,
    last_order_id: Option<String>,
}

#[derive(Clone, Copy)]
enum SubmitMode {
    Accept,
    TimeoutAfterAccept,
}

#[derive(Clone, Copy)]
enum HostileScenario {
    DelayedAckAfterSubmit,
    DuplicateFillsDifferentEnvelope,
    CancelFillRace,
    ReplaceFillRace,
    ReplayAfterCursorLoss,
    NoEvents,
}

impl HostileBroker {
    fn new(scenario: HostileScenario, submit_mode: SubmitMode) -> Self {
        Self {
            state: Arc::new(Mutex::new(BrokerState {
                scenario,
                submit_mode,
                fetch_calls: 0,
                last_order_id: None,
            })),
        }
    }
}

impl BrokerAdapter for HostileBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        let mut s = self.state.lock().expect("broker mutex poisoned");
        s.last_order_id = Some(req.order_id.clone());
        match s.submit_mode {
            SubmitMode::Accept => Ok(BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1,
                status: "ok".to_string(),
            }),
            SubmitMode::TimeoutAfterAccept => Err(BrokerError::AmbiguousSubmit {
                detail: "timeout-after-accept: request sent, response lost".to_string(),
            }),
        }
    }

    fn cancel_order(
        &self,
        id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
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
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        let mut s = self.state.lock().expect("broker mutex poisoned");
        s.fetch_calls += 1;
        let order_id = s
            .last_order_id
            .clone()
            .unwrap_or_else(|| "ord-missing".to_string());

        let events = match s.scenario {
            HostileScenario::DelayedAckAfterSubmit => {
                if s.fetch_calls == 2 {
                    vec![BrokerEvent::Ack {
                        broker_message_id: "02r-delayed-ack-1".to_string(),
                        internal_order_id: order_id,
                        broker_order_id: Some("broker-delayed".to_string()),
                    }]
                } else {
                    vec![]
                }
            }
            HostileScenario::DuplicateFillsDifferentEnvelope => {
                if s.fetch_calls == 1 {
                    vec![
                        BrokerEvent::Fill {
                            broker_message_id: "02r-dup-fill-1".to_string(),
                            internal_order_id: order_id.clone(),
                            broker_order_id: Some("broker-a".to_string()),
                            symbol: "SPY".to_string(),
                            side: Side::Buy,
                            delta_qty: 10,
                            price_micros: 500_000_000,
                            fee_micros: 0,
                        },
                        BrokerEvent::Fill {
                            broker_message_id: "02r-dup-fill-2".to_string(),
                            internal_order_id: order_id,
                            broker_order_id: Some("broker-b".to_string()),
                            symbol: "SPY".to_string(),
                            side: Side::Buy,
                            delta_qty: 10,
                            price_micros: 500_000_000,
                            fee_micros: 0,
                        },
                    ]
                } else {
                    vec![]
                }
            }
            HostileScenario::CancelFillRace => {
                if s.fetch_calls == 1 {
                    vec![
                        BrokerEvent::CancelAck {
                            broker_message_id: "02r-cancel-ack-1".to_string(),
                            internal_order_id: order_id.clone(),
                            broker_order_id: Some("broker-race".to_string()),
                        },
                        BrokerEvent::Fill {
                            broker_message_id: "02r-cancel-fill-1".to_string(),
                            internal_order_id: order_id,
                            broker_order_id: Some("broker-race".to_string()),
                            symbol: "SPY".to_string(),
                            side: Side::Buy,
                            delta_qty: 10,
                            price_micros: 500_000_000,
                            fee_micros: 0,
                        },
                    ]
                } else {
                    vec![]
                }
            }
            HostileScenario::ReplaceFillRace => {
                if s.fetch_calls == 1 {
                    vec![
                        BrokerEvent::Fill {
                            broker_message_id: "02r-replace-fill-1".to_string(),
                            internal_order_id: order_id.clone(),
                            broker_order_id: Some("broker-replace".to_string()),
                            symbol: "SPY".to_string(),
                            side: Side::Buy,
                            delta_qty: 10,
                            price_micros: 500_000_000,
                            fee_micros: 0,
                        },
                        BrokerEvent::ReplaceAck {
                            broker_message_id: "02r-replace-ack-1".to_string(),
                            internal_order_id: order_id,
                            broker_order_id: Some("broker-replace".to_string()),
                            new_total_qty: 0,
                        },
                    ]
                } else {
                    vec![]
                }
            }
            HostileScenario::ReplayAfterCursorLoss => {
                if cursor.is_none() {
                    vec![BrokerEvent::Fill {
                        broker_message_id: "02r-replay-fill-1".to_string(),
                        internal_order_id: order_id,
                        broker_order_id: Some("broker-replay".to_string()),
                        symbol: "SPY".to_string(),
                        side: Side::Buy,
                        delta_qty: 10,
                        price_micros: 500_000_000,
                        fee_micros: 0,
                    }]
                } else {
                    vec![]
                }
            }
            HostileScenario::NoEvents => vec![],
        };

        let next_cursor = match s.scenario {
            HostileScenario::ReplayAfterCursorLoss if !events.is_empty() => {
                Some("02r-cursor-1".to_string())
            }
            _ if !events.is_empty() => Some(format!("02r-cursor-{}", s.fetch_calls)),
            _ => cursor.map(|c| c.to_string()),
        };

        Ok((events, next_cursor))
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

async fn require_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("PROOF: cannot connect to DB: {e}"))
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
            engine_id: "brk-02r".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "brk-02r".to_string(),
            config_hash: "brk-02r".to_string(),
            config_json: json!({}),
            host_fingerprint: "brk-02r".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn enqueue_order(pool: &PgPool, run_id: Uuid, idem: &str) -> Result<()> {
    let created = mqk_db::outbox_enqueue(
        pool,
        run_id,
        idem,
        json!({
            "symbol": "SPY",
            "qty": 10,
            "side": "buy",
            "order_type": "market",
            "time_in_force": "day"
        }),
    )
    .await?;
    assert!(created, "outbox order must be created");
    Ok(())
}

fn make_orchestrator(
    pool: PgPool,
    run_id: Uuid,
    broker: HostileBroker,
    broker_cursor: Option<String>,
    oms_orders: BTreeMap<String, OmsOrder>,
    portfolio: PortfolioState,
) -> ExecutionOrchestrator<HostileBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        oms_orders,
        portfolio,
        run_id,
        "brk-02r-dispatcher",
        "test",
        broker_cursor,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

async fn outbox_status(pool: &PgPool, idem: &str) -> Result<String> {
    let row = mqk_db::outbox_fetch_by_idempotency_key(pool, idem).await?;
    Ok(row.expect("outbox row must exist").status)
}

async fn inbox_count(pool: &PgPool, run_id: Uuid, msg_id: &str) -> Result<i64> {
    let count = sqlx::query_scalar::<_, i64>(
        "select count(*) from oms_inbox where run_id = $1 and broker_message_id = $2",
    )
    .bind(run_id)
    .bind(msg_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

#[tokio::test]
async fn delayed_ack_after_submit_preserves_outbox_and_state() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "02r-delayed-ack-ord";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue_order(&pool, run_id, idem).await?;

    let broker = HostileBroker::new(HostileScenario::DelayedAckAfterSubmit, SubmitMode::Accept);
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        broker,
        None,
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
    );

    orch.tick().await?;
    assert_eq!(outbox_status(&pool, idem).await?, "SENT");
    assert_eq!(orch.portfolio().ledger.len(), 0);

    orch.tick().await?;
    assert_eq!(inbox_count(&pool, run_id, "02r-delayed-ack-1").await?, 1);
    assert_eq!(orch.portfolio().ledger.len(), 0);
    assert!(orch.portfolio().positions.is_empty());

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
async fn duplicate_fills_different_envelopes_do_not_double_mutate_portfolio() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "02r-dup-fill-ord";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue_order(&pool, run_id, idem).await?;

    let broker = HostileBroker::new(
        HostileScenario::DuplicateFillsDifferentEnvelope,
        SubmitMode::Accept,
    );
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        broker,
        None,
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
    );

    orch.tick().await?;

    assert_eq!(outbox_status(&pool, idem).await?, "SENT");
    assert_eq!(inbox_count(&pool, run_id, "02r-dup-fill-1").await?, 1);
    assert_eq!(inbox_count(&pool, run_id, "02r-dup-fill-2").await?, 1);
    assert_eq!(orch.portfolio().ledger.len(), 1, "only one fill may apply");
    assert_eq!(
        orch.portfolio()
            .positions
            .get("SPY")
            .map(|p| p.qty_signed())
            .unwrap_or(0),
        10
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
async fn cancel_fill_race_halts_without_portfolio_corruption() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "02r-cancel-race-ord";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;
    enqueue_order(&pool, run_id, idem).await?;

    let broker = HostileBroker::new(HostileScenario::CancelFillRace, SubmitMode::Accept);
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        broker,
        None,
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
    );

    let err = orch.tick().await.expect_err("cancel/fill race must halt");
    assert!(err.to_string().contains("OMS transition error"));

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));
    assert_eq!(
        mqk_db::load_arm_state(&pool).await?.map(|x| x.0),
        Some("DISARMED".into())
    );
    assert_eq!(outbox_status(&pool, idem).await?, "SENT");
    assert_eq!(
        orch.portfolio().ledger.len(),
        0,
        "fill must not mutate portfolio"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
async fn replace_fill_race_halts_after_single_fill_application() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;

    let mut oms = BTreeMap::new();
    let mut ord = OmsOrder::new("ord-replace-race", "SPY", 10);
    ord.apply(&OmsEvent::ReplaceRequest, Some("pre-replace-request"))
        .expect("replace request must put order into ReplacePending");
    oms.insert("ord-replace-race".to_string(), ord);

    let broker = HostileBroker::new(HostileScenario::ReplaceFillRace, SubmitMode::Accept);
    {
        let mut s = broker.state.lock().expect("broker mutex poisoned");
        s.last_order_id = Some("ord-replace-race".to_string());
    }

    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        broker,
        None,
        oms,
        PortfolioState::new(1_000_000_000),
    );

    let err = orch.tick().await.expect_err("replace/fill race must halt");
    assert!(err.to_string().contains("OMS transition error"));

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));
    assert_eq!(
        mqk_db::load_arm_state(&pool).await?.map(|x| x.0),
        Some("DISARMED".into())
    );
    assert_eq!(
        orch.portfolio().ledger.len(),
        1,
        "only fill event may apply once"
    );
    assert_eq!(
        orch.portfolio()
            .positions
            .get("SPY")
            .map(|p| p.qty_signed())
            .unwrap_or(0),
        10
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
async fn timeout_after_accept_transitions_to_ambiguous_and_halts() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "02r-timeout-accept-ord";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;
    enqueue_order(&pool, run_id, idem).await?;

    let broker = HostileBroker::new(HostileScenario::NoEvents, SubmitMode::TimeoutAfterAccept);
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        broker,
        None,
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
    );

    let err = orch
        .tick()
        .await
        .expect_err("timeout-after-accept must halt");
    assert!(err.to_string().contains("AmbiguousSubmit"));

    assert_eq!(outbox_status(&pool, idem).await?, "AMBIGUOUS");
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));
    assert_eq!(
        mqk_db::load_arm_state(&pool).await?.map(|x| x.0),
        Some("DISARMED".into())
    );
    assert!(orch.portfolio().positions.is_empty());

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

#[tokio::test]
async fn replay_after_cursor_loss_is_deduped_and_state_safe() -> Result<()> {
    let pool = require_pool(&require_db_url()).await;
    mqk_db::migrate(&pool).await?;
    let run_id: Uuid = RUN_ID_STR.parse().unwrap();
    let idem = "02r-replay-cursor-ord";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;
    enqueue_order(&pool, run_id, idem).await?;

    let broker1 = HostileBroker::new(HostileScenario::ReplayAfterCursorLoss, SubmitMode::Accept);
    let mut orch1 = make_orchestrator(
        pool.clone(),
        run_id,
        broker1,
        None,
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000),
    );
    orch1.tick().await?;

    assert_eq!(orch1.portfolio().ledger.len(), 1);
    assert_eq!(
        mqk_db::load_broker_cursor(&pool, "test").await?,
        Some("02r-cursor-1".into())
    );

    let broker2 = HostileBroker::new(HostileScenario::ReplayAfterCursorLoss, SubmitMode::Accept);
    {
        let mut s = broker2.state.lock().expect("broker mutex poisoned");
        s.last_order_id = Some(idem.to_string());
    }
    let mut orch2 = make_orchestrator(
        pool.clone(),
        run_id,
        broker2,
        None,
        orch1.oms_orders().clone(),
        orch1.portfolio().clone(),
    );

    // Simulate cursor loss on restart by injecting `None` instead of the persisted cursor.
    orch2.tick().await?;

    assert_eq!(inbox_count(&pool, run_id, "02r-replay-fill-1").await?, 1);
    assert_eq!(
        orch2.portfolio().ledger.len(),
        1,
        "replayed fill must not double-apply"
    );
    assert_eq!(
        orch2
            .portfolio()
            .positions
            .get("SPY")
            .map(|p| p.qty_signed())
            .unwrap_or(0),
        10
    );
    assert_eq!(outbox_status(&pool, idem).await?, "SENT");

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
