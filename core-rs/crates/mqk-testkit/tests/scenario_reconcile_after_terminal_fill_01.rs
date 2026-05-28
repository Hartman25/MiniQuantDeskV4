//! RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01 — Proof tests for the terminal-fill
//! broker snapshot settle window in Phase 0c.
//!
//! # Root cause
//!
//! After a terminal fill is applied in Phase 3:
//!   1. `broker_map_remove` runs — `outbox_load_sent_with_broker_map_for_run`
//!      returns zero rows on the very next tick.
//!   2. The existing `RECONCILE-DRIFT-AFTER-FAST-PAPER-FILL-01` guard (which
//!      checks SENT+mapped rows) fires `in_grace = false` immediately.
//!   3. The broker REST snapshot still shows the pre-fill position (stale).
//!   4. Phase 0c compares local=post-fill vs broker=pre-fill → DIRTY →
//!      RECONCILE_DRIFT halt — a false positive.
//!
//! Observed in live smoke 2026-05-27: fill applied at 19:46:01.150, halt
//! triggered 710ms later (19:46:01.861) before any broker snapshot refresh.
//!
//! # Fix proven
//!
//! Phase 3 now records each terminal fill in an in-memory ring buffer
//! (`recently_applied_fills`).  Phase 0c checks this buffer when the
//! SENT+mapped guard does not cover the drift: if a fill was applied within
//! `TERMINAL_FILL_SETTLE_GRACE_SECS` (180s) and the drift is directionally
//! consistent with that fill, the check is deferred until the broker snapshot
//! refreshes.
//!
//! # Test inventory
//!
//! | ID      | Scenario                                                    | Expected       |
//! |---------|-------------------------------------------------------------|----------------|
//! | RTF-01  | Stale broker snapshot immediately after terminal fill       | tick() Ok      |
//! | RTF-02  | Same but grace expired (applied_at beyond 180s window)      | tick() Err     |
//! | RTF-03  | Fresh broker snapshot shows unexpected qty → halt           | tick() Err     |
//! | RTF-04  | No recent fills, unexplained drift → halt (fail-closed)     | tick() Err     |
//! | RTF-05  | Fresh matching broker snapshot → clean reconcile → Ok       | tick() Ok      |
//! | RTF-06  | Sell fill: local behind broker correctly defers             | tick() Ok      |
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
use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs
// ---------------------------------------------------------------------------

const RTF01_RUN: &str = "f1f10001-0001-0000-0000-000000000001";
const RTF02_RUN: &str = "f2f20002-0002-0000-0000-000000000002";
const RTF03_RUN: &str = "f3f30003-0003-0000-0000-000000000003";
const RTF04_RUN: &str = "f4f40004-0004-0000-0000-000000000004";
const RTF05_RUN: &str = "f5f50005-0005-0000-0000-000000000005";
const RTF06_RUN: &str = "f6f60006-0006-0000-0000-000000000006";

// ---------------------------------------------------------------------------
// Serialization guard
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
        Ok(v) if !v.trim().is_empty() => {
            let url = v.trim().to_string();
            mqk_testkit::assert_test_db_url(&url);
            Some(url)
        }
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

fn make_orchestrator(
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
        "rtf-dispatcher",
        "test-adapter",
        None,
        clock,
        Box::new(move || local.clone()),
        Box::new(move || broker.clone()),
    )
}

// ---------------------------------------------------------------------------
// RTF-01: stale broker snapshot immediately after terminal fill → defers
// ---------------------------------------------------------------------------

/// Core regression test.  Proves that Phase 0c defers RECONCILE_DRIFT when:
///   - local portfolio shows AAPL=7 (fill applied: baseline 6 + fill +1)
///   - broker snapshot still shows AAPL=6 (stale, not yet refreshed)
///   - recently_applied_fills has a buy AAPL +1 applied < 180s ago
///
/// This is the exact failure from the 2026-05-27 live smoke run.
#[tokio::test(flavor = "multi_thread")]
async fn rtf01_deferred_when_terminal_fill_within_grace_and_consistent() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF01_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf01").await?;

    // Local: AAPL=7 (baseline 6 + fill +1 applied)
    // Broker: AAPL=6 (stale, pre-fill snapshot)
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 6);

    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    // Inject the terminal fill as if Phase 3 just applied it (< 180s ago).
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RTF-01: tick() must defer RECONCILE_DRIFT when terminal fill within grace; got: {:?}",
        result.err()
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "RTF-01: run must remain RUNNING; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RTF-02: grace expired → RECONCILE_DRIFT fires
// ---------------------------------------------------------------------------

/// After the 180-second settle window expires, RECONCILE_DRIFT fires even when
/// a recent terminal fill is present.  Proves the deferral is bounded.
#[tokio::test(flavor = "multi_thread")]
async fn rtf02_halts_when_settle_grace_expired() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF02_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf02").await?;

    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 6);

    // Clock is now+181s so the fill appears 181s old (> 180s grace).
    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    // Fill was applied 181s ago (outside grace).
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RTF-02: tick() must halt when settle grace expired; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RTF-02: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RTF-02: run must be HALTED after grace expired; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RTF-03: fresh broker snapshot shows unexpected qty → halt
// ---------------------------------------------------------------------------

/// If the broker snapshot refreshes but shows an unexpected position (AAPL=8
/// instead of the expected 7 after a buy +1), RECONCILE_DRIFT fires.
/// The settle window does not suppress genuine drift.
#[tokio::test(flavor = "multi_thread")]
async fn rtf03_halts_when_fresh_broker_snapshot_mismatches() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF03_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf03").await?;

    // Local: AAPL=7 (expected after buy +1 from baseline 6)
    // Broker: AAPL=8 (unexpected — more than the fill explains)
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 8);

    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    // Fill applied recently — but broker shows 8 not 7, which is not explained.
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RTF-03: tick() must halt when broker shows unexpected position; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RTF-03: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RTF-03: run must be HALTED; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RTF-04: no recent fills, unexplained drift → halt (fail-closed regression)
// ---------------------------------------------------------------------------

/// Without any recent terminal fills, unexplained drift causes RECONCILE_DRIFT
/// immediately.  Confirms the fix does not relax fail-closed behavior when no
/// fill evidence is present.
#[tokio::test(flavor = "multi_thread")]
async fn rtf04_halts_immediately_with_no_recent_fills_unexplained_drift() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF04_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf04").await?;

    // Dirty reconcile with no fills to explain it.
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let broker = BrokerSnapshot::empty_at(2_000_000_000);

    // No inject_recent_terminal_fill_for_test call.
    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RTF-04: tick() must halt with no recent fills and unexplained drift; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RTF-04: error must be RECONCILE_DRIFT; got: {msg}"
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "RTF-04: run must be HALTED; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RTF-05: fresh matching broker snapshot → clean reconcile
// ---------------------------------------------------------------------------

/// After the broker snapshot refreshes to match the post-fill local position
/// (AAPL=7), reconcile is clean and tick() succeeds without needing the settle
/// window.  Proves the fix does not interfere with the normal clean path.
#[tokio::test(flavor = "multi_thread")]
async fn rtf05_clean_reconcile_when_broker_snapshot_refreshes() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF05_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf05").await?;

    // Local and broker both show AAPL=7 (broker snapshot has caught up).
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 7);

    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    // Fill is in buffer too (doesn't matter — reconcile is clean).
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RTF-05: tick() must succeed when broker snapshot matches local post-fill; got: {:?}",
        result.err()
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "RTF-05: run must remain RUNNING; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RTF-06: sell fill — local behind broker correctly defers
// ---------------------------------------------------------------------------

/// For a sell fill: local position decreases (e.g. local=5, broker=6 stale).
/// The settle window must recognize this as a deferred case when a recent
/// sell fill explains the delta.
///
/// signed_delta for sell = -1. local - broker = 5 - 6 = -1. Expected = -1.
/// Both negative, magnitude matches → deferred.
#[tokio::test(flavor = "multi_thread")]
async fn rtf06_deferred_for_sell_fill_within_grace() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RTF06_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    clear_runtime_lease_rows(&pool).await?;
    clear_arm_state(&pool).await?;
    seed_running_run(&pool, run_id, "rtf06").await?;

    // Local: AAPL=5 (sold 1 from baseline 6)
    // Broker: AAPL=6 (stale, pre-sell snapshot)
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 5);
    let mut broker = BrokerSnapshot::empty_at(2_000_000_000);
    broker.positions.insert("AAPL".to_string(), 6);

    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, broker);

    // Inject sell fill: signed_delta = -1 (sell reduces position).
    orch.inject_recent_terminal_fill_for_test("AAPL", -1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RTF-06: tick() must defer for sell fill within grace; got: {:?}",
        result.err()
    );

    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "RTF-06: run must remain RUNNING; got: {:?}",
        run.status
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
