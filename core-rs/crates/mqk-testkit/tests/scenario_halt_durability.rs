//! Scenario: Dispatch Halt Durability — DISPATCH-HALT-DURABILITY-01
//!
//! # Mission
//!
//! Prove that halt-required broker error paths in dispatch.rs now persist the
//! halt durably (mandatory, not best-effort) so that:
//!   - runs.status transitions to HALTED in DB
//!   - sys_arm_state transitions to DISARMED in DB
//!   - tick() returns Err (not a silent swallow)
//!   - a fresh orchestrator for the same run_id is refused at Phase-0 HALT_GUARD
//!
//! Before this patch, all halt paths in dispatch.rs used `let _ = persist_halt_and_disarm(...)`
//! which silently dropped the persistence result.  After the patch, `.await?` propagation
//! means DB failures surface as HALT_PERSISTENCE_FAILURE rather than being swallowed.
//!
//! # Invariants under test
//!
//! **DHD-01** — AmbiguousSubmit broker error triggers mandatory halt:
//!   A broker that returns `BrokerError::AmbiguousSubmit` on submit must cause
//!   tick() to persist HALTED + DISARMED and return Err.
//!
//! **DHD-02** — AuthSession broker error triggers mandatory halt:
//!   A broker that returns `BrokerError::AuthSession` on submit must cause
//!   tick() to persist HALTED + DISARMED and return Err.
//!
//! **DHD-03** — Cancel HaltAmbiguous path triggers mandatory halt:
//!   A cancel broker that returns an AmbiguousSubmit error must cause
//!   tick() to persist HALTED + DISARMED and return Err.
//!
//! **DHD-04** — Halt is sticky across restart:
//!   After DHD-01, a fresh orchestrator for the same run_id is refused at Phase-0.
//!
//! # Test matrix
//!
//! | Test | Invariants | DB?  |
//! |------|------------|------|
//! | `dhd01_ambiguous_submit_halts_durably`      | DHD-01       | Yes (skip) |
//! | `dhd02_auth_session_halts_durably`          | DHD-02       | Yes (skip) |
//! | `dhd03_cancel_halt_ambiguous_halts_durably` | DHD-03       | Yes (skip) |
//! | `dhd04_halt_sticky_after_ambiguous_submit`  | DHD-01+DHD-04| Yes (skip) |
//!
//! Tests skip gracefully when `MQK_DATABASE_URL` is absent or DB is unreachable.
//!
//! # Execution requirement
//!
//! These tests share the singleton `runtime_leader_lease` row.  Run with
//! `--test-threads 1` to prevent lease conflicts between parallel test threads:
//!
//!     cargo test -p mqk-testkit --test scenario_dispatch_halt_durability -- --test-threads 1

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::Utc;
use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixed UUIDs — deterministic, never collide with real runs.
// ---------------------------------------------------------------------------

const DHD01_RUN_ID: &str = "d4d00001-0000-0000-0000-000000000000";
const DHD02_RUN_ID: &str = "d4d00002-0000-0000-0000-000000000000";
const DHD03_RUN_ID: &str = "d4d00003-0000-0000-0000-000000000000";
const DHD04_RUN_ID: &str = "d4d00004-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// Brokers under test
// ---------------------------------------------------------------------------

/// Broker that always returns `BrokerError::AmbiguousSubmit` on submit.
struct AmbiguousSubmitBroker;

impl BrokerAdapter for AmbiguousSubmitBroker {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Err(BrokerError::AmbiguousSubmit {
            detail: "dhd-test: simulated ambiguous submit".to_string(),
        })
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
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

/// Broker that returns `BrokerError::AuthSession` on submit.
struct AuthSessionBroker;

impl BrokerAdapter for AuthSessionBroker {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Err(BrokerError::AuthSession {
            detail: "dhd-test: simulated auth session failure".to_string(),
        })
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
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

/// Broker where cancel returns `BrokerError::AmbiguousSubmit` (triggering HaltAmbiguous).
struct AmbiguousCancelBroker;

impl BrokerAdapter for AmbiguousCancelBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        _id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        Err(BrokerError::AmbiguousSubmit {
            detail: "dhd-test: simulated ambiguous cancel".to_string(),
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
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

// ---------------------------------------------------------------------------
// Gate stubs
// ---------------------------------------------------------------------------

struct BoolGate(bool);

impl IntegrityGate for BoolGate {
    fn is_armed(&self) -> bool {
        self.0
    }
}
impl RiskGate for BoolGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        if self.0 {
            mqk_execution::RiskDecision::Allow
        } else {
            mqk_execution::RiskDecision::Deny(mqk_execution::RiskDenial {
                reason: mqk_execution::RiskReason::RiskEngineUnavailable,
                evidence: mqk_execution::RiskEvidence::default(),
            })
        }
    }
}
impl ReconcileGate for BoolGate {
    fn is_clean(&self) -> bool {
        self.0
    }
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

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

async fn seed_running_run(pool: &PgPool, run_id: Uuid, engine_id: &str) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "dhd-test".to_string(),
            config_hash: "dhd-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "dhd-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        "delete from broker_order_map where internal_id in \
         (select idempotency_key from oms_outbox where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    // Release any runtime leader lease held for this run so parallel tests
    // do not conflict on the singleton lease row.  The holder_id format is
    // "dispatcher|host|user|pid=N|run=UUID" — match by run suffix.
    sqlx::query("delete from runtime_leader_lease where holder_id like $1")
        .bind(format!("%run={run_id}"))
        .execute(pool)
        .await?;
    Ok(())
}

/// Assert run is HALTED and arm state is DISARMED in DB.
async fn assert_halted_and_disarmed(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let run = mqk_db::fetch_run(pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "DHD: expected runs.status=HALTED, got {:?}",
        run.status
    );
    let arm = mqk_db::load_arm_state(pool).await?;
    let (state, _reason) = arm.expect("arm state must be present after halt");
    assert_eq!(
        state, "DISARMED",
        "DHD: expected sys_arm_state=DISARMED, got {state}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Orchestrator factories
// ---------------------------------------------------------------------------

fn make_ambiguous_submit_orch(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<AmbiguousSubmitBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(
        AmbiguousSubmitBroker,
        BoolGate(true),
        BoolGate(true),
        BoolGate(true),
    );
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "dhd-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

fn make_auth_session_orch(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<AuthSessionBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(
        AuthSessionBroker,
        BoolGate(true),
        BoolGate(true),
        BoolGate(true),
    );
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "dhd-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

fn make_ambiguous_cancel_orch(
    pool: PgPool,
    run_id: Uuid,
    oms_orders: BTreeMap<String, mqk_execution::oms::state_machine::OmsOrder>,
    order_map: BrokerOrderMap,
) -> ExecutionOrchestrator<AmbiguousCancelBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(
        AmbiguousCancelBroker,
        BoolGate(true),
        BoolGate(true),
        BoolGate(true),
    );
    ExecutionOrchestrator::new(
        pool,
        gateway,
        order_map,
        oms_orders,
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "dhd-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

fn make_ok_orch_for_halt_guard_check(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<AmbiguousSubmitBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    // Re-uses AmbiguousSubmitBroker but with no outbox rows left — the Phase-0
    // HALT_GUARD fires before any dispatch happens.
    make_ambiguous_submit_orch(pool, run_id)
}

// ---------------------------------------------------------------------------
// DHD-01: AmbiguousSubmit → mandatory halt + disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dhd01_ambiguous_submit_halts_durably() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = DHD01_RUN_ID.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "dhd01-ambiguous-submit").await?;

    // Seed one PENDING outbox row so Phase-1 dispatch fires.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "dhd01-order-001",
        json!({"order_id":"dhd01-order-001","symbol":"SPY","quantity":1,"side":"buy","order_type":"market"}),
    )
    .await?;

    let mut orch = make_ambiguous_submit_orch(pool.clone(), run_id);

    // tick() must return Err — the AmbiguousSubmit error propagates.
    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "DHD-01: tick() must return Err when broker returns AmbiguousSubmit"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("AmbiguousSubmit") || err_str.contains("HALT_PERSISTENCE_FAILURE"),
        "DHD-01: error must mention AmbiguousSubmit or HALT_PERSISTENCE_FAILURE, got: {err_str}"
    );

    // DB must show HALTED + DISARMED — proving persist_halt_and_disarm was mandatory.
    assert_halted_and_disarmed(&pool, run_id).await?;

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// DHD-02: AuthSession → mandatory halt + disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dhd02_auth_session_halts_durably() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = DHD02_RUN_ID.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "dhd02-auth-session").await?;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "dhd02-order-001",
        json!({"order_id":"dhd02-order-001","symbol":"SPY","quantity":1,"side":"buy","order_type":"market"}),
    )
    .await?;

    let mut orch = make_auth_session_orch(pool.clone(), run_id);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "DHD-02: tick() must return Err when broker returns AuthSession"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("AuthSession") || err_str.contains("HALT_PERSISTENCE_FAILURE"),
        "DHD-02: error must mention AuthSession or HALT_PERSISTENCE_FAILURE, got: {err_str}"
    );

    assert_halted_and_disarmed(&pool, run_id).await?;

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// DHD-03: Cancel HaltAmbiguous path → mandatory halt + disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dhd03_cancel_halt_ambiguous_halts_durably() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = DHD03_RUN_ID.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "dhd03-cancel-ambiguous").await?;

    // To reach the cancel dispatch path, we need:
    // 1. A SENT order already in OMS state (so cancel has a target)
    // 2. A PENDING cancel outbox row

    let order_id = "dhd03-order-001";
    let broker_order_id = "b-dhd03-order-001";
    let cancel_request_id = "dhd03-cancel-001";

    // Seed the original submit row and advance it to SENT so Phase-1 skip it
    // (only PENDING rows are claimed).
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        order_id,
        json!({"order_id":order_id,"symbol":"SPY","quantity":1,"side":"buy","order_type":"market"}),
    )
    .await?;
    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "dhd03-setup", Utc::now()).await?;
    assert_eq!(claimed.len(), 1, "DHD-03 setup: must claim submit row");
    mqk_db::outbox_mark_dispatching(
        &pool,
        claimed[0].row.outbox_id,
        order_id,
        "dhd03-setup",
        "dhd03-setup",
        Utc::now(),
    )
    .await?;
    mqk_db::outbox_mark_sent_with_broker_map(&pool, order_id, broker_order_id, Utc::now()).await?;

    // Seed the cancel outbox row.  The cancel parser requires "target_order_id".
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        cancel_request_id,
        json!({"request_type":"cancel","target_order_id":order_id}),
    )
    .await?;

    // Build in-memory OMS state with the order already open (ACKed).
    let mut oms_orders = BTreeMap::new();
    let mut order = mqk_execution::oms::state_machine::OmsOrder::new(order_id, "SPY", 1);
    // Advance to Open state (Pending → Ack).
    order
        .apply(
            &mqk_execution::oms::state_machine::OmsEvent::Ack,
            Some("dhd03-ack-msg"),
        )
        .expect("DHD-03 setup: OMS Ack must succeed");
    oms_orders.insert(order_id.to_string(), order);

    let mut order_map = BrokerOrderMap::new();
    order_map.register(order_id, broker_order_id);

    let mut orch = make_ambiguous_cancel_orch(pool.clone(), run_id, oms_orders, order_map);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "DHD-03: tick() must return Err when cancel returns AmbiguousSubmit"
    );

    // DB must show HALTED + DISARMED — proving the cancel halt path is now mandatory.
    assert_halted_and_disarmed(&pool, run_id).await?;

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// DHD-04: Halt is sticky — fresh orchestrator refused at Phase-0 after DHD-01
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dhd04_halt_sticky_after_ambiguous_submit() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = DHD04_RUN_ID.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "dhd04-halt-sticky").await?;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "dhd04-order-001",
        json!({"order_id":"dhd04-order-001","symbol":"SPY","quantity":1,"side":"buy","order_type":"market"}),
    )
    .await?;

    // First tick: AmbiguousSubmit → halt + disarm persisted.
    let mut orch1 = make_ambiguous_submit_orch(pool.clone(), run_id);
    let r1 = orch1.tick().await;
    assert!(r1.is_err(), "DHD-04: first tick must Err (AmbiguousSubmit)");
    assert_halted_and_disarmed(&pool, run_id).await?;

    // Second tick — fresh orchestrator, no in-memory state, all gates pass.
    // Phase-0 HALT_GUARD reads DB and must refuse.
    let mut orch2 = make_ok_orch_for_halt_guard_check(pool.clone(), run_id);
    let r2 = orch2.tick().await;
    assert!(
        r2.is_err(),
        "DHD-04: fresh orchestrator must be refused at Phase-0 HALT_GUARD"
    );
    let err2 = r2.unwrap_err().to_string();
    assert!(
        err2.contains("HALT_GUARD"),
        "DHD-04: Phase-0 error must contain HALT_GUARD, got: {err2}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
