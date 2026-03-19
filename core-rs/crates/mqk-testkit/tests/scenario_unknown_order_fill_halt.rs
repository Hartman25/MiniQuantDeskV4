//! Scenario: Unknown-Order Fill → Halt + Disarm — Section C
//!
//! # Mission
//!
//! Prove that `tick()` → inbox `Fill` for an `internal_order_id` not present in
//! the OMS order map → mandatory halt + disarm, with the halt visible to a brand-
//! new orchestrator instance that reads DB state on the subsequent tick.
//!
//! # Invariants under test
//!
//! **C-1** — Unknown-order fill halts and disarms:
//!   When the inbox contains a `Fill` event whose `internal_order_id` is absent
//!   from the orchestrator's OMS order map, `tick()` must:
//!   1. Call `persist_halt_and_disarm` (mandatory, not best-effort).
//!   2. Write `runs.status = 'HALTED'`, `halted_at_utc IS NOT NULL`.
//!   3. Write `sys_arm_state = 'DISARMED'`, reason `'IntegrityViolation'`.
//!   4. Return `Err(…)` whose string representation contains `"UNKNOWN_ORDER_FILL"`.
//!
//! **C-2** — Halt is sticky across restart:
//!   A fresh `ExecutionOrchestrator` constructed for the same `run_id` — with
//!   all in-process gates passing — is refused at Phase 0 (HALT_GUARD) before
//!   any outbox claim, gateway call, or inbox apply.  This proves the guard reads
//!   from DB on every tick, not from in-memory state.
//!
//! # Test matrix
//!
//! | Test | Invariants | DB? |
//! |------|------------|-----|
//! | `c1_c2_unknown_fill_halts_disarms_and_refuses_restart` | C-1, C-2 | Yes (skip) |
//!
//! Tests skip gracefully when `MQK_DATABASE_URL` is absent or DB is unreachable.
//! If `MQK_DATABASE_URL` is set but the DB cannot be reached, the test skips
//! (same policy as other scenario tests).
//!
//! # Design notes
//!
//! The OMS order map (`BTreeMap::new()`) is intentionally empty so that
//! every Fill event targets an unknown order.  All three gates use `BoolGate(true)`
//! so the only path to halt is the unknown-order-fill detector in the inbox apply
//! loop (Phase 3b, Section C).  This isolates the variable under test.

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

/// Fixed run UUID for the C-1/C-2 unknown-order-fill halt test.
const C1_RUN_ID: &str = "c1000001-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// Stubs
// (Each integration test binary is independent — these cannot be imported
//  from scenario_kill_switch_guarantees.rs.)
// ---------------------------------------------------------------------------

/// Broker that unconditionally accepts everything.
///
/// Gate enforcement — not broker behaviour — is what Section C tests.
struct OkBroker;

impl BrokerAdapter for OkBroker {
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

/// Boolean gate that implements all three gate traits with a single value.
///
/// All gates use `BoolGate(true)` in this file so the only halt path is the
/// unknown-order fill detector — not an integrity gate, risk gate, or
/// reconcile gate decision.
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

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "c1-unknown-fill".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "c1-test".to_string(),
            config_hash: "c1-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "c1-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    // oms_inbox cascades from runs (ON DELETE CASCADE).
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Orchestrator factory
// ---------------------------------------------------------------------------

/// Construct an orchestrator with all gates passing and an EMPTY OMS order map.
///
/// The empty `BTreeMap` means every `Fill` event targets an unknown
/// `internal_order_id`.  The only path to halt is therefore the unknown-order
/// fill detector in the inbox apply loop (Phase 3b).
///
/// This is also used for the Phase 0 HALT_GUARD test: all gates pass in-process
/// so the DB-backed HALT_GUARD is the sole mechanism that can refuse `tick()`.
fn make_clean_orch(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<OkBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), BoolGate(true));

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(), // empty oms_orders — every fill order_id is unknown
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "c1-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

// ---------------------------------------------------------------------------
// C-1 + C-2 — unknown-order fill halts, disarms, and refuses fresh orchestrator
// ---------------------------------------------------------------------------

/// C-1: `tick()` → inbox `Fill` for unknown `internal_order_id` →
///      `UNKNOWN_ORDER_FILL` halt with mandatory DB writes.
///
/// C-2: A fresh `ExecutionOrchestrator` (all gates pass, no in-memory halt
///      state) for the same `run_id` is refused at Phase 0 (HALT_GUARD).
///
/// Sequence:
///
/// 1. Seed a RUNNING run.  Clear `sys_arm_state` so assertions are unambiguous.
/// 2. Insert one unapplied inbox `Fill` row.  The `internal_order_id`
///    (`"cuf-ord-unknown"`) is absent from the empty OMS order map.
/// 3. `tick()` → Phase 3b apply loop → `apply_fill_step` returns `Err` for
///    the unknown order → `persist_halt_and_disarm("IntegrityViolation")` is
///    called mandatorily → `tick()` returns `Err`.
/// 4. Assert error string contains `"UNKNOWN_ORDER_FILL"`.
/// 5. Assert `runs.status = HALTED`, `halted_at_utc IS NOT NULL`.
/// 6. Assert `sys_arm_state = ('DISARMED', 'IntegrityViolation')`.
/// 7. Construct a BRAND-NEW orchestrator (all gates pass — `BoolGate(true)` —
///    no shared in-memory state with the first orchestrator).
/// 8. `tick()` → Phase 0 HALT_GUARD reads `runs.status` from DB → sees
///    `HALTED` → returns `Err` before any outbox claim, gateway call, or inbox
///    apply.
///
/// Step 8 is only possible because step 3's `halt_run` write succeeded —
/// proving the write is mandatory and not best-effort.
///
/// Requires `MQK_DATABASE_URL`.  Skips gracefully if absent or unreachable.
#[tokio::test]
async fn c1_c2_unknown_fill_halts_disarms_and_refuses_restart() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = C1_RUN_ID.parse().expect("C1_RUN_ID must be a valid UUID");

    // ── Pre-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    // Clear the arm-state singleton so our assertions are unambiguous.
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;

    // ── 1. Seed a RUNNING run ─────────────────────────────────────────────
    seed_running_run(&pool, run_id).await?;

    // ── 2. Insert an unapplied Fill for an unknown internal_order_id ──────
    //
    // The BTreeMap in make_clean_orch is empty, so "cuf-ord-unknown" has no
    // matching entry.  apply_fill_step returns Err → Phase 3b halt path.
    //
    // JSON format mirrors BrokerEvent::Fill (serde tag = "snake_case"):
    //   type, broker_message_id, internal_order_id, broker_order_id (null),
    //   symbol, side, delta_qty, price_micros, fee_micros.
    let msg_json = json!({
        "type":              "fill",
        "broker_message_id": "cuf-msg-001",
        "internal_order_id": "cuf-ord-unknown",
        "broker_order_id":   null,
        "symbol":            "SPY",
        "side":              "Buy",
        "delta_qty":         100_i64,
        "price_micros":      100_000_000_i64,
        "fee_micros":        0_i64
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "cuf-msg-001", msg_json).await?;
    assert!(
        inserted,
        "C1: Fill inbox row must be inserted (dedup returned false)"
    );

    // ── 3. tick() must fail — unknown-order fill triggers mandatory halt ───
    let mut orch = make_clean_orch(pool.clone(), run_id);
    let err = orch
        .tick()
        .await
        .expect_err("C1: tick() must return Err on unknown-order fill");

    // ── 4. Error string must identify the halt reason ─────────────────────
    let err_str = err.to_string();
    assert!(
        err_str.contains("UNKNOWN_ORDER_FILL"),
        "C1: error must contain 'UNKNOWN_ORDER_FILL'; got: {err_str}"
    );

    // ── 5. DB: run must be HALTED with halted_at_utc set ─────────────────
    //
    // If halt_run() had been best-effort (let _ = ...) and silently failed,
    // runs.status would still be RUNNING here and this assertion would fail.
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "C1: runs.status must be HALTED after unknown-order fill — \
         proves halt_run() write succeeded (mandatory, not best-effort)"
    );
    assert!(
        run.halted_at_utc.is_some(),
        "C1: halted_at_utc must be non-NULL after halt"
    );

    // ── 6. DB: arm state must be DISARMED / IntegrityViolation ───────────
    //
    // If persist_arm_state() had been best-effort and silently failed,
    // load_arm_state would return None here and the assertion would fail.
    let arm = mqk_db::load_arm_state(&pool)
        .await?
        .expect("C1: sys_arm_state must have a row after halt");
    assert_eq!(
        arm.0, "DISARMED",
        "C1: arm state must be DISARMED — proves persist_arm_state() write succeeded"
    );
    assert_eq!(
        arm.1.as_deref(),
        Some("IntegrityViolation"),
        "C1: disarm reason must be 'IntegrityViolation' to match the production halt path"
    );

    // ── 7+8. Fresh orchestrator for same run_id is refused by HALT_GUARD ─
    //
    // All gates pass (BoolGate(true)) — the ONLY thing that can block tick()
    // here is the DB-backed Phase 0 HALT_GUARD.  This simulates a daemon
    // restart: process B has no shared in-memory state with process A.
    //
    // If halt_run() had been best-effort and failed silently, runs.status
    // would still be RUNNING in DB and tick() would proceed past Phase 0,
    // potentially re-executing the halted run.
    let mut orch_fresh = make_clean_orch(pool.clone(), run_id);
    let fresh_err = orch_fresh
        .tick()
        .await
        .expect_err("C2: fresh orchestrator tick() must be refused for halted run");

    let fresh_err_str = fresh_err.to_string();
    assert!(
        fresh_err_str.contains("HALT_GUARD"),
        "C2: fresh orchestrator must be refused by HALT_GUARD (Phase 0 DB check), \
         not by a gate or in-memory state; got: {fresh_err_str}"
    );

    // ── Post-test cleanup ─────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    Ok(())
}
