//! RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01 — Proof tests for
//! the forced broker snapshot refresh at the terminal-fill settle boundary.
//!
//! # Root cause (revisited)
//!
//! The 180-second settle grace prevents RECONCILE_DRIFT during the window
//! when the broker REST snapshot lags behind a locally-applied terminal fill.
//! But the original fix had a gap at the grace boundary:
//!
//!   - At T+179s: has_recent_terminal_fill=true → eager refresh triggered at
//!     end of the tick (AFTER orchestrator.tick() returns Ok).
//!   - At T+180s: has_recent_terminal_fill=false (elapsed=180 ∉ [0,180)) →
//!     Phase 0c calls drift_is_consistent_with_recent_terminal_fills → false
//!     → RECONCILE_DRIFT halt immediately, WITHOUT a final broker refresh.
//!
//! Live smoke 2026-05-29: fill at 17:37:28.076, halt at 17:40:28.845 (T+180s).
//! Broker REST still showed stale position (1 AAPL; expected 2 after buy fill).
//!
//! # Fix proven
//!
//! When the settle grace has just expired and a recently-expired terminal fill
//! still direction-explains the drift, Phase 0c calls an injected
//! `terminal_fill_expiry_refresher` to force one final broker REST fetch
//! before firing RECONCILE_DRIFT:
//!
//!   - Clean fresh snapshot → RECONCILE_DRIFT suppressed.
//!   - Still-dirty fresh snapshot → RECONCILE_DRIFT fires with evidence.
//!   - Refresh unavailable/failed → fail-closed RECONCILE_DRIFT with diagnostic.
//!   - No expired fill explaining drift → genuine drift, no refresh attempted.
//!
//! # Test inventory
//!
//! | ID      | Scenario                                                              | Expected   |
//! |---------|-----------------------------------------------------------------------|------------|
//! | RFS-01  | Grace expired + fill explains drift + fresh clean snapshot            | tick() Ok  |
//! | RFS-02  | Grace expired + fill explains drift + fresh DIRTY snapshot            | tick() Err |
//! | RFS-03  | Grace expired + fill explains drift + refresher returns None          | tick() Err |
//! | RFS-04  | Grace expired + fill explains drift + NO refresher configured         | tick() Err |
//! | RFS-05  | Grace expired + drift NOT explained by expired fill → genuine drift   | tick() Err |
//! | RFS-06  | Grace ACTIVE (fill < 180s) → still deferred (regression guard)        | tick() Ok  |
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

const RFS01_RUN: &str = "a1a10001-0001-0000-0000-000000000011";
const RFS02_RUN: &str = "a2a20002-0002-0000-0000-000000000012";
const RFS03_RUN: &str = "a3a30003-0003-0000-0000-000000000013";
const RFS04_RUN: &str = "a4a40004-0004-0000-0000-000000000014";
const RFS05_RUN: &str = "a5a50005-0005-0000-0000-000000000015";
const RFS06_RUN: &str = "a6a60006-0006-0000-0000-000000000016";

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
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
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
        "rfs-dispatcher",
        "test-adapter",
        None,
        clock,
        Box::new(move || local.clone()),
        Box::new(move || broker.clone()),
    )
}

/// Build a BrokerSnapshot with a specific AAPL position and a recent timestamp.
fn broker_snapshot_with_aapl(qty: i64) -> BrokerSnapshot {
    let mut snap = BrokerSnapshot::empty_at(Utc::now().timestamp_millis() + 1);
    snap.positions.insert("AAPL".to_string(), qty);
    snap
}

// ---------------------------------------------------------------------------
// RFS-01: grace expired + fill explains drift + fresh clean snapshot → Ok
// ---------------------------------------------------------------------------

/// Core regression test for the T+180s boundary.
///
/// After the settle grace expires (clock at T+181s, fill applied T+0), a
/// stale broker snapshot that still shows the pre-fill position would have
/// caused RECONCILE_DRIFT.  With the forced-refresh fix:
///   - Phase 0c detects the expired fill still direction-explains the drift.
///   - The injected refresher returns a fresh clean snapshot (AAPL=7).
///   - Re-evaluation is clean → tick() returns Ok.
#[tokio::test(flavor = "multi_thread")]
async fn rfs01_fresh_clean_snapshot_prevents_halt_after_grace_expiry() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS01_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs01").await?;

    // Local: AAPL=7 (after buy +1 from baseline 6).
    // Broker (stale): AAPL=6 (pre-fill position, not yet refreshed).
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut stale_broker = BrokerSnapshot::empty_at(2_000_000_000);
    stale_broker.positions.insert("AAPL".to_string(), 6);

    // Clock at T+181s so the fill appears 181s old (outside grace window).
    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, stale_broker);

    // Fill was applied 181s ago (just expired from 180s grace).
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    // Refresher returns a clean snapshot: AAPL=7 (broker caught up).
    let fresh_snap = broker_snapshot_with_aapl(7);
    orch.set_terminal_fill_expiry_refresher(Box::new(move || Some(fresh_snap.clone())));

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RFS-01: tick() must succeed when forced-refresh returns clean snapshot \
         after grace expiry; got: {:?}",
        result.unwrap_err()
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RFS-02: grace expired + fill explains drift + fresh DIRTY snapshot → Err
// ---------------------------------------------------------------------------

/// If the forced broker snapshot refresh returns a snapshot that STILL
/// mismatches the local position, RECONCILE_DRIFT fires.
///
/// Proves fail-closed: genuine drift confirmed by fresh data halts even with
/// the expiry-refresh mechanism active.
#[tokio::test(flavor = "multi_thread")]
async fn rfs02_fresh_dirty_snapshot_still_halts_after_grace_expiry() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS02_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs02").await?;

    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut stale_broker = BrokerSnapshot::empty_at(2_000_000_000);
    stale_broker.positions.insert("AAPL".to_string(), 6);

    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, stale_broker);
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    // Refresher returns a STILL-MISMATCHING snapshot: AAPL=5 (unexpected regression).
    let still_dirty = broker_snapshot_with_aapl(5);
    orch.set_terminal_fill_expiry_refresher(Box::new(move || Some(still_dirty.clone())));

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RFS-02: tick() must halt when forced-refresh still shows mismatch; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RFS-02: error must be RECONCILE_DRIFT; got: {msg}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RFS-03: grace expired + fill explains drift + refresher returns None → Err
// ---------------------------------------------------------------------------

/// If the refresher is configured but returns None (fetch failed), Phase 0c
/// fails closed and fires RECONCILE_DRIFT with a diagnostic.
///
/// Proves: a failed broker REST call at grace expiry does not silently suppress
/// drift detection — it fails as loudly as genuine drift.
#[tokio::test(flavor = "multi_thread")]
async fn rfs03_refresher_failure_halts_fail_closed() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS03_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs03").await?;

    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut stale_broker = BrokerSnapshot::empty_at(2_000_000_000);
    stale_broker.positions.insert("AAPL".to_string(), 6);

    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, stale_broker);
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    // Refresher simulates a broker REST failure (returns None).
    orch.set_terminal_fill_expiry_refresher(Box::new(|| None));

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RFS-03: tick() must halt when refresher returns None; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RFS-03: error must be RECONCILE_DRIFT when refresh unavailable; got: {msg}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RFS-04: grace expired + fill explains drift + NO refresher configured → Err
// ---------------------------------------------------------------------------

/// If no refresher is configured (None), Phase 0c fails closed immediately
/// when the grace window expires and drift is explained by an expired fill.
///
/// This matches the pre-patch behavior (RTF-02) but exercises the new code
/// path: `has_expired_terminal_fill_explaining_drift` = true,
/// `terminal_fill_expiry_refresher` = None → halt.
#[tokio::test(flavor = "multi_thread")]
async fn rfs04_no_refresher_configured_halts_fail_closed() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS04_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs04").await?;

    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut stale_broker = BrokerSnapshot::empty_at(2_000_000_000);
    stale_broker.positions.insert("AAPL".to_string(), 6);

    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    // No set_terminal_fill_expiry_refresher call — refresher stays None.
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, stale_broker);
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RFS-04: tick() must halt when no refresher is configured; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RFS-04: error must be RECONCILE_DRIFT with no refresher; got: {msg}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RFS-05: grace expired + drift NOT explained by expired fill → genuine drift
// ---------------------------------------------------------------------------

/// When the drift is not directionally explained by any recently-expired fill,
/// `has_expired_terminal_fill_explaining_drift` returns false, and Phase 0c
/// halts immediately without calling the refresher.
///
/// Scenario: local=7, broker=8 (broker AHEAD of local on AAPL).
/// Fill was +1 (buy), so expected local-ahead drift is +1.
/// Actual drift is -1 (broker-ahead) → direction mismatch → genuine drift.
#[tokio::test(flavor = "multi_thread")]
async fn rfs05_genuine_drift_halts_without_refresh() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS05_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs05").await?;

    // Local=7, broker=8: broker ahead of local (unexpected external activity).
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut dirty_broker = BrokerSnapshot::empty_at(2_000_000_000);
    dirty_broker.positions.insert("AAPL".to_string(), 8);

    let clock = FixedClock::new(Utc::now() + Duration::seconds(181));
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, dirty_broker);
    // Fill was +1 buy (expects local to be AHEAD); broker is AHEAD → direction mismatch.
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    // Refresher would succeed, but it must NOT be called for genuine drift.
    let refresher_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let refresher_called_clone = std::sync::Arc::clone(&refresher_called);
    orch.set_terminal_fill_expiry_refresher(Box::new(move || {
        refresher_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Some(BrokerSnapshot::empty_at(Utc::now().timestamp_millis() + 1))
    }));

    let result = orch.tick().await;
    assert!(
        result.is_err(),
        "RFS-05: tick() must halt on genuine drift; got: Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("RECONCILE_DRIFT"),
        "RFS-05: error must be RECONCILE_DRIFT; got: {msg}"
    );
    assert!(
        !refresher_called.load(std::sync::atomic::Ordering::SeqCst),
        "RFS-05: refresher must NOT be called for genuine (direction-mismatched) drift"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RFS-06: grace ACTIVE (fill < 180s) → deferred (regression guard)
// ---------------------------------------------------------------------------

/// Proves the grace-active deferral still works after the expiry-refresh
/// changes.  A fill applied within the grace window with a stale broker
/// snapshot defers RECONCILE_DRIFT as before.
#[tokio::test(flavor = "multi_thread")]
async fn rfs06_grace_active_still_defers_reconcile_drift() -> Result<()> {
    let _guard = test_guard().await;
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = RFS06_RUN.parse().unwrap();
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id, "rfs06").await?;

    // Local=7 (post-fill), broker=6 (stale pre-fill).
    let mut local = LocalSnapshot::empty();
    local.positions.insert("AAPL".to_string(), 7);
    let mut stale_broker = BrokerSnapshot::empty_at(2_000_000_000);
    stale_broker.positions.insert("AAPL".to_string(), 6);

    // Clock at current time — fill applied now is within the 180s grace window.
    let clock = FixedClock::new(Utc::now());
    let mut orch = make_orchestrator(pool.clone(), run_id, clock, local, stale_broker);
    orch.inject_recent_terminal_fill_for_test("AAPL", 1, Utc::now());

    let result = orch.tick().await;
    assert!(
        result.is_ok(),
        "RFS-06: tick() must defer when fill is within grace window; got: {:?}",
        result.unwrap_err()
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
