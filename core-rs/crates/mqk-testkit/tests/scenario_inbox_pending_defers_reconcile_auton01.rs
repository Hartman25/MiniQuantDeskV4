//! AUTON-SENT-ORDER-BROKER-TRUTH-01 — Regression tests for inbox-pending
//! reconcile deferral in Phase 0c.
//!
//! # Root cause reproduced
//!
//! When the Alpaca WS transport inserts a fill event into `oms_inbox` between
//! orchestrator ticks, the next tick's Phase 0c runs with:
//!   - local snapshot = previous tick's portfolio (no fill applied)
//!   - broker snapshot = Alpaca REST (fill reflected or order missing from seed)
//!
//! Without the fix, this causes a false-positive dirty reconcile that halts
//! the run before Phase 3 can apply the inbox fill.
//!
//! # Fix proven
//!
//! Phase 0c now loads `inbox_load_unapplied_for_run` before the reconcile
//! check.  If unapplied rows exist, the check is deferred to the next tick
//! after Phase 3 drains them.
//!
//! # Test inventory
//!
//! | ID      | Scenario                                          | Expected         |
//! |---------|---------------------------------------------------|------------------|
//! | ASB-01  | Dirty reconcile + pending inbox rows → tick ok    | tick() Ok        |
//! | ASB-02  | Dirty reconcile + empty inbox → tick halts        | tick() Err       |
//! | ASB-03  | Two-tick: first defers (inbox), second halts      | Ok then Err      |
//!
//! All tests require `MQK_DATABASE_URL`. Skipped gracefully when absent.

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
use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs
// ---------------------------------------------------------------------------

const ASB01_RUN: &str = "a5b10001-0000-0000-0000-000000000000";
const ASB02_RUN: &str = "a5b20002-0000-0000-0000-000000000000";
const ASB03_RUN: &str = "a5b30003-0000-0000-0000-000000000000";

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

/// Build a dirty reconcile snapshot pair: local has AAPL position, broker has none.
///
/// This simulates the state where a fill arrived via WS and is reflected in the
/// broker snapshot but not yet applied to the local portfolio.
fn dirty_local() -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert("AAPL".to_string(), 1);
    s
}

fn empty_broker_at(ts_ms: i64) -> BrokerSnapshot {
    BrokerSnapshot::empty_at(ts_ms)
}

/// Orchestrator wired with a dirty reconcile: local says AAPL=1, broker says nothing.
///
/// The gateway ReconcileGate is PassGate (clean) — only Phase 0c inline check
/// uses the local/broker snapshot providers.
fn make_dirty_reconcile_orchestrator(
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
        "asb-dispatcher",
        "test-adapter",
        None,
        FixedClock::new(Utc::now()),
        // Local snapshot: AAPL position = 1 (simulates post-fill local state)
        Box::new(dirty_local),
        // Broker snapshot: empty (simulates stale broker seed or seed missing order)
        // fetched_at_ms > 0 so watermark accepts it as Fresh.
        Box::new(|| empty_broker_at(1_000_000_000)),
    )
}

// ---------------------------------------------------------------------------
// ASB-01: dirty reconcile + pending inbox rows → tick defers and succeeds
// ---------------------------------------------------------------------------

/// When Phase 0c detects unapplied inbox rows, the inline reconcile check is
/// deferred. The tick proceeds to Phase 1 (no pending outbox) and Phase 3
/// (applies the ACK from inbox). tick() returns Ok.
///
/// This proves the root-cause fix: WS fills in inbox no longer cause false-
/// positive ReconcileDrift halts.
#[tokio::test(flavor = "multi_thread")]
async fn asb01_dirty_reconcile_deferred_when_inbox_has_pending_rows() -> Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = ASB01_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "asb01").await?;

    // Seed an unapplied ACK inbox row. This simulates a WS event that arrived
    // between ticks (applied_at_utc IS NULL).
    let ack_json = json!({
        "type": "ack",
        "broker_message_id": "asb01-ack-001",
        "internal_order_id": "asb01-ord-001",
        "broker_order_id": null
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "asb01-ack-001", ack_json).await?;
    assert!(inserted, "ASB-01: inbox ACK row must be inserted");

    // Verify the row is unapplied before the tick.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        unapplied.len(),
        1,
        "ASB-01: exactly 1 unapplied inbox row expected before tick"
    );

    // tick() must succeed: Phase 0c defers the dirty reconcile because inbox is
    // non-empty. Phase 3 attempts to apply the ACK, but because there is no
    // corresponding OMS order in memory the non-fill ACK is silently skipped
    // (Section C policy: non-fill events for unknown orders are skipped).
    let mut orch = make_dirty_reconcile_orchestrator(pool.clone(), run_id);
    let result = orch.tick().await;

    assert!(
        result.is_ok(),
        "ASB-01: tick() must succeed when inbox has pending rows \
         (reconcile deferred by AUTON-SENT-ORDER-BROKER-TRUTH-01 guard); got: {:?}",
        result.err()
    );

    // After tick, the ACK row is marked applied.
    let still_unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        still_unapplied.len(),
        0,
        "ASB-01: inbox must be fully drained after tick"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ASB-02: dirty reconcile + empty inbox → tick halts
// ---------------------------------------------------------------------------

/// When the inbox is empty (no pending rows), Phase 0c runs the inline
/// reconcile check normally.  A dirty local vs broker snapshot causes the tick
/// to halt with ReconcileDrift — no false deferral occurs.
///
/// This proves the guard does NOT permanently suppress genuine dirty reconcile.
#[tokio::test(flavor = "multi_thread")]
async fn asb02_dirty_reconcile_halts_when_inbox_empty() -> Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = ASB02_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "asb02").await?;

    // No inbox rows — Phase 0c will run the reconcile check.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        unapplied.len(),
        0,
        "ASB-02: inbox must be empty before tick"
    );

    let mut orch = make_dirty_reconcile_orchestrator(pool.clone(), run_id);
    let result = orch.tick().await;

    assert!(
        result.is_err(),
        "ASB-02: tick() must fail on dirty reconcile with empty inbox"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("RECONCILE_DRIFT") || err_msg.contains("RECONCILE_SNAPSHOT_STALE"),
        "ASB-02: error must be a reconcile halt; got: {err_msg}"
    );

    // Verify the run was halted in DB.
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "ASB-02: run must be HALTED after reconcile dirty tick"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ASB-03: two-tick sequence — first defers, second halts on genuine dirty
// ---------------------------------------------------------------------------

/// Proves the one-tick deferral: on the first tick inbox has rows → deferred.
/// After Phase 3 applies them the inbox is empty.  On the second tick the
/// inbox is empty → Phase 0c fires the reconcile check → halts on genuine
/// dirty (local AAPL=1, broker empty).
///
/// This closes the proof that the guard cannot suppress genuine drift
/// indefinitely — it defers by exactly one tick, no more.
#[tokio::test(flavor = "multi_thread")]
async fn asb03_one_tick_deferral_then_genuine_halt() -> Result<()> {
    let _guard = test_guard().await;

    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = ASB03_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "asb03").await?;

    // Seed one unapplied ACK row.
    let ack_json = json!({
        "type": "ack",
        "broker_message_id": "asb03-ack-001",
        "internal_order_id": "asb03-ord-001",
        "broker_order_id": null
    });
    mqk_db::inbox_insert_deduped(&pool, run_id, "asb03-ack-001", ack_json).await?;

    let mut orch = make_dirty_reconcile_orchestrator(pool.clone(), run_id);

    // Tick 1: inbox has 1 row → Phase 0c defers → tick succeeds.
    let tick1 = orch.tick().await;
    assert!(
        tick1.is_ok(),
        "ASB-03: tick 1 must succeed (reconcile deferred by inbox-pending guard); got: {:?}",
        tick1.err()
    );

    // After tick 1: inbox is empty (ACK was applied/skipped in Phase 3).
    let after_tick1 = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        after_tick1.len(),
        0,
        "ASB-03: inbox must be empty after tick 1"
    );

    // Tick 2: inbox empty → Phase 0c runs reconcile → dirty (AAPL position
    // mismatch) → halts.
    let tick2 = orch.tick().await;
    assert!(
        tick2.is_err(),
        "ASB-03: tick 2 must halt on genuine dirty reconcile; got: Ok"
    );
    let err_msg = tick2.unwrap_err().to_string();
    assert!(
        err_msg.contains("RECONCILE_DRIFT") || err_msg.contains("RECONCILE_SNAPSHOT_STALE"),
        "ASB-03: tick 2 error must be reconcile halt; got: {err_msg}"
    );

    // Run must be HALTED in DB after tick 2.
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "ASB-03: run must be HALTED after tick 2 reconcile drift"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
