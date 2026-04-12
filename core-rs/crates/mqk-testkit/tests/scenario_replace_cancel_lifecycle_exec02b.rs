//! EXEC-02B — Orchestrator-driven lifecycle event write path.
//!
//! Closes the proof gap left by EXEC-02 surface tests (E02-P01..P05 / D01..D02),
//! which proved the DB path is wired but never drove a real lifecycle event
//! through `ExecutionOrchestrator::tick()` and asserted on DB rows.
//!
//! Each test below drives a real `ExecutionOrchestrator::tick()` call with a
//! cancel/replace inbox event and then asserts on the `oms_order_lifecycle_events`
//! rows produced by Phase 3b.
//!
//! Events for unknown orders are used deliberately — `apply_fill_step` silently
//! skips non-fill events for unknown orders (Section C), so the orchestrator
//! proceeds to Phase 3b and the lifecycle write executes without needing a full
//! order submit cycle.
//!
//! ## Test inventory
//!
//! | ID    | Scenario                                      | Write path proven           |
//! |-------|-----------------------------------------------|-----------------------------|
//! | LC-01 | CancelAck → 1 row, operation="cancel_ack"     | Phase 3b → DB               |
//! | LC-02 | ReplaceAck → 1 row, new_total_qty populated   | Phase 3b → DB, qty field    |
//! | LC-03 | CancelReject → 1 row, operation="cancel_reject" | Phase 3b → DB             |
//! | LC-04 | ReplaceReject → 1 row, operation="replace_reject" | Phase 3b → DB           |
//!
//! Requires `MQK_DATABASE_URL`. Skips with a diagnostic message if absent or
//! unreachable — same skip-gracefully contract as all other DB-backed tests.

use anyhow::Result;
use chrono::Utc;
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
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs
// ---------------------------------------------------------------------------

const LC01_RUN_ID: &str = "ec020b01-0000-0000-0000-000000000000";
const LC02_RUN_ID: &str = "ec020b02-0000-0000-0000-000000000000";
const LC03_RUN_ID: &str = "ec020b03-0000-0000-0000-000000000000";
const LC04_RUN_ID: &str = "ec020b04-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// In-process serialization — single runtime lease row
// ---------------------------------------------------------------------------

static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn test_guard() -> MutexGuard<'static, ()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

// ---------------------------------------------------------------------------
// Stubs (identical contract to TV-EXEC-01B)
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

/// Delete lifecycle events for this run (no FK cascade from runs).
async fn cleanup_lifecycle_events(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from oms_order_lifecycle_events where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        delete from broker_order_map
        where internal_id in (
            select idempotency_key from oms_outbox where run_id = $1
        )
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
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
        declare
            rec record;
        begin
            for rec in
                select c.table_schema, c.table_name
                from information_schema.columns c
                where c.table_schema = 'public'
                group by c.table_schema, c.table_name
                having
                    (
                        bool_or(c.column_name = 'holder_id')
                        or bool_or(c.column_name = 'current_holder')
                        or bool_or(c.column_name = 'holder')
                    )
                    and
                    (
                        bool_or(c.column_name = 'current_epoch')
                        or bool_or(c.column_name = 'epoch')
                    )
                    and
                    (
                        bool_or(c.column_name = 'lease_expires_at')
                        or bool_or(c.column_name = 'lease_expires_at_utc')
                        or bool_or(c.column_name = 'expires_at')
                        or bool_or(c.column_name = 'expires_at_utc')
                    )
            loop
                execute format('delete from %I.%I', rec.table_schema, rec.table_name);
            end loop;
        end
        $$;
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn make_orchestrator(
    pool: PgPool,
    run_id: Uuid,
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
        "lc-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

// ---------------------------------------------------------------------------
// LC-01: CancelAck → 1 lifecycle row, operation="cancel_ack"
// ---------------------------------------------------------------------------

/// A real orchestrator tick processes a CancelAck inbox event for an unknown
/// order (silently skipped by apply_fill_step) and writes exactly one
/// `oms_order_lifecycle_events` row with operation="cancel_ack" in Phase 3b.
///
/// This proves the end-to-end path: inbox row → tick() Phase 3b →
/// insert_order_lifecycle_event → fetch_order_lifecycle_events_for_run.
#[tokio::test]
async fn lc01_cancel_ack_writes_lifecycle_row() -> anyhow::Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = LC01_RUN_ID.parse().unwrap();

    cleanup_lifecycle_events(&pool, run_id).await?;
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;

    seed_running_run(&pool, run_id, "lc01").await?;

    // Seed a CancelAck for an unknown order.
    // apply_fill_step skips non-fill events for unknown orders (Section C) →
    // Phase 3b lifecycle write executes unconditionally for CancelAck.
    let cancel_json = json!({
        "type":              "cancel_ack",
        "broker_message_id": "lc01-msg-001",
        "internal_order_id": "lc01-ord-unknown",
        "broker_order_id":   "brk-lc01-001"
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "lc01-msg-001", cancel_json).await?;
    assert!(inserted, "LC-01: CancelAck inbox row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("LC-01: tick() must succeed for CancelAck of unknown order, got: {e}")
    })?;

    // Assert exactly one lifecycle event row.
    let rows = mqk_db::fetch_order_lifecycle_events_for_run(&pool, run_id).await?;
    assert_eq!(
        rows.len(),
        1,
        "LC-01: expected 1 lifecycle row after CancelAck, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(
        row.event_id, "lc01-msg-001",
        "LC-01: event_id must equal broker_message_id"
    );
    assert_eq!(row.run_id, run_id, "LC-01: run_id");
    assert_eq!(
        row.internal_order_id, "lc01-ord-unknown",
        "LC-01: internal_order_id"
    );
    assert_eq!(
        row.operation, "cancel_ack",
        "LC-01: operation must be cancel_ack"
    );
    assert_eq!(
        row.broker_order_id.as_deref(),
        Some("brk-lc01-001"),
        "LC-01: broker_order_id must be preserved"
    );
    assert!(
        row.new_total_qty.is_none(),
        "LC-01: new_total_qty must be None for cancel_ack"
    );

    cleanup_lifecycle_events(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LC-02: ReplaceAck → 1 lifecycle row, new_total_qty populated
// ---------------------------------------------------------------------------

/// A ReplaceAck inbox event for an unknown order produces one lifecycle row
/// with operation="replace_ack" and new_total_qty carrying the authoritative
/// post-replace total quantity from the broker event.
#[tokio::test]
async fn lc02_replace_ack_writes_lifecycle_row_with_new_qty() -> anyhow::Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = LC02_RUN_ID.parse().unwrap();

    cleanup_lifecycle_events(&pool, run_id).await?;
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;

    seed_running_run(&pool, run_id, "lc02").await?;

    let replace_json = json!({
        "type":              "replace_ack",
        "broker_message_id": "lc02-msg-001",
        "internal_order_id": "lc02-ord-unknown",
        "broker_order_id":   null,
        "new_total_qty":     75_i64
    });
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, "lc02-msg-001", replace_json).await?;
    assert!(inserted, "LC-02: ReplaceAck inbox row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("LC-02: tick() must succeed for ReplaceAck of unknown order, got: {e}")
    })?;

    let rows = mqk_db::fetch_order_lifecycle_events_for_run(&pool, run_id).await?;
    assert_eq!(
        rows.len(),
        1,
        "LC-02: expected 1 lifecycle row after ReplaceAck, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(row.event_id, "lc02-msg-001", "LC-02: event_id");
    assert_eq!(
        row.operation, "replace_ack",
        "LC-02: operation must be replace_ack"
    );
    assert_eq!(
        row.new_total_qty,
        Some(75),
        "LC-02: new_total_qty must carry authoritative post-replace qty"
    );
    assert!(
        row.broker_order_id.is_none(),
        "LC-02: broker_order_id must be None when not supplied"
    );

    cleanup_lifecycle_events(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LC-03: CancelReject → 1 lifecycle row, operation="cancel_reject"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lc03_cancel_reject_writes_lifecycle_row() -> anyhow::Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = LC03_RUN_ID.parse().unwrap();

    cleanup_lifecycle_events(&pool, run_id).await?;
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;

    seed_running_run(&pool, run_id, "lc03").await?;

    let reject_json = json!({
        "type":              "cancel_reject",
        "broker_message_id": "lc03-msg-001",
        "internal_order_id": "lc03-ord-unknown",
        "broker_order_id":   null
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "lc03-msg-001", reject_json).await?;
    assert!(inserted, "LC-03: CancelReject inbox row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("LC-03: tick() must succeed for CancelReject of unknown order, got: {e}")
    })?;

    let rows = mqk_db::fetch_order_lifecycle_events_for_run(&pool, run_id).await?;
    assert_eq!(
        rows.len(),
        1,
        "LC-03: expected 1 lifecycle row after CancelReject, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(row.event_id, "lc03-msg-001", "LC-03: event_id");
    assert_eq!(
        row.operation, "cancel_reject",
        "LC-03: operation must be cancel_reject"
    );
    assert!(
        row.new_total_qty.is_none(),
        "LC-03: new_total_qty must be None for cancel_reject"
    );

    cleanup_lifecycle_events(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LC-04: ReplaceReject → 1 lifecycle row, operation="replace_reject"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lc04_replace_reject_writes_lifecycle_row() -> anyhow::Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = LC04_RUN_ID.parse().unwrap();

    cleanup_lifecycle_events(&pool, run_id).await?;
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;

    seed_running_run(&pool, run_id, "lc04").await?;

    let replace_reject_json = json!({
        "type":              "replace_reject",
        "broker_message_id": "lc04-msg-001",
        "internal_order_id": "lc04-ord-unknown",
        "broker_order_id":   null
    });
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, "lc04-msg-001", replace_reject_json).await?;
    assert!(inserted, "LC-04: ReplaceReject inbox row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("LC-04: tick() must succeed for ReplaceReject of unknown order, got: {e}")
    })?;

    let rows = mqk_db::fetch_order_lifecycle_events_for_run(&pool, run_id).await?;
    assert_eq!(
        rows.len(),
        1,
        "LC-04: expected 1 lifecycle row after ReplaceReject, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(row.event_id, "lc04-msg-001", "LC-04: event_id");
    assert_eq!(
        row.operation, "replace_reject",
        "LC-04: operation must be replace_reject"
    );
    assert!(
        row.new_total_qty.is_none(),
        "LC-04: new_total_qty must be None for replace_reject"
    );

    cleanup_lifecycle_events(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    cleanup_run(&pool, run_id).await?;
    Ok(())
}
