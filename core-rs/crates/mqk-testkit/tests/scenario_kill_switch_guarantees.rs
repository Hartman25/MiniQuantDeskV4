//! Scenario: Kill-Switch Guarantees — Section I
//!
//! # Mission
//!
//! Prove that a halt/disarm event prevents ALL trading activity everywhere, and
//! that the halt is durable across daemon restart.
//!
//! # Invariants under test
//!
//! **I1** — Halt blocks new orders: `BrokerGateway::submit` returns
//! `GateRefusal::IntegrityDisarmed` when `IntegrityGate::is_armed()` is false.
//!
//! **I2** — Halt blocks cancel/replace: `cancel` and `replace` also return
//! `GateRefusal::IntegrityDisarmed` — they use the same `enforce_gates()`
//! pre-check as submit, so disarm is uniformly applied.
//!
//! **I4** — Halt is sticky across restart: a *new* `ExecutionOrchestrator`
//! instance constructed for a `run_id` that was HALTED in the DB is refused by
//! the Phase 0 HALT_GUARD at the top of `tick()` — before any outbox claim,
//! gateway call, or inbox apply.  This proves the guard reads from DB on every
//! tick, not from in-memory state.
//!
//! **I5** — Explicit re-arm restores trading: after the integrity gate has been
//! disarmed, constructing a gateway with `IntegrityGate::is_armed() = true`
//! (the explicit operator re-arm step) restores submit, cancel, and replace.
//!
//! # Test matrix
//!
//! | Test | Invariant | DB? |
//! |------|-----------|-----|
//! | `i1_i2_disarmed_gateway_blocks_all_trading_operations` | I1, I2 | No |
//! | `i5_rearm_restores_all_trading_operations` | I5 | No |
//! | `i4_halt_in_db_refuses_new_orchestrator_after_restart` | I4 | Yes (skip) |
//!
//! # I3 note
//!
//! I3 (dispatch loop stops after halt) is proven by
//! `scenario_invariant_violation_halts_and_persists` S2: a halted run refuses
//! a subsequent tick on the same orchestrator instance via the Phase 0
//! HALT_GUARD.  The guard reads `runs.status` from DB — not in-memory state —
//! so the proof is equivalent to a new-instance test.  I4 (below) extends this
//! to a new orchestrator instance to complete the restart proof.
//!
//! # I6 note
//!
//! I6 (no background bypass) is enforced structurally and requires no runtime
//! test:
//!
//! 1. `BrokerInvokeToken` is `pub(crate)` — cannot be manufactured outside
//!    `mqk-execution`.  Any background task without the token cannot call broker
//!    methods.
//! 2. `BrokerGateway` is the only path to `BrokerInvokeToken` and enforces
//!    gates on every call.
//! 3. `wiring_paper.rs` (which exposes `PassGate`) is gated
//!    `#[cfg(any(test, feature = "testkit"))]` — unreachable in production
//!    binaries.
//! 4. `tick()` is the ONLY production call site for `gateway.submit()`.
//!    There are no background workers that call `tick()` concurrently;
//!    the daemon's `spawn_execution_loop` runs a single sequential loop.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::Utc;
use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
    SubmitError,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

/// Broker that unconditionally accepts everything.
///
/// Gate enforcement — not broker behaviour — is what Section I tests.
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
/// `integrity: false`  simulates DISARMED (halt state).
/// `integrity: true`   simulates ARMED   (re-armed state).
///
/// Risk and reconcile are always `true` in these tests so the integrity gate
/// is the isolated variable.
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
// Helpers — pure in-process
// ---------------------------------------------------------------------------

/// Build a gateway with the given integrity state.
/// Risk + reconcile always pass so integrity is the only variable.
fn make_gateway(integrity: bool) -> BrokerGateway<OkBroker, BoolGate, BoolGate, BoolGate> {
    BrokerGateway::for_test(
        OkBroker,
        BoolGate(integrity),
        BoolGate(true),
        BoolGate(true),
    )
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-i-001".to_string(),
        symbol: "SPY".to_string(),
        side: mqk_execution::Side::Buy,
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

fn claim_token() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-i-001")
}

fn empty_map() -> BrokerOrderMap {
    BrokerOrderMap::new()
}

fn registered_map() -> BrokerOrderMap {
    let mut m = BrokerOrderMap::new();
    m.register("ord-i-001", "b-ord-i-001");
    m
}

// ---------------------------------------------------------------------------
// I1 + I2 — disarmed gateway blocks all trading operations (pure in-process)
// ---------------------------------------------------------------------------

/// I1: `BrokerGateway::submit` returns `GateRefusal::IntegrityDisarmed` when
/// the `IntegrityGate` reports disarmed (simulating halt state).
///
/// I2: `cancel` and `replace` return the same refusal — `enforce_gates()` runs
/// before every broker operation, uniformly blocking all trading paths.
///
/// The gate fires before the order-map lookup, so an empty map is sufficient
/// for cancel/replace (the test never reaches map resolution).
#[test]
fn i1_i2_disarmed_gateway_blocks_all_trading_operations() {
    let gw = make_gateway(false); // integrity = DISARMED
    let map = empty_map();

    // I1 — submit is blocked.
    let submit_err = gw.submit(&claim_token(), submit_req()).unwrap_err();
    let SubmitError::Gate(submit_refusal) = submit_err else {
        panic!("I1: submit error must be SubmitError::Gate, got {submit_err:?}")
    };
    assert_eq!(
        submit_refusal,
        GateRefusal::IntegrityDisarmed,
        "I1: submit must return IntegrityDisarmed when disarmed"
    );

    // I2 — cancel is blocked.
    let cancel_err = gw
        .cancel("ord-i-001", &map)
        .unwrap_err()
        .downcast::<GateRefusal>()
        .expect("I2: cancel error must be GateRefusal");
    assert_eq!(
        *cancel_err,
        GateRefusal::IntegrityDisarmed,
        "I2: cancel must return IntegrityDisarmed when disarmed"
    );

    // I2 — replace is blocked.
    let replace_err = gw
        .replace("ord-i-001", &map, 20, None, "day".to_string())
        .unwrap_err()
        .downcast::<GateRefusal>()
        .expect("I2: replace error must be GateRefusal");
    assert_eq!(
        *replace_err,
        GateRefusal::IntegrityDisarmed,
        "I2: replace must return IntegrityDisarmed when disarmed"
    );
}

// ---------------------------------------------------------------------------
// I5 — explicit re-arm restores all trading operations (pure in-process)
// ---------------------------------------------------------------------------

/// I5 (positive path): after disarm, explicitly re-arming the gate (operator
/// action — constructing or updating the gateway with an armed IntegrityGate)
/// restores submit, cancel, and replace.
///
/// The two-step structure proves the gate is the single control point:
///
/// Step 1: disarmed gate → all three operations refused.
/// Step 2: armed gate   → all three operations succeed.
///
/// In production, re-arm requires an explicit operator call (`ArmState::arm()`)
/// followed by starting a new run.  Restart alone never re-arms (proven by
/// `scenario_restart_defaults_to_disarmed`).
#[test]
fn i5_rearm_restores_all_trading_operations() {
    // Step 1: disarmed — all operations blocked.
    let disarmed = make_gateway(false);
    assert!(
        disarmed.submit(&claim_token(), submit_req()).is_err(),
        "I5: submit must be blocked before re-arm"
    );
    assert!(
        disarmed.cancel("ord-i-001", &empty_map()).is_err(),
        "I5: cancel must be blocked before re-arm"
    );
    assert!(
        disarmed
            .replace("ord-i-001", &empty_map(), 20, None, "day".to_string())
            .is_err(),
        "I5: replace must be blocked before re-arm"
    );

    // Step 2: re-armed — all operations succeed.
    let armed = make_gateway(true);
    let m = registered_map();
    assert!(
        armed.submit(&claim_token(), submit_req()).is_ok(),
        "I5: submit must succeed after explicit re-arm"
    );
    assert!(
        armed.cancel("ord-i-001", &m).is_ok(),
        "I5: cancel must succeed after explicit re-arm"
    );
    assert!(
        armed
            .replace("ord-i-001", &m, 20, None, "day".to_string())
            .is_ok(),
        "I5: replace must succeed after explicit re-arm"
    );
}

// ---------------------------------------------------------------------------
// DB helpers (I4 test)
// ---------------------------------------------------------------------------

/// Fixed run UUID for the I4 restart test.
const I4_RUN_ID: &str = "14000001-0000-0000-0000-000000000000";

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
            engine_id: "i4-kill-switch".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "i4-test".to_string(),
            config_hash: "i4-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "i4-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Construct a fresh `ExecutionOrchestrator` with all gates passing and a clean
/// portfolio.  This simulates process B starting from scratch with no in-memory
/// knowledge of what process A did.
fn make_fresh_orchestrator(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<OkBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    // All gates pass so that the HALT_GUARD (Phase 0, DB-backed) is the only
    // thing that can block tick().  If the test relied on in-memory gate state
    // it would not prove the restart invariant.
    let gateway = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), BoolGate(true));

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "i4-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(mqk_reconcile::BrokerSnapshot::empty),
    )
}

// ---------------------------------------------------------------------------
// I4 — halt persisted in DB refuses a brand-new orchestrator (DB-backed)
// ---------------------------------------------------------------------------

/// I4: Simulates the critical restart scenario:
///
/// - Process A halts a run and writes `runs.status = 'HALTED'` to the DB
///   (e.g. due to an invariant violation, reconcile drift, or operator kill).
/// - Process A crashes / exits.
/// - Process B (daemon restart) constructs a **new** `ExecutionOrchestrator`
///   instance for the same `run_id`.  It has no shared in-memory state with
///   process A.
/// - Process B calls `tick()`.
/// - The Phase 0 HALT_GUARD reads `run_id` status from DB → sees `HALTED` →
///   returns `Err("HALT_GUARD: …")` before any outbox claim, gateway call,
///   or inbox apply.
///
/// This proves that halt is sticky across restart and that the HALT_GUARD
/// mechanism is DB-backed — not dependent on in-memory state surviving a crash.
///
/// Requires `MQK_DATABASE_URL`. Skips gracefully if absent or unreachable.
#[tokio::test]
async fn i4_halt_in_db_refuses_new_orchestrator_after_restart() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = I4_RUN_ID.parse().expect("I4_RUN_ID must be a valid UUID");

    // ── Pre-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;

    // ── Process A: seed and run a normal RUNNING run, then halt it ────────
    //
    // We use `halt_run()` directly rather than triggering an invariant
    // violation.  This is the general case — halt can be triggered by any
    // path (invariant, reconcile drift, deadman, operator kill switch).
    seed_running_run(&pool, run_id).await?;
    mqk_db::halt_run(&pool, run_id, Utc::now()).await?;
    mqk_db::persist_arm_state(&pool, "DISARMED", Some("ManualDisarm")).await?;

    // Confirm halt is in DB before constructing the restarted orchestrator.
    let run_before = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run_before.status, mqk_db::RunStatus::Halted),
        "I4: run must be HALTED in DB before restart simulation"
    );

    // ── Process B (restart): fresh orchestrator, no shared in-memory state ─
    //
    // All gates pass (BoolGate(true)).  The ONLY thing that can block tick()
    // is the DB-backed Phase 0 HALT_GUARD.
    let mut orch_b = make_fresh_orchestrator(pool.clone(), run_id);

    // ── tick() must be refused immediately by HALT_GUARD (Phase 0) ────────
    let err = orch_b
        .tick()
        .await
        .expect_err("I4: tick() on restarted orchestrator must be refused for halted run");

    let err_str = err.to_string();
    assert!(
        err_str.contains("HALT_GUARD"),
        "I4: error must be from HALT_GUARD (Phase 0 DB check), not a gate or invariant; \
         got: {err_str}"
    );

    // ── Arm state must still be DISARMED after the refused tick ───────────
    //
    // Proves the refused tick did not accidentally overwrite the disarm reason.
    let arm = mqk_db::load_arm_state(&pool)
        .await?
        .expect("I4: sys_arm_state must have a row");
    assert_eq!(
        arm.0, "DISARMED",
        "I4: arm state must remain DISARMED after refused tick"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hardening proof — orchestrator-triggered halt writes both DB records
// ---------------------------------------------------------------------------

/// Fixed run UUID for the mandatory-halt-persistence test.
const IMHP_RUN_ID: &str = "1a1a0001-0000-0000-0000-000000000000";

/// Construct an orchestrator whose portfolio is pre-corrupted so that the
/// capital-invariant check fires on the first inbox event it processes.
///
/// `cash_micros` is set one micro below `initial_capital_micros` while the
/// ledger is empty.  `recompute_from_ledger` will return the original
/// `initial_capital_micros`, so the invariant check detects the mismatch.
///
/// All gates pass (`BoolGate(true)`) so the ONLY path to halt is the
/// capital-invariant check inside the apply loop (Phase 3b / I9-1 path).
fn make_corrupted_orch_for_i(
    pool: PgPool,
    run_id: Uuid,
) -> ExecutionOrchestrator<OkBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), BoolGate(true));

    let mut portfolio = PortfolioState::new(1_000_000_000_i64);
    // Corrupt the cash balance by −1 micro without a matching ledger entry.
    // recompute_from_ledger returns 1_000_000_000; the state holds 999_999_999
    // → cash_micros mismatch → invariant violation on the first inbox apply.
    portfolio.cash_micros -= 1;

    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        portfolio,
        run_id,
        "imhp-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(mqk_reconcile::BrokerSnapshot::empty),
    )
}

/// Hardening proof — orchestrator-triggered halt writes both DB records
/// mandatorily, and a fresh orchestrator for the same run_id is refused.
///
/// Sequence:
///
/// 1. Seed a RUNNING run.
/// 2. Insert one unapplied inbox Ack (triggers the apply loop; Ack carries no
///    fill so the portfolio is not mutated, only the invariant check fires).
/// 3. Construct an orchestrator with a pre-corrupted portfolio.
/// 4. `tick()` → capital invariant violation → `persist_halt_and_disarm` must
///    succeed before the error is returned.
/// 5. Assert `runs.status = HALTED` and `halted_at_utc IS NOT NULL`.
/// 6. Assert `sys_arm_state = ('DISARMED', 'IntegrityViolation')`.
/// 7. Construct a BRAND-NEW orchestrator for the same `run_id` (all gates pass).
/// 8. `tick()` → Phase-0 HALT_GUARD reads DB → refused immediately.
///
/// Step 8 is only possible because step 4's `halt_run` write succeeded —
/// proving the write is mandatory and not best-effort.
///
/// Requires `MQK_DATABASE_URL`. Skips gracefully if absent or unreachable.
#[tokio::test]
async fn i_orchestrator_triggered_halt_proves_mandatory_writes() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = IMHP_RUN_ID
        .parse()
        .expect("IMHP_RUN_ID must be a valid UUID");

    // ── Pre-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    // Clear the arm-state singleton so our assertions are unambiguous.
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await?;

    // ── 1. Seed a RUNNING run ─────────────────────────────────────────────
    seed_running_run(&pool, run_id).await?;

    // ── 2. Insert an unapplied inbox Ack to trigger the apply loop ────────
    //
    // An Ack event carries no fill — the portfolio is not mutated — so the
    // invariant check fires on the pre-corrupted cash_micros value.
    let msg_json = serde_json::json!({
        "type":              "ack",
        "broker_message_id": "imhp-msg-001",
        "internal_order_id": "imhp-ord-001"
    });
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "imhp-msg-001", msg_json).await?;
    assert!(inserted, "IMHP: inbox Ack row must be inserted");

    // ── 3. tick() must fail — invariant violation triggers mandatory halt ──
    let mut orch = make_corrupted_orch_for_i(pool.clone(), run_id);
    let err = orch
        .tick()
        .await
        .expect_err("IMHP: tick() must return Err on capital invariant violation");

    let err_str = err.to_string();
    assert!(
        err_str.contains("INVARIANT_VIOLATED"),
        "IMHP: error must contain 'INVARIANT_VIOLATED'; got: {err_str}"
    );

    // ── 4+5. DB: run must be HALTED with halted_at_utc set ───────────────
    let run = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "IMHP: runs.status must be HALTED after invariant violation — \
         proves halt_run() write succeeded (mandatory, not best-effort)"
    );
    assert!(
        run.halted_at_utc.is_some(),
        "IMHP: halted_at_utc must be non-NULL after halt"
    );

    // ── 6. DB: arm state must be DISARMED / IntegrityViolation ───────────
    let arm = mqk_db::load_arm_state(&pool)
        .await?
        .expect("IMHP: sys_arm_state must have a row after halt");
    assert_eq!(
        arm.0, "DISARMED",
        "IMHP: arm state must be DISARMED — proves persist_arm_state() write succeeded"
    );
    assert_eq!(
        arm.1.as_deref(),
        Some("IntegrityViolation"),
        "IMHP: disarm reason must be 'IntegrityViolation'"
    );

    // ── 7+8. Fresh orchestrator for same run_id is refused by HALT_GUARD ─
    //
    // All gates pass (BoolGate(true)) so the HALT_GUARD is the only blocker.
    // If halt_run() had been best-effort (and silently failed), runs.status
    // would still be RUNNING here and tick() would proceed past Phase 0.
    let mut orch_fresh = make_fresh_orchestrator(pool.clone(), run_id);
    let fresh_err = orch_fresh
        .tick()
        .await
        .expect_err("IMHP: fresh orchestrator tick() must be refused for halted run");

    let fresh_err_str = fresh_err.to_string();
    assert!(
        fresh_err_str.contains("HALT_GUARD"),
        "IMHP: fresh orchestrator must be refused by HALT_GUARD (Phase 0 DB check); \
         got: {fresh_err_str}"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}
