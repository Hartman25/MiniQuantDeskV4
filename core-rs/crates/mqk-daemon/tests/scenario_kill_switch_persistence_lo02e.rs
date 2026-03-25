//! LO-02E: Kill-switch persistence and safe restart proof.
//!
//! Proves that the daemon kill-switch / halt / disarm control truth persists
//! correctly across restart boundaries and blocks unsafe restart-time
//! progression until explicitly cleared by operator action.
//!
//! # Relationship to existing coverage
//!
//! `scenario_kill_switch_guarantees.rs` (mqk-testkit) already proves:
//! - I1/I2: disarmed BrokerGateway blocks all order operations (execution layer)
//! - I4: halt persisted in `runs.status` refuses fresh ExecutionOrchestrator
//! - I5: explicit re-arm at execution layer restores trading
//! - IMHP: orchestrator-triggered halt mandatorily writes both DB records
//!
//! LO-02E adds the complementary **daemon control-plane** perspective:
//! - that `POST /v1/run/halt` persists `sys_arm_state=DISARMED/OperatorHalt`
//! - that a fresh AppState surfaces that durable truth via `current_status_snapshot`
//! - that the start gate is blocked fail-closedly at the integrity gate (not DB)
//! - that `POST /v1/integrity/arm` explicitly clears the kill-switch, after which
//!   start progresses past integrity gate (WS continuity becomes next blocker)
//! - that no-DB kill-switch is still fail-closed (in-memory disarmed default)
//!
//! # Key architectural property
//!
//! At daemon restart:
//! - Fresh `AppState` starts with `integrity.disarmed = true` (fail-closed default)
//! - The start gate reads in-memory state → blocked regardless of DB content
//! - `current_status_snapshot()` loads `sys_arm_state` from DB at status-read time
//!   → surfaces `integrity_armed=false` + `state="halted"` from DB truth
//! - These two layers are independent: DB does not ARM the in-memory gate
//!   (start protection is not DB-dependent); DB provides honest status reporting
//!
//! # Test matrix
//!
//! | ID    | DB? | Scenario                                         | Key assertion                                   |
//! |-------|-----|--------------------------------------------------|-------------------------------------------------|
//! | KS-01 | Yes | halt → fresh AppState reads DISARMED from DB     | integrity_armed=false, state="halted" from DB   |
//! | KS-02 | Yes | halt → fresh start blocked at integrity gate     | 403 gate=integrity_armed (in-memory fail-closed)|
//! | KS-03 | Yes | halt → explicit arm → start past integrity gate  | 403 gate=alpaca_ws_continuity, not integrity    |
//! | KS-04 | No  | no-DB fail-closed kill-switch                    | 403 gate=integrity_armed, no phantom DB clear   |
//!
//! # Run DB-backed tests
//!
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_kill_switch_persistence_lo02e \
//!     -- --test-threads 1

use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::routes;
use mqk_daemon::state::{AppState, BrokerKind, DeploymentMode};
use tokio::sync::{Semaphore, SemaphorePermit};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Within-binary DB-test serialization
// ---------------------------------------------------------------------------

/// Serializes DB-backed tests within this binary (1 permit).
/// KS-01/02/03 write to `sys_arm_state` (singleton, sentinel_id=1).
/// Without serialization they race on that singleton.
static DB_SEMA: OnceLock<Semaphore> = OnceLock::new();

fn db_sema() -> &'static Semaphore {
    DB_SEMA.get_or_init(|| Semaphore::new(1))
}

async fn db_serialize() -> SemaphorePermit<'static> {
    db_sema()
        .acquire()
        .await
        .expect("LO-02E: DB serialization semaphore poisoned")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("LO-02E: failed to connect to MQK_DATABASE_URL"),
    )
}

/// Connect, migrate, and clear kill-switch singleton + run state for a clean baseline.
///
/// Clears:
/// - `runs WHERE engine_id = 'mqk-daemon'`: stale runs could cause
///   `fetch_active_run_for_engine` to block `halt_execution_runtime` with
///   a `durable_active_without_local_owner` conflict.
/// - `sys_arm_state WHERE sentinel_id = 1`: each KS test asserts on the
///   exact state written by the in-test halt; stale ARMED/DISARMED rows
///   from prior tests would corrupt the assertion baseline.
///
/// `sys_reconcile_status_state` is intentionally NOT cleared (LO-02D-F1):
/// KS tests use Paper+Alpaca, but the reconcile gate is irrelevant to kill-switch
/// persistence because all KS tests are blocked before the reconcile gate
/// (at the integrity gate in KS-01/KS-02/KS-04, or at the WS gate in KS-03).
async fn setup_pool() -> Option<sqlx::PgPool> {
    let Some(pool) = db_pool_or_skip().await else {
        return None;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("LO-02E: migration failed");
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("LO-02E: delete runs failed");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("LO-02E: delete arm state failed");
    Some(pool)
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Issue `POST /v1/run/halt` against the given AppState and return the response.
async fn halt_via_http(st: &Arc<AppState>) -> (StatusCode, serde_json::Value) {
    call(
        routes::build_router(Arc::clone(st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/halt")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
}

/// Issue `POST /v1/integrity/arm` against the given AppState.
async fn arm_via_http(st: &Arc<AppState>) {
    let (status, json) = call(
        routes::build_router(Arc::clone(st)),
        Request::builder()
            .method("POST")
            .uri("/v1/integrity/arm")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "arm_via_http: arm must succeed; got: {json}"
    );
}

/// Issue `POST /v1/run/start` against the given AppState and return the response.
async fn start_via_http(st: &Arc<AppState>) -> (StatusCode, serde_json::Value) {
    call(
        routes::build_router(Arc::clone(st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
}

// ---------------------------------------------------------------------------
// KS-01: halt persists DISARMED/OperatorHalt; fresh AppState surfaces it from DB
// ---------------------------------------------------------------------------

/// LO-02E / KS-01: Kill-switch persistence — halt writes durable control truth to DB.
///
/// Proves that `POST /v1/run/halt` persists `sys_arm_state = DISARMED / OperatorHalt`
/// and that a fresh AppState (simulating a daemon restart) reads that truth back via
/// `current_status_snapshot()` — with no in-process injection.
///
/// Two independent sources of halt truth are surfaced:
/// - `integrity_armed = false` → loaded from `sys_arm_state.state = 'DISARMED'` (DB)
/// - `state = "halted"` → derived from `sys_arm_state.reason = 'OperatorHalt'` setting
///   `locally_halted = true` (DB, via `load_arm_state`)
///
/// This is the control-plane complement to `scenario_kill_switch_guarantees` I4:
/// I4 proves the execution orchestrator's HALT_GUARD reads `runs.status`;
/// KS-01 proves the daemon status route reads `sys_arm_state` — the control-plane
/// kill-switch record.
#[tokio::test]
async fn lo02e_ks01_halt_persists_disarmed_operator_halt_surfaces_on_fresh_restart() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("KS-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // AppState #1: represents the session that is halted.
    // Paper+Alpaca with DB; no active execution loop (simulates halt-a-stopped-system
    // or halt called immediately after a crash, before any new loop started).
    let st1 = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    // Assert halt returns 200 and the response already reflects halted state.
    let (halt_status, halt_json) = halt_via_http(&st1).await;
    assert_eq!(
        halt_status,
        StatusCode::OK,
        "KS-01: halt must succeed (200) even with no active run; \
         kill-switch is always actionable; got: {halt_json}"
    );
    assert_eq!(
        halt_json["state"], "halted",
        "KS-01: halt response must surface state='halted' immediately; got: {halt_json}"
    );
    assert_eq!(
        halt_json["integrity_armed"], false,
        "KS-01: halt response must surface integrity_armed=false; got: {halt_json}"
    );

    // AppState #2: simulates fresh daemon restart — new process, same DB.
    // NO injection: no arm call, no in-process halt call, no integrity write.
    let st2 = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );

    // Read status from DB — simulates what /api/v1/system/status surfaces at restart.
    let snapshot = st2
        .current_status_snapshot()
        .await
        .expect("KS-01: current_status_snapshot on fresh AppState must not error");

    assert_eq!(
        snapshot.integrity_armed, false,
        "KS-01: fresh AppState must surface integrity_armed=false from DB \
         (sys_arm_state=DISARMED persisted by halt); \
         this is DB truth, not in-memory default — proves durable kill-switch persistence; \
         got: {:?}",
        snapshot.integrity_armed
    );
    assert_eq!(
        snapshot.state, "halted",
        "KS-01: fresh AppState must surface state='halted' from DB \
         (sys_arm_state.reason=OperatorHalt → locally_halted=true); \
         no synthetic 'idle' or 'ready' semantics after kill-switch; \
         got: {:?}",
        snapshot.state
    );
}

// ---------------------------------------------------------------------------
// KS-02: after halt in DB, fresh start is blocked at integrity gate
// ---------------------------------------------------------------------------

/// LO-02E / KS-02: Kill-switch safe-restart — fresh start is blocked at integrity gate.
///
/// After `halt_via_http` persists `sys_arm_state=DISARMED/OperatorHalt`, a fresh
/// AppState (simulating daemon restart) must have start blocked at the integrity gate —
/// NOT at the DB gate or a later gate.
///
/// This proves a key architectural property:
/// - The start integrity gate reads **in-memory** `self.integrity.read().await.is_execution_blocked()`
/// - Fresh `AppState` starts with `integrity.disarmed = true` (fail-closed default)
/// - This fail-closed default fires BEFORE any DB operation is attempted
/// - The DB `sys_arm_state` record does NOT auto-arm the in-memory state
///   (which would be unsafe: an operator might want to inspect before re-arming)
///
/// The gate ordering proves: restart is safe even without DB access — the
/// in-memory fail-closed default is the primary protection, not DB loading.
#[tokio::test]
async fn lo02e_ks02_halt_in_db_fresh_start_blocked_at_integrity_gate_not_db_gate() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("KS-02: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Persist kill-switch state to DB via halt.
    let st1 = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let (halt_status, halt_json) = halt_via_http(&st1).await;
    assert_eq!(
        halt_status,
        StatusCode::OK,
        "KS-02: pre-test halt must succeed; got: {halt_json}"
    );

    // Fresh AppState: simulates daemon restart with DISARMED/OperatorHalt in DB.
    let st2 = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    // Start: must be refused at integrity gate (in-memory disarmed=true, default).
    // Paper+Alpaca deploy gate passes (start_allowed=true), so integrity is gate 2.
    let (start_status, start_json) = start_via_http(&st2).await;

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "KS-02: fresh start after halt must return 403 FORBIDDEN at integrity gate; \
         got: {start_status}, body: {start_json}"
    );
    assert_eq!(
        start_json["gate"], "integrity_armed",
        "KS-02: gate must be integrity_armed (in-memory fail-closed default); \
         must NOT be 'runtime_db' (which would mean start reached the DB gate, \
         proving the in-memory integrity gate did not fire); got: {start_json}"
    );
    assert_eq!(
        start_json["fault_class"], "runtime.control_refusal.integrity_disarmed",
        "KS-02: fault_class must be integrity_disarmed; got: {start_json}"
    );
    // Belt-and-suspenders: confirm it was NOT the DB gate (503 SERVICE_UNAVAILABLE).
    assert_ne!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "KS-02: must not reach DB gate (503) — integrity gate must fire first; \
         got: {start_status}"
    );
}

// ---------------------------------------------------------------------------
// KS-03: explicit arm after halt clears integrity gate; start reaches WS gate
// ---------------------------------------------------------------------------

/// LO-02E / KS-03: Kill-switch coherent clearing — explicit arm after halt restores
/// start progression past the integrity gate.
///
/// After halt persists `sys_arm_state=DISARMED/OperatorHalt`, an operator must
/// explicitly call `POST /v1/integrity/arm` to re-arm.  This proves:
///
/// 1. Arm clears the in-memory kill-switch (`integrity.disarmed=false, integrity.halted=false`)
///    AND persists `sys_arm_state=ARMED` to DB.
/// 2. After arm, the start integrity gate no longer fires — start progresses.
/// 3. For Paper+Alpaca, the next gate is WS continuity (ColdStartUnproven → 403
///    gate=alpaca_ws_continuity).  This proves integrity is no longer the blocker.
///
/// This test proves the clearing is coherent: the kill-switch is not permanently
/// non-resettable, but requires an explicit operator action.  The architecture does
/// not auto-arm on restart (proven by KS-02) and does not auto-arm on halt-then-arm
/// without DB (proven by KS-04).
#[tokio::test]
async fn lo02e_ks03_explicit_arm_after_halt_clears_integrity_gate_start_reaches_ws_gate() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("KS-03: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Step 1: persist kill-switch state via halt.
    let st_halted = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let (halt_status, halt_json) = halt_via_http(&st_halted).await;
    assert_eq!(
        halt_status,
        StatusCode::OK,
        "KS-03: pre-test halt must succeed; got: {halt_json}"
    );

    // Step 2: Fresh AppState — simulates restart after halt.
    // Confirm start is initially blocked at integrity gate (as proven by KS-02).
    let st2 = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let (pre_arm_status, pre_arm_json) = start_via_http(&st2).await;
    assert_eq!(
        pre_arm_status,
        StatusCode::FORBIDDEN,
        "KS-03: before arm, start must be blocked at integrity gate; got: {pre_arm_json}"
    );
    assert_eq!(
        pre_arm_json["gate"], "integrity_armed",
        "KS-03: pre-arm gate must be integrity_armed; got: {pre_arm_json}"
    );

    // Step 3: Explicit arm — operator clears the kill-switch.
    // arm_via_http sets in-memory disarmed=false + persists ARMED to DB.
    arm_via_http(&st2).await;

    // Step 4: Start again — integrity gate must no longer fire.
    // For Paper+Alpaca: WS continuity gate fires next (ColdStartUnproven → 403).
    let (post_arm_status, post_arm_json) = start_via_http(&st2).await;

    assert_eq!(
        post_arm_status,
        StatusCode::FORBIDDEN,
        "KS-03: after arm, start must still be refused (at WS gate, not integrity); \
         got: {post_arm_status}, body: {post_arm_json}"
    );
    assert_eq!(
        post_arm_json["gate"], "alpaca_ws_continuity",
        "KS-03: after arm, gate must be alpaca_ws_continuity — proving integrity gate \
         was cleared by explicit arm; integrity gate must not be the blocker anymore; \
         got: {post_arm_json}"
    );
    assert_ne!(
        post_arm_json["gate"], "integrity_armed",
        "KS-03: integrity gate must NOT fire after explicit arm; \
         if it does, arm did not clear the kill-switch; got: {post_arm_json}"
    );
}

// ---------------------------------------------------------------------------
// KS-04: no-DB fail-closed kill-switch — start blocked without any DB access
// ---------------------------------------------------------------------------

/// LO-02E / KS-04: Kill-switch fail-closed without DB — no phantom DB auto-clear.
///
/// Without a database (`AppState` has no DB pool), start must be blocked at the
/// integrity gate.  The fresh AppState default (`integrity.disarmed = true`) is
/// the protection — not DB loading.
///
/// This proves two things:
///
/// 1. The kill-switch is fail-closed even when the DB is unavailable at restart
///    (e.g. DB connection lost, offline mode).  The operator cannot get into a
///    "started despite DB being offline" situation.
///
/// 2. There is no phantom DB state that could auto-clear the kill-switch.
///    `sys_arm_state` is not loaded at start gate time (it's only loaded by
///    `current_status_snapshot()` for status display).  The start gate reads
///    in-memory state exclusively — a missing/unavailable DB cannot accidentally
///    allow start.
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02e_ks04_no_db_fail_closed_kill_switch_blocks_start_at_integrity_gate() {
    // Fresh AppState with no DB pool — simulates restart with DB offline/unconfigured.
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    // No arm, no injection: fresh disarmed default.
    let (status, json) = start_via_http(&st).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "KS-04: start without DB must return 403 FORBIDDEN at integrity gate; \
         got: {status}, body: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "KS-04: gate must be integrity_armed — in-memory fail-closed default; \
         no DB is needed to enforce the kill-switch; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.control_refusal.integrity_disarmed",
        "KS-04: fault_class must be integrity_disarmed; got: {json}"
    );
    // Confirm DB gate did NOT fire (which would mean integrity gate was bypassed).
    assert_ne!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "KS-04: must not reach DB gate (503) — integrity gate fires before DB is needed; \
         got: {status}"
    );
}
