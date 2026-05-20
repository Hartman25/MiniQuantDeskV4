//! RECONCILE-DRIFT-AFTER-FAST-PAPER-FILL-01 — Proof tests for the
//! pending-fill-propagation deferral in Phase 0c.
//!
//! # Root cause reproduced
//!
//! Alpaca paper market orders can fill faster than the local WS fill event is
//! processed.  After ACK inbox rows are applied, the outbox row remains SENT
//! (terminal happens on fill/cancel, not ack).  If the broker REST snapshot
//! already reflects the fill but the WS fill event has NOT yet arrived in
//! oms_inbox, Phase 0c sees:
//!   - pending_inbox.is_empty() == true  (ACK rows already applied)
//!   - local snapshot: 1 AAPL position + 1 Open AAPL order
//!   - broker snapshot: 2 AAPL positions + 0 open orders
//!     => DIRTY reconcile => false-positive RECONCILE_DRIFT halt
//!
//! # Fix proven
//!
//! Phase 0c now loads `outbox_load_sent_with_broker_map_for_run` before
//! halting on a DIRTY result.  If SENT+mapped rows exist within the grace
//! window AND the drift is directionally consistent with those pending fills,
//! the check is deferred.  The fill event either arrives (Phase 3 applies it,
//! resolving the drift) or the grace window expires (RECONCILE_DRIFT fires).
//!
//! # Test inventory
//!
//! | ID      | Scenario                                                    | Expected       |
//! |---------|-------------------------------------------------------------|----------------|
//! | RDF-01  | Dirty reconcile + SENT+mapped within grace + consistent     | tick() Ok      |
//! | RDF-02  | Same but grace expired (clock advanced)                     | tick() Err     |
//! | RDF-03  | Dirty reconcile + SENT+mapped within grace + wrong symbol   | tick() Err     |
//! | RDF-04  | Dirty reconcile + no SENT rows at all                       | tick() Err     |
//! | RDF-05  | Fill applied → subsequent reconcile clean                   | tick() Ok      |
//!
//! All tests require `MQK_DATABASE_URL`. Skipped gracefully when absent.

use anyhow::Result;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_reconcile::{BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, Side};
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs (deterministic UUIDv5-style constants)
// ---------------------------------------------------------------------------

const RDF01_RUN: &str = "d0f10001-0000-0000-0000-000000000001";
const RDF02_RUN: &str = "d0f20002-0000-0000-0000-000000000002";
const RDF03_RUN: &str = "d0f30003-0000-0000-0000-000000000003";
const RDF04_RUN: &str = "d0f40004-0000-0000-0000-000000000004";
const RDF05_RUN: &str = "d0f50005-0000-0000-0000-000000000005";

// ---------------------------------------------------------------------------
// Serialization guard — tests share the runtime lease table
// ---------------------------------------------------------------------------

static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn test_guard() -> MutexGuard<'static, ()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

struct NullBroker;

impl BrokerAdapter for NullBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("null-{}", req.order_id),
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
// Helpers
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

async fn seed_running_run(pool: &PgPool, run_id: Uuid, tag: &str) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("{tag}-test"),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: format!("{tag}-test"),
            config_hash: format!("{tag}-test"),
            config_json: json!({}),
            host_fingerprint: format!("{tag}-test"),
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
    // Clear any runtime lease rows acquired during the test so subsequent
    // test binaries (e.g. scenario_recovery_quarantine_after_ack_fill_01) can
    // acquire leases without waiting for the 90-second TTL to expire.
    clear_runtime_lease_rows(pool).await?;
    Ok(())
}

async fn clear_arm_state(pool: &PgPool) -> Result<()> {
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(pool)
        .await?;
    Ok(())
}

async fn clear_runtime_lease_rows(pool: &PgPool) -> Result<()> {
    sqlx::query(
        r#"
        do $$
        declare rec record;
        begin
            for rec in
                select c.table_schema, c.table_name
                from information_schema.columns c
                where c.table_schema = 'public'
                group by c.table_schema, c.table_name
                having
                    (bool_or(c.column_name = 'holder_id') or bool_or(c.column_name = 'current_holder'))
                    and (bool_or(c.column_name = 'current_epoch') or bool_or(c.column_name = 'epoch'))
                    and (bool_or(c.column_name = 'lease_expires_at') or bool_or(c.column_name = 'expires_at'))
            loop
                execute format('delete from %I.%I', rec.table_schema, rec.table_name);
            end loop;
        end $$;
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Seed a SENT outbox row with a broker_order_map entry directly via SQL,
/// bypassing the claim/dispatch lifecycle (test setup only).
#[allow(clippy::too_many_arguments)]
async fn seed_sent_outbox_with_map(
    pool: &PgPool,
    run_id: Uuid,
    internal_id: &str,
    broker_id: &str,
    symbol: &str,
    side: &str,
    qty: i64,
    sent_at: chrono::DateTime<Utc>,
) -> Result<()> {
    let order_json = json!({
        "symbol": symbol,
        "side": side,
        "qty": qty,
        "order_type": "market",
        "time_in_force": "day"
    });
    sqlx::query(
        r#"
        insert into oms_outbox
            (run_id, idempotency_key, order_json, status, created_at_utc, sent_at_utc)
        values ($1, $2, $3, 'SENT', $5, $4)
        on conflict (idempotency_key) do nothing
        "#,
    )
    .bind(run_id)
    .bind(internal_id)
    .bind(&order_json)
    .bind(sent_at)
    .bind(Utc::now())
    .execute(pool)
    .await?;

    sqlx::query(
        "insert into broker_order_map (internal_id, broker_id) values ($1, $2) \
         on conflict (internal_id) do update set broker_id = excluded.broker_id",
    )
    .bind(internal_id)
    .bind(broker_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Build LocalSnapshot simulating the ack→fill window for an AAPL buy order:
/// - portfolio has 1 AAPL (from a prior fill or run)
/// - OMS has 1 active AAPL buy order (just acked, not yet filled)
fn aapl_local_in_fill_window(order_id: &str) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert("AAPL".to_string(), 1);
    s.orders.insert(
        order_id.to_string(),
        OrderSnapshot::new(order_id, "AAPL", Side::Buy, 1, 0, OrderStatus::Accepted),
    );
    s
}

/// Build BrokerSnapshot simulating broker REST after fast fill:
/// - AAPL position = 2 (fill applied at broker, +1 from the order)
/// - no open orders (fill closed the order)
fn aapl_broker_post_fill(ts_ms: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty_at(ts_ms);
    s.positions.insert("AAPL".to_string(), 2);
    s
}


fn make_orchestrator_with_snapshots(
    pool: PgPool,
    run_id: Uuid,
    clock: FixedClock,
    local: LocalSnapshot,
    broker: BrokerSnapshot,
) -> ExecutionOrchestrator<NullBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(NullBroker, PassGate, PassGate, PassGate);
    let portfolio = PortfolioState::new(500_000_000);
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        portfolio,
        run_id,
        "rdf-dispatcher",
        "test-adapter",
        None,
        clock,
        Box::new(move || local.clone()),
        Box::new(move || broker.clone()),
    )
}

// ---------------------------------------------------------------------------
// RDF-01: dirty reconcile + SENT+mapped within grace + consistent direction
//         → tick defers and succeeds
// ---------------------------------------------------------------------------

/// Proves the core fix: when the broker REST snapshot already reflects a fill
/// (AAPL 1→2) and the local OMS still has the open buy order (ack applied,
/// fill not yet arrived), Phase 0c defers rather than halting because:
///   - SENT+mapped outbox row is within the 30s grace window
///   - Drift (AAPL +1) is consistent with the pending buy fill
#[tokio::test(flavor = "multi_thread")]
async fn rdf01_deferred_when_sent_mapped_in_grace_and_consistent() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RDF01_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rdf01").await?;

    let internal_id = "rdf01-ord-001";
    let sent_at = Utc::now(); // sent_at = now → elapsed ≈ 0s < 30s grace
    seed_sent_outbox_with_map(
        &pool,
        run_id,
        internal_id,
        "broker-rdf01",
        "AAPL",
        "buy",
        1,
        sent_at,
    )
    .await?;

    // Verify the row is visible as SENT+mapped.
    let sent_mapped = mqk_db::outbox_load_sent_with_broker_map_for_run(&pool, run_id).await?;
    assert_eq!(
        sent_mapped.len(),
        1,
        "RDF-01: expected 1 SENT+mapped row before tick"
    );

    // Clock set to now: row is fresh (elapsed ≈ 0s).
    let clock = FixedClock::new(Utc::now());
    let local = aapl_local_in_fill_window(internal_id);
    let broker = aapl_broker_post_fill(2_000_000_000);
    let mut orch = make_orchestrator_with_snapshots(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RDF-01: tick() must succeed when SENT+mapped row is in grace and drift is consistent; \
         got: {:?}",
        result.err()
    );

    // Run must still be RUNNING (not halted).
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "RDF-01: run must remain RUNNING after deferred reconcile tick; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RDF-02: grace expired → RECONCILE_DRIFT halt
// ---------------------------------------------------------------------------

/// After the grace window expires, RECONCILE_DRIFT fires even when a
/// SENT+mapped row exists.  This proves the deferral is bounded.
#[tokio::test(flavor = "multi_thread")]
async fn rdf02_halts_when_grace_expired() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RDF02_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rdf02").await?;

    let internal_id = "rdf02-ord-001";
    let sent_at = Utc::now(); // sent_at = now
    seed_sent_outbox_with_map(
        &pool,
        run_id,
        internal_id,
        "broker-rdf02",
        "AAPL",
        "buy",
        1,
        sent_at,
    )
    .await?;

    // Advance clock by 60s so the SENT+mapped row appears 60s old (> 30s grace).
    let clock = FixedClock::new(Utc::now() + Duration::seconds(60));
    let local = aapl_local_in_fill_window(internal_id);
    let broker = aapl_broker_post_fill(2_000_000_000);
    let mut orch = make_orchestrator_with_snapshots(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RDF-02: tick() must halt when grace window is expired; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RDF-02: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RDF-02: run must be HALTED after grace-expired reconcile; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RDF-03: unexpected broker position (wrong symbol) → halt immediately
// ---------------------------------------------------------------------------

/// Even with a SENT+mapped order within grace, if the broker shows an
/// unexpected position change on a different symbol, RECONCILE_DRIFT fires
/// immediately.  The deferral is direction-aware — it does not suppress
/// genuine unexpected drift.
#[tokio::test(flavor = "multi_thread")]
async fn rdf03_halts_immediately_on_unexpected_broker_drift() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RDF03_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rdf03").await?;

    let internal_id = "rdf03-ord-001";
    let sent_at = Utc::now(); // within grace
                              // SENT+mapped order is for AAPL buy.
    seed_sent_outbox_with_map(
        &pool,
        run_id,
        internal_id,
        "broker-rdf03",
        "AAPL",
        "buy",
        1,
        sent_at,
    )
    .await?;

    let clock = FixedClock::new(Utc::now()); // fresh clock — still in grace

    // Local: AAPL=1, open AAPL buy order
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 1);
    local.orders.insert(
        internal_id.to_string(),
        OrderSnapshot::new(internal_id, "AAPL", Side::Buy, 1, 0, OrderStatus::Accepted),
    );

    // Broker: AAPL still 1, but unexpected TSLA=5 position (not explained by any SENT order).
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 1);
    broker.positions.insert("TSLA".to_string(), 5); // unexpected — not explained by AAPL buy

    let mut orch = make_orchestrator_with_snapshots(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RDF-03: tick() must halt on unexpected broker position even within grace; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RDF-03: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RDF-03: run must be HALTED after unexpected drift; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RDF-04: no SENT+mapped rows → halt immediately (fail-closed)
// ---------------------------------------------------------------------------

/// If there are no SENT+mapped outbox rows and the inbox is empty, Phase 0c
/// fires RECONCILE_DRIFT immediately.  No deferral occurs without evidence
/// of a pending fill.
#[tokio::test(flavor = "multi_thread")]
async fn rdf04_halts_immediately_when_no_sent_mapped_rows() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RDF04_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rdf04").await?;

    // No outbox rows seeded — no SENT+mapped rows exist.
    let sent_mapped = mqk_db::outbox_load_sent_with_broker_map_for_run(&pool, run_id).await?;
    assert_eq!(
        sent_mapped.len(),
        0,
        "RDF-04: no SENT+mapped rows should exist before tick"
    );

    let clock = FixedClock::new(Utc::now());
    // Dirty reconcile: local AAPL=1, broker empty.
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 1);
    let broker = BrokerSnapshot::empty_at(2_000_000_000);

    let mut orch = make_orchestrator_with_snapshots(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RDF-04: tick() must halt immediately with no SENT+mapped rows; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RDF-04: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RDF-04: run must be HALTED; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RDF-05: fill applied → subsequent reconcile clean → tick ok
// ---------------------------------------------------------------------------

/// After a fill inbox row is applied, the local snapshot is updated and the
/// reconcile check passes cleanly.  No deferred state persists beyond the
/// fill application.
///
/// This test seeds a SENT+mapped row, applies a fill inbox row to mark the
/// outbox ACKED and broker_map removed, then presents matching local+broker
/// snapshots.  The tick must succeed with a clean reconcile.
#[tokio::test(flavor = "multi_thread")]
async fn rdf05_clean_reconcile_after_fill_applied() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RDF05_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rdf05").await?;

    let internal_id = "rdf05-ord-001";
    let sent_at = Utc::now();
    seed_sent_outbox_with_map(
        &pool,
        run_id,
        internal_id,
        "broker-rdf05",
        "AAPL",
        "buy",
        1,
        sent_at,
    )
    .await?;

    // Simulate fill applied: mark outbox ACKED and remove broker_order_map.
    mqk_db::outbox_mark_acked(&pool, internal_id).await?;
    mqk_db::broker_map_remove(&pool, internal_id).await?;

    // After fill: no SENT+mapped rows remain.
    let sent_mapped = mqk_db::outbox_load_sent_with_broker_map_for_run(&pool, run_id).await?;
    assert_eq!(
        sent_mapped.len(),
        0,
        "RDF-05: no SENT+mapped rows after fill applied"
    );

    let clock = FixedClock::new(Utc::now());

    // Local and broker both show AAPL=2 (fill applied to portfolio) + no open orders.
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 2);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 2);

    let mut orch = make_orchestrator_with_snapshots(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RDF-05: tick() must succeed after fill applied and reconcile clean; got: {:?}",
        result.err()
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "RDF-05: run must remain RUNNING after clean reconcile; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
