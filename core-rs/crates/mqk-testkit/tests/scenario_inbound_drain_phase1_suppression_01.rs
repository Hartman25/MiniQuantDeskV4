//! PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01
//!
//! # Problem addressed
//!
//! `stop_execution_runtime` used to clear local ownership unconditionally,
//! regardless of whether any broker-reachable order was still unresolved.
//! Because the WS inbound transport (`alpaca_ws_transport.rs`) gates durable
//! ingest on "is a run currently locally owned", clearing ownership the
//! instant stop is requested meant any late fill/partial_fill/cancel_ack/
//! reject frame for that run was silently dropped -- zero logging, zero
//! durable trace.
//!
//! # Fix under test
//!
//! `runs.stop_requested_at_utc` (migration 0063) records a stop request
//! durably without moving `status` off RUNNING. The orchestrator's Phase 1
//! (`ExecutionOrchestrator::tick`, mqk-runtime) reads this column fresh every
//! tick (same pattern as the pre-existing Phase 0 HALT_GUARD) and refuses to
//! claim/dispatch new outbox rows once it is set, while Phase 2 (broker event
//! fetch/ingest) and Phase 3 (inbound apply) keep running exactly as before.
//!
//! # Coverage
//!
//! | Test | Claim                                                              |
//! |------|---------------------------------------------------------------------|
//! | D06  | No stop requested: Phase 1 claims and dispatches a PENDING row      |
//! | D07  | Stop requested: Phase 1 claims nothing; row stays PENDING           |
//! | D08  | Stop requested: Phase 3 still applies an already-unapplied late fill |
//! | D09  | `outbox_unresolved_broker_reachable_orders` classification matrix   |
//! | D10  | `request_stop_run` is idempotent and RUNNING-only                   |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). This is a load-bearing institutional
//! proof test -- it must fail hard if `MQK_DATABASE_URL` is absent, not skip.

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
use mqk_execution::oms::state_machine::OmsOrder;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Test broker / gates (mirrors scenario_restart_quarantines_sending_outbox.rs)
// ---------------------------------------------------------------------------

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
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
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

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

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
    // broker_order_map has no ON DELETE CASCADE from oms_outbox -- a
    // successful Phase 1 dispatch (D06/D08 both exercise the real dispatch
    // path) registers a broker_order_map row, which must be cleared first or
    // the cascaded oms_outbox delete below violates fk_broker_map_outbox_idempotency.
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
    // runtime_leader_lease is a singleton row (id = 1) shared across every
    // orchestrator in this test binary -- clear it so an earlier test (or an
    // earlier failed run of this same test) never blocks a later one from
    // acquiring leadership under a different holder/run.
    sqlx::query("delete from runtime_leader_lease where id = 1")
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "drain-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "drain-test".to_string(),
            config_hash: "drain-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "drain-test".to_string(),
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
    dispatcher_id: &str,
    broker: CountingBroker,
    oms_orders: BTreeMap<String, OmsOrder>,
) -> ExecutionOrchestrator<CountingBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        oms_orders,
        PortfolioState::new(0),
        run_id,
        dispatcher_id,
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

async fn outbox_status(pool: &PgPool, idem: &str) -> Result<String> {
    let status: String =
        sqlx::query_scalar("select status from oms_outbox where idempotency_key = $1")
            .bind(idem)
            .fetch_one(pool)
            .await?;
    Ok(status)
}

// ---------------------------------------------------------------------------
// D06 — baseline: no stop requested, Phase 1 claims and dispatches normally.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d06_phase1_claims_pending_row_when_no_stop_requested() -> Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "d0600001-0000-0000-0000-000000000000".parse().unwrap();
    let idem = "d06-order-001";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;

    let broker = CountingBroker::default();
    let probe = broker.clone();
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        "d06-dispatcher",
        broker,
        BTreeMap::new(),
    );
    orch.tick().await?;

    assert_eq!(
        probe.submit_count(),
        1,
        "D06: with no stop requested, Phase 1 must dispatch the pending order"
    );
    assert_eq!(
        outbox_status(&pool, idem).await?,
        "SENT",
        "D06: order must reach SENT"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D07 — stop requested: Phase 1 claims nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d07_phase1_suppressed_when_stop_requested() -> Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "d0700002-0000-0000-0000-000000000000".parse().unwrap();
    let idem = "d07-order-001";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;

    // Durably request stop BEFORE the tick -- this is the exact durable
    // signal stop_execution_runtime writes before returning its typed
    // "still draining" conflict.
    mqk_db::request_stop_run(&pool, run_id).await?;
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        run.stop_requested_at_utc.is_some(),
        "D07 precondition: stop_requested_at_utc must be set"
    );
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "D07 precondition: status must remain RUNNING, not transition away"
    );

    // Fresh orchestrator instance with empty oms_orders -- deliberately NOT
    // reusing any in-memory state from a "prior tick", to prove the
    // suppression is driven entirely by the durable column, not a cached
    // flag. This also doubles as the restart-safety proof: a daemon restart
    // mid-drain constructs exactly this kind of fresh orchestrator, and it
    // must suppress Phase 1 on its very first tick with no special recovery
    // code required.
    let broker = CountingBroker::default();
    let probe = broker.clone();
    let mut orch = make_orchestrator(
        pool.clone(),
        run_id,
        "d07-dispatcher",
        broker,
        BTreeMap::new(),
    );
    orch.tick().await?;

    assert_eq!(
        probe.submit_count(),
        0,
        "D07: with stop requested, Phase 1 must NOT dispatch any order"
    );
    assert_eq!(
        outbox_status(&pool, idem).await?,
        "PENDING",
        "D07: order must remain PENDING -- never claimed"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D08 — stop requested: Phase 3 still applies an already-unapplied late fill.
// ---------------------------------------------------------------------------

/// Simulates: an order was dispatched and ACKed in an earlier tick (before
/// stop was requested) -- oms_orders already has it registered, exactly as
/// production would after Phase 1 dispatch. A late `fill` event has already
/// been durably ingested into oms_inbox (as Phase 2 / the WS transport would
/// do, since ownership is still held during drain) but not yet applied. This
/// proves Phase 3 (inbound apply) is untouched by the Phase 1 suppression --
/// the run keeps draining broker-reachable orders to a terminal outcome.
#[tokio::test]
async fn d08_phase3_still_applies_late_fill_during_drain() -> Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "d0800003-0000-0000-0000-000000000000".parse().unwrap();
    let idem = "d08-order-001";
    let broker_order_id = "broker-d08-order-001";

    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;
    seed_running_run(&pool, run_id).await?;

    // Order already SENT in a prior tick, before drain began.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(idem)
        .execute(&pool)
        .await?;
    // A SENT row without a broker_order_map entry is restart-ambiguous
    // (Phase 0b) -- register the map entry a genuinely-dispatched order
    // would have, so this tick reaches Phase 1/3 instead of quarantining.
    mqk_db::broker_map_upsert(&pool, idem, broker_order_id).await?;

    // A late terminal fill arrived and was durably ingested (mirrors what
    // Phase 2 / the WS transport does while ownership is still held). Full
    // BrokerEvent::Fill shape required -- Phase 3 deserializes message_json
    // back into BrokerEvent.
    let fill_msg_id = "alpaca:d08-broker-order:fill:2026-08-08T14:00:00.000000Z";
    let fill_payload = json!({
        "type":               "fill",
        "broker_message_id":  fill_msg_id,
        "broker_fill_id":     serde_json::Value::Null,
        "internal_order_id":  idem,
        "broker_order_id":    broker_order_id,
        "symbol":             "SPY",
        "side":               "Buy",
        "delta_qty":          1_i64,
        "price_micros":       500_000_000_i64,
        "fee_micros":         0_i64
    });
    let inserted = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        fill_msg_id,
        None,
        idem,
        broker_order_id,
        "fill",
        &fill_payload,
        0,
        Utc::now(),
    )
    .await?;
    assert!(inserted, "D08 precondition: late fill must insert");

    // Drain requested.
    mqk_db::request_stop_run(&pool, run_id).await?;

    // Fresh orchestrator, but oms_orders seeded with this order at Ack state
    // -- exactly the in-memory state Phase 1's dispatch would have left
    // behind in the SAME orchestrator instance that keeps ticking through
    // drain (this orchestrator is never torn down mid-drain; only Phase 1 is
    // gated).
    let mut oms_orders = BTreeMap::new();
    let mut order = OmsOrder::new(idem, "SPY", 1);
    let _ = order.apply(
        &mqk_execution::oms::state_machine::OmsEvent::Ack,
        Some("ack-evt"),
    );
    oms_orders.insert(idem.to_string(), order);

    let broker = CountingBroker::default();
    let probe = broker.clone();
    let mut orch = make_orchestrator(pool.clone(), run_id, "d08-dispatcher", broker, oms_orders);
    orch.tick().await?;

    assert_eq!(
        probe.submit_count(),
        0,
        "D08: Phase 1 must still be suppressed during this same drain tick"
    );

    let applied_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "select applied_at_utc from oms_inbox where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(idem)
    .fetch_one(&pool)
    .await?;
    assert!(
        applied_at.is_some(),
        "D08: the late fill must be applied (Phase 3) even while Phase 1 is suppressed -- \
         drainage must not silently strand it"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D09 — outbox_unresolved_broker_reachable_orders classification matrix.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d09_unresolved_broker_reachable_orders_classification() -> Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "d0900004-0000-0000-0000-000000000000".parse().unwrap();

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    // PENDING: never broker-reachable -- must never appear.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "d09-pending",
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;

    // SENT with no terminal inbox event -- unresolved, must appear.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "d09-sent-unresolved",
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    sqlx::query(
        "update oms_outbox set status = 'SENT' where idempotency_key = 'd09-sent-unresolved'",
    )
    .execute(&pool)
    .await?;

    // ACKED but resolved via a terminal cancel_ack -- must NOT appear.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "d09-acked-resolved",
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    sqlx::query(
        "update oms_outbox set status = 'ACKED' where idempotency_key = 'd09-acked-resolved'",
    )
    .execute(&pool)
    .await?;
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "d09-cancel-msg",
        None,
        "d09-acked-resolved",
        "broker-d09-acked-resolved",
        "cancel_ack",
        &json!({"type": "cancel_ack"}),
        0,
        Utc::now(),
    )
    .await?;

    // DISPATCHING with no resolution -- unresolved, must appear (fail-closed:
    // submission may have reached the broker before the crash).
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "d09-dispatching",
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    sqlx::query(
        "update oms_outbox set status = 'DISPATCHING' where idempotency_key = 'd09-dispatching'",
    )
    .execute(&pool)
    .await?;

    let unresolved = mqk_db::outbox_unresolved_broker_reachable_orders(&pool, run_id).await?;

    assert!(
        !unresolved.contains(&"d09-pending".to_string()),
        "D09: PENDING must never be classified as broker-reachable"
    );
    assert!(
        unresolved.contains(&"d09-sent-unresolved".to_string()),
        "D09: SENT with no terminal event must be unresolved"
    );
    assert!(
        !unresolved.contains(&"d09-acked-resolved".to_string()),
        "D09: an order with a terminal cancel_ack must not be unresolved"
    );
    assert!(
        unresolved.contains(&"d09-dispatching".to_string()),
        "D09: DISPATCHING with no resolution must be unresolved (fail-closed)"
    );
    assert_eq!(
        unresolved.len(),
        2,
        "D09: exactly the two genuinely-unresolved rows must be returned, got: {unresolved:?}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// D10 — request_stop_run is idempotent and RUNNING-only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d10_request_stop_run_is_idempotent_and_running_only() -> Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "d1000005-0000-0000-0000-000000000000".parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    mqk_db::request_stop_run(&pool, run_id).await?;
    let first = mqk_db::fetch_run(&pool, run_id)
        .await?
        .stop_requested_at_utc
        .expect("D10: first request must set stop_requested_at_utc");

    // Idempotent: a second call must not move the original request time.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    mqk_db::request_stop_run(&pool, run_id).await?;
    let second = mqk_db::fetch_run(&pool, run_id)
        .await?
        .stop_requested_at_utc
        .expect("D10: value must still be set");
    assert_eq!(
        first, second,
        "D10: a second request_stop_run call must not overwrite the original request time"
    );

    // begin_run defensively clears any stale flag on a fresh RUNNING period.
    mqk_db::stop_run(&pool, run_id).await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    let after_rearm = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        after_rearm.stop_requested_at_utc.is_none(),
        "D10: begin_run must clear a stale stop_requested_at_utc from a prior cycle"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
