//! Scenario: Invariant Violation Halts and Persists — I9-1
//!
//! # Invariants under test
//!
//! When `check_capital_invariants` fires inside the runtime `tick()` apply
//! loop, the orchestrator must — before returning — atomically (best-effort)
//! write two DB records and then return `Err`:
//!
//! 1. `runs.status = 'HALTED'` via `halt_run`.
//! 2. `sys_arm_state.state = 'DISARMED'`, `reason = 'IntegrityViolation'`
//!    via `persist_arm_state`.
//!
//! A subsequent `tick()` call on the same (or a new) orchestrator for the
//! same `run_id` must be refused immediately by the halt guard at the top of
//! `tick()` — the guard reads run status from DB at entry and returns
//! `Err("HALT_GUARD: …")` without reaching Phase 1 / submit.
//!
//! ## Test S1 — i91_invariant_violation_halts_run_and_disarms
//!
//! Scenario:
//!   - Run seeded in RUNNING state.
//!   - Unapplied inbox Ack row inserted (triggers apply loop; no portfolio
//!     mutation — Ack carries no fill — so corruption is detected as-is).
//!   - `ExecutionOrchestrator` constructed with a portfolio whose `cash_micros`
//!     is offset by −1 from the empty ledger's recomputed value.
//!   - `tick()` called → invariant violation → halt + disarm persisted.
//!   - DB assertions: `runs.status = 'HALTED'`, `halted_at_utc IS NOT NULL`,
//!     `sys_arm_state = ('DISARMED', 'IntegrityViolation')`.
//!
//! ## Test S2 — i91_halted_run_refuses_subsequent_tick
//!
//! Scenario:
//!   - Same setup as S1; first `tick()` halts.
//!   - Second `tick()` on the same orchestrator must be refused by the halt
//!     guard (error contains "HALT_GUARD"), NOT by re-running the invariant.
//!
//! Requires `MQK_DATABASE_URL`. Skips with a diagnostic message if absent or
//! unreachable.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    IntegrityGate, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs — deterministic, zero production collision risk.
// ---------------------------------------------------------------------------

const S1_RUN_ID: &str = "19100001-0000-0000-0000-000000000000";
const S2_RUN_ID: &str = "19100002-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

/// Broker adapter that accepts submits and returns no events.
struct NullBroker;

impl BrokerAdapter for NullBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
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

/// All-pass gate stub — isolates the invariant-violation path from gates.
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

/// Seed a run in RUNNING state (CREATED → ARMED → RUNNING).
async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "i91-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "i91-test".to_string(),
            config_hash: "i91-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "i91-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

/// Delete all rows for run_id (cascades to oms_inbox, oms_outbox).
async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Build a `ExecutionOrchestrator` with a pre-corrupted portfolio.
///
/// `portfolio.cash_micros` is offset by −1 from `initial_cash_micros` while
/// the ledger remains empty.  `recompute_from_ledger` will therefore return
/// `initial_cash_micros` and the invariant check will detect a mismatch.
fn make_corrupted_orchestrator(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<NullBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(NullBroker, PassGate, PassGate, PassGate);

    let mut portfolio = PortfolioState::new(1_000_000_000_i64);
    // Corrupt without a ledger entry — recompute will return 1_000_000_000,
    // but state holds 999_999_999 → cash_micros mismatch.
    portfolio.cash_micros -= 1;

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(), // empty oms_orders — Ack skips OMS transition
        portfolio,
        run_id,
        "i91-dispatcher",
        FixedClock::new(Utc::now()),
    )
}

// ---------------------------------------------------------------------------
// S1: invariant violation → HALTED run + DISARMED arm state persisted
// ---------------------------------------------------------------------------

/// After a capital invariant violation in tick(), the run must be HALTED in
/// the DB and the arm state must be DISARMED with reason IntegrityViolation.
#[tokio::test]
async fn i91_invariant_violation_halts_run_and_disarms() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = S1_RUN_ID.parse().unwrap();

    // ── Pre-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    // Reset arm-state singleton so this test's assertion is unambiguous.
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;

    // ── Seed RUNNING run ──────────────────────────────────────────────────
    seed_running_run(&pool, run_id).await?;

    // ── Insert unapplied inbox Ack so the apply loop fires ────────────────
    //
    // An Ack event carries no fill data, so broker_event_to_fill returns None
    // and apply_entry is not called.  The portfolio stays corrupted and the
    // invariant check (Step 8) fires on the first iteration.
    let msg_json = json!({
        "type":              "ack",
        "broker_message_id": "i91-s1-msg-001",
        "internal_order_id": "i91-s1-ord-001"
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "i91-s1-msg-001", msg_json).await?;
    assert!(inserted, "S1: inbox Ack row must be inserted");

    // ── tick() must fail with INVARIANT_VIOLATED ──────────────────────────
    let mut orch = make_corrupted_orchestrator(pool.clone(), run_id);
    let err = orch
        .tick()
        .await
        .expect_err("S1: tick() must return Err on invariant violation");

    let err_str = err.to_string();
    assert!(
        err_str.contains("INVARIANT_VIOLATED"),
        "S1: error must contain 'INVARIANT_VIOLATED', got: {err_str}"
    );

    // ── DB: run must be HALTED ────────────────────────────────────────────
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "S1: run must be HALTED after invariant violation, status={}",
        run.status.as_str()
    );
    assert!(
        run.halted_at_utc.is_some(),
        "S1: halted_at_utc must be set after halt"
    );

    // ── DB: arm state must be DISARMED / IntegrityViolation ───────────────
    let arm = mqk_db::load_arm_state(&pool)
        .await?
        .expect("S1: sys_arm_state must have a row after invariant halt");
    assert_eq!(arm.0, "DISARMED", "S1: arm state must be DISARMED");
    assert_eq!(
        arm.1.as_deref(),
        Some("IntegrityViolation"),
        "S1: disarm reason must be 'IntegrityViolation'"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// S2: halted run refuses all subsequent ticks via the halt guard
// ---------------------------------------------------------------------------

/// After the halt is persisted by the first tick(), a second tick() on the
/// same orchestrator must be refused by the Phase 0 halt guard — not by
/// re-running the invariant check.  The error must contain "HALT_GUARD".
#[tokio::test]
async fn i91_halted_run_refuses_subsequent_tick() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = S2_RUN_ID.parse().unwrap();

    // ── Pre-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;

    seed_running_run(&pool, run_id).await?;

    let msg_json = json!({
        "type":              "ack",
        "broker_message_id": "i91-s2-msg-001",
        "internal_order_id": "i91-s2-ord-001"
    });
    mqk_db::inbox_insert_deduped(&pool, run_id, "i91-s2-msg-001", msg_json).await?;

    let mut orch = make_corrupted_orchestrator(pool.clone(), run_id);

    // ── First tick: invariant violation → halt + disarm ───────────────────
    let first_err = orch.tick().await.unwrap_err();
    assert!(
        first_err.to_string().contains("INVARIANT_VIOLATED"),
        "S2: first tick must fail with INVARIANT_VIOLATED"
    );

    // Confirm halt is in DB before the second tick.
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "S2: run must be HALTED before second tick"
    );

    // ── Second tick: must be refused by HALT_GUARD ────────────────────────
    //
    // The inbox Ack row was never marked applied (invariant fired before
    // Step 9).  Without the halt guard the second tick would re-enter the
    // apply loop and re-fire the invariant check.  The halt guard must
    // short-circuit at Phase 0 — before any outbox claim or inbox apply.
    let second_err = orch.tick().await.unwrap_err();
    let second_str = second_err.to_string();
    assert!(
        second_str.contains("HALT_GUARD"),
        "S2: second tick must be refused by HALT_GUARD, got: {second_str}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
