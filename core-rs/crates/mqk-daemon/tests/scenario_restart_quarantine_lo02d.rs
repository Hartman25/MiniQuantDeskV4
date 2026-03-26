//! LO-02D: Restart quarantine / resume / refuse-to-resume proof.
//!
//! Proves that after hostile restart conditions, MiniQuantDesk:
//! - stays quarantined when required (halted lifecycle persists across restart)
//! - refuses unsafe fresh start with an explicit canonical fault_class
//! - does not auto-resume silently without operator action
//! - surfaces quarantine truth from durable DB state without injection
//! - does not fabricate ownership when DB is unavailable
//!
//! # Gap this patch closes
//!
//! LO-02A–C proved stressed recovery, shadow restart, and continuity/reconcile
//! boundary behavior.  But no patch proved:
//!
//! * that a HALTED run lifecycle is a durable quarantine that survives a full
//!   daemon restart and blocks any new start (all other gates pass, halted
//!   lifecycle fires last and is non-bypassable via restart)
//! * that status surfaces "halted" from durable DB truth on fresh restart
//!   without any in-process state injection (clean quarantine surface)
//! * that `restart_truth_snapshot()` correctly identifies durable-active-
//!   without-local-ownership at the DB restart boundary
//! * that `restart_truth_snapshot()` is safe (no phantom ownership) when DB
//!   is unavailable
//!
//! # Test matrix
//!
//! | ID    | Condition          | DB state          | Expected outcome                        |
//! |-------|--------------------|-------------------|-----------------------------------------|
//! | QR-01 | Halted lifecycle   | HALTED run in DB  | start refused: halted_lifecycle (409)   |
//! | QR-02 | Status surface     | HALTED run in DB  | state = "halted" (no injection)         |
//! | QR-03 | Orphan detection   | RUNNING run in DB | restart_truth: durable_active_mismatch  |
//! | QR-04 | No-DB fail-safe    | (no DB)           | restart_truth: no phantom ownership     |
//!
//! # QR-01: The cornerstone quarantine proof
//!
//! A HALTED run is a durable quarantine that the operator cannot bypass by
//! simply restarting the daemon process.  Uses `LiveShadow+Alpaca` so that the
//! WS and reconcile gates are skipped (both fire only for `Paper+Alpaca`),
//! making the halted_lifecycle gate the definitive blocker without reconcile
//! singleton dependencies.  The start gate sequence is:
//!
//! ```text
//! 1. deploy gate     → pass    (LiveShadow+Alpaca, start_allowed=true)
//! 2. integrity gate  → pass    (armed in memory)
//! 3. WS gate         → SKIPPED (LiveShadow+Alpaca)
//! 4. reconcile gate  → SKIPPED (LiveShadow+Alpaca)
//! 5. DB pool gate    → pass    (pool configured)
//! 6. active-run gate → pass    (HALTED ∉ {ARMED, RUNNING})
//! 7. latest-run gate → FIRES   (status=HALTED → halted_lifecycle)
//! ```
//!
//! The operator must explicitly clear the halted lifecycle before any new
//! execution is permitted.
//!
//! # DB-backed tests
//!
//! QR-01/02/03 require MQK_DATABASE_URL; they skip gracefully without it.
//! QR-04 is pure in-process (no DB required).
//!
//! Run DB tests with (--test-threads 1 required: DB tests share the runs table):
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_restart_quarantine_lo02d \
//!     -- --test-threads 1
//!
//! # Cross-binary parallel constraint
//!
//! LO-02D-F1: after switching QR-01/QR-02 to `LiveShadow+Alpaca`, LO-02D no
//! longer deletes `sys_reconcile_status_state`.  The reconcile-singleton race
//! with LO-02C has been eliminated.
//!
//! Residual singleton: LO-02D still deletes `sys_arm_state` (needed for QR-02's
//! `integrity_armed=false` assertion).  A concurrent binary actively arming in
//! the narrow window between QR-02's `setup_pool` call and its
//! `current_status_snapshot()` call could cause a spurious `integrity_armed=true`.
//! In practice the test suite does not leave `sys_arm_state=ARMED` as a durable
//! persistent state, so concurrent binaries are generally safe.  For guaranteed
//! isolation run LO-02D in a separate invocation:
//!
//! ```text
//! cargo test -p mqk-daemon --test scenario_restart_quarantine_lo02d -- --test-threads 1
//! ```
//!
//! CI is unaffected (MQK_DATABASE_URL absent → all DB tests skip).

use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::routes;
use mqk_daemon::state::{AppState, BrokerKind, DeploymentMode};
use tokio::sync::{Semaphore, SemaphorePermit};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Within-binary DB-test serialization
// ---------------------------------------------------------------------------

/// Global semaphore (1 permit) that serializes DB-backed tests within this
/// binary.  Without serialization, QR-01/QR-02/QR-03 run in parallel and
/// race on the shared singleton DB tables (sys_arm_state,
/// sys_reconcile_status_state, runs), causing spurious failures.
///
/// Each DB-backed test acquires `db_serialize().await` at the top and holds
/// the permit for the test's lifetime (auto-dropped on return).  QR-04 is
/// in-process (no DB) and does not need the lock.
static DB_SEMA: OnceLock<Semaphore> = OnceLock::new();

fn db_sema() -> &'static Semaphore {
    DB_SEMA.get_or_init(|| Semaphore::new(1))
}

async fn db_serialize() -> SemaphorePermit<'static> {
    db_sema()
        .acquire()
        .await
        .expect("LO-02D: DB serialization semaphore poisoned")
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
            .expect("LO-02D: failed to connect to MQK_DATABASE_URL"),
    )
}

/// Connect, migrate, and clean singleton/run state for a fresh test baseline.
///
/// Cleans two tables:
/// - `runs`: each QR test inserts HALTED or RUNNING rows; stale rows from
///   prior QR runs would cause `fetch_latest_run_for_engine` to return a
///   wrong run (e.g. a stale HALTED run from a prior QR-01 contaminating
///   QR-02 if setup_pool is not called between tests).
/// - `sys_arm_state`: QR-02 expects `integrity_armed=false` (fresh boot =
///   disarmed); the singleton must be absent on entry so that
///   `current_status_snapshot()` does not pick up a stale ARMED row.
///
/// `sys_reconcile_status_state` is intentionally NOT cleared here (LO-02D-F1).
/// QR-01/QR-02 use `LiveShadow+Alpaca`; the reconcile gate fires only for
/// `Paper+Alpaca`, so LO-02D tests are unaffected by reconcile singleton state.
/// This eliminates the cross-binary race with LO-02C, which seeds dirty reconcile
/// state for its RC tests.
async fn setup_pool() -> Option<sqlx::PgPool> {
    let _serial = db_serialize().await;
    let pool = db_pool_or_skip().await?;
    mqk_db::migrate(&pool)
        .await
        .expect("LO-02D: migration failed");
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("LO-02D: clear runs failed");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("LO-02D: clear arm state failed");
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

/// Arm the integrity gate directly in memory, bypassing the HTTP arm route.
///
/// QR-01/QR-02 use `LiveShadow+Alpaca`; the integrity arm is a required
/// PRECONDITION for QR-01, not the behavior under test.
///
/// We bypass the HTTP route (and its internal `current_status_snapshot()` call)
/// intentionally: `current_status_snapshot()` calls `deadman_truth_for_run` on
/// any RUNNING run it finds in the DB for the same mode.  Although QR-01/QR-02
/// use LiveShadow mode (which only sees LIVE-SHADOW runs) and QR-03's orphan is
/// PAPER mode, using `arm_in_memory` keeps the arm precondition simple,
/// fast, and free of any DB write to `sys_arm_state`.
///
/// Direct manipulation is safe: the integrity HTTP route is proven by dedicated
/// scenario_daemon_routes.rs tests.  This helper only satisfies the gate
/// precondition.
async fn arm_in_memory(st: &Arc<AppState>) {
    let mut ig = st.integrity.write().await;
    ig.disarmed = false;
    ig.halted = false;
}

/// Insert a HALTED run into DB, simulating a prior session that was halted.
///
/// Performs the full lifecycle: Created → Armed → Running → Halted.
/// This mirrors what `halt_execution_runtime` does in production.
async fn insert_halted_run(pool: &sqlx::PgPool, mode: &str) -> Uuid {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: mode.to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "lo02d-test-config".to_string(),
            config_json: serde_json::json!({"mode": mode}),
            host_fingerprint: "lo02d-test-host".to_string(),
        },
    )
    .await
    .expect("LO-02D: insert_run failed");
    mqk_db::arm_run(pool, run_id)
        .await
        .expect("LO-02D: arm_run failed");
    mqk_db::begin_run(pool, run_id)
        .await
        .expect("LO-02D: begin_run failed");
    mqk_db::halt_run(pool, run_id, Utc::now())
        .await
        .expect("LO-02D: halt_run failed");
    run_id
}

/// Insert a RUNNING (orphaned) run into DB, simulating a prior session crash.
///
/// Performs Created → Armed → Running but NOT stop/halt, leaving the run
/// in RUNNING state with no local execution loop owner.
async fn insert_running_orphan(pool: &sqlx::PgPool, mode: &str) -> Uuid {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: mode.to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "lo02d-test-config".to_string(),
            config_json: serde_json::json!({"mode": mode}),
            host_fingerprint: "lo02d-test-host".to_string(),
        },
    )
    .await
    .expect("LO-02D: insert_run (orphan) failed");
    mqk_db::arm_run(pool, run_id)
        .await
        .expect("LO-02D: arm_run (orphan) failed");
    mqk_db::begin_run(pool, run_id)
        .await
        .expect("LO-02D: begin_run (orphan) failed");
    // No stop or halt — run is orphaned in RUNNING state.
    run_id
}

// ---------------------------------------------------------------------------
// QR-01: Halted lifecycle quarantine persists across daemon restart
// ---------------------------------------------------------------------------

/// LO-02D / QR-01: HALTED lifecycle is a durable quarantine that survives restart.
///
/// A HALTED run cannot be bypassed by simply restarting the daemon process.
/// Uses `LiveShadow+Alpaca` (LO-02D-F1) so the WS and reconcile gates are
/// skipped (both fire only for `Paper+Alpaca`), making the halted_lifecycle
/// gate the definitive blocker without any reconcile singleton dependency.
///
/// All prior start gates pass:
///   - integrity armed (in memory)
///   - WS gate: SKIPPED (LiveShadow+Alpaca)
///   - reconcile gate: SKIPPED (LiveShadow+Alpaca)
///   - DB pool configured and reachable
///   - `fetch_active_run_for_engine` returns None (HALTED ∉ {ARMED, RUNNING})
///
/// But the latest-run status check fires:
///   - `fetch_latest_run_for_engine` returns the HALTED LIVE-SHADOW run
///   - Gate refuses: `runtime.start_refused.halted_lifecycle`
///
/// The operator MUST explicitly clear the halted lifecycle state before any
/// new execution is permitted.  Restart alone does not escape quarantine.
#[tokio::test]
async fn lo02d_qr01_halted_lifecycle_quarantine_survives_daemon_restart() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("QR-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Prior session: insert a LIVE-SHADOW run and halt it.
    insert_halted_run(&pool, "LIVE-SHADOW").await;

    // Fresh restart: LiveShadow+Alpaca AppState with DB configured.
    // LiveShadow+Alpaca: deploy gate passes (start_allowed=true);
    // WS and reconcile gates are skipped (Paper+Alpaca only).
    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));

    // Clear Gate 2: arm in memory (no DB write; does not call current_status_snapshot).
    arm_in_memory(&st).await;

    // Gate 3 (WS continuity): SKIPPED for LiveShadow+Alpaca.
    // Gate 4 (reconcile): SKIPPED for LiveShadow+Alpaca.
    // Gate 5 (DB pool) passes: pool is configured.
    // Gate 6 (active-run): HALTED is not ARMED/RUNNING → passes.
    // Gate 7 (latest-run status): HALTED → fires.

    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "QR-01: halted lifecycle must block start with 409 CONFLICT; got: {status}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.halted_lifecycle",
        "QR-01: fault_class must be halted_lifecycle; got: {json}"
    );
    assert!(
        json["error"].as_str().unwrap_or("").contains("halted"),
        "QR-01: error message must reference the halted condition; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// QR-02: Status surfaces "halted" from DB truth on fresh restart, no injection
// ---------------------------------------------------------------------------

/// LO-02D / QR-02: Fresh restart reads "halted" quarantine state from durable DB.
///
/// After a halt, a fresh daemon process must surface `state = "halted"` in the
/// status snapshot — read from the durable HALTED run in the DB — without any
/// in-process state injection.
///
/// This proves the quarantine state is faithfully surfaced to the operator,
/// not hidden or auto-recovered.  The system does not claim a nominal state
/// that would mislead the operator into thinking restart was safe.
///
/// The distinction from `hostile_restart_with_poisoned_local_cache_...` in
/// scenario_daemon_runtime_lifecycle.rs: no in-memory local run ownership is
/// injected.  Pure clean boot + durable HALTED truth.
#[tokio::test]
async fn lo02d_qr02_halted_status_surfaces_from_durable_db_truth_without_injection() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("QR-02: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Prior session: halt a LIVE-SHADOW run (matches LiveShadow AppState mode filter).
    insert_halted_run(&pool, "LIVE-SHADOW").await;

    // Fresh restart: no injection, no arm, no WS continuity injection.
    // LiveShadow+Alpaca so fetch_latest_run_for_engine finds the LIVE-SHADOW run.
    let st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    );

    // Read status snapshot directly — simulates what /api/v1/system/status surfaces.
    let snapshot = st
        .current_status_snapshot()
        .await
        .expect("QR-02: current_status_snapshot failed");

    assert_eq!(
        snapshot.state, "halted",
        "QR-02: fresh AppState must surface state='halted' from durable HALTED run \
         in DB, without any in-process injection; got: {:?}",
        snapshot.state
    );
    // Integrity must NOT be claimed as armed — quarantine is also integrity-level.
    assert!(
        !snapshot.integrity_armed,
        "QR-02: integrity_armed must be false after halt (quarantine is integrity-level too); \
         got: {:?}",
        snapshot.integrity_armed
    );
}

// ---------------------------------------------------------------------------
// QR-03: restart_truth_snapshot detects orphaned RUNNING run at restart boundary
// ---------------------------------------------------------------------------

/// LO-02D / QR-03: `restart_truth_snapshot()` identifies durable-active-without-
/// local-ownership for an orphaned RUNNING run at the DB restart boundary.
///
/// When a prior session crashed (run left in RUNNING state with no local owner),
/// a fresh daemon process must have `restart_truth_snapshot()` report:
///   - `durable_active_run_id = Some(the_run_id)`
///   - `local_owned_run_id = None`
///   - `durable_active_without_local_ownership = true`
///
/// This is the foundational restart truth detector.  The same condition triggers
/// the `runtime.truth_mismatch.durable_active_without_local_owner` gate in
/// `start_execution_runtime` (SR-10 proves that gate; QR-03 proves the
/// detector underneath it is accurate at the DB boundary).
///
/// Proving detector accuracy means: the gate refusal is grounded in real restart
/// truth, not a false positive; and the operator receives an honest mismatch
/// signal, not synthetic safe-looking state.
#[tokio::test]
async fn lo02d_qr03_restart_truth_snapshot_detects_orphaned_running_run() {
    let _serial = db_serialize().await;
    let Some(pool) = setup_pool().await else {
        eprintln!("QR-03: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    // Prior session: crash simulation — run left in RUNNING state, no stop.
    let orphaned_run_id = insert_running_orphan(&pool, "PAPER").await;

    // Fresh restart: no local execution loop, no injection.
    // Clone pool so it remains available for post-test cleanup below.
    let st = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );

    let truth = st
        .restart_truth_snapshot()
        .await
        .expect("QR-03: restart_truth_snapshot failed");

    assert!(
        truth.durable_active_without_local_ownership,
        "QR-03: restart_truth must report durable_active_without_local_ownership=true \
         for orphaned RUNNING run; got: durable={:?}, local={:?}",
        truth.durable_active_run_id, truth.local_owned_run_id
    );
    assert_eq!(
        truth.durable_active_run_id,
        Some(orphaned_run_id),
        "QR-03: durable_active_run_id must be the orphaned run_id"
    );
    assert_eq!(
        truth.local_owned_run_id, None,
        "QR-03: local_owned_run_id must be None (fresh boot, no local execution loop)"
    );

    // Cleanup: halt the orphan so it no longer appears RUNNING in the DB.
    //
    // Without this, `current_status_snapshot()` in any subsequent test that
    // calls `arm_via_http` (which internally calls `current_status_snapshot`)
    // would find a RUNNING run and invoke `deadman_truth_for_run`.  For an
    // orphan with no heartbeat the deadman fires immediately and sets
    // `integrity.disarmed=true` in-memory — poisoning concurrent binaries
    // (e.g. BRK-07R D06, LO-02C RC-01) that call arm_via_http while this
    // run is still RUNNING in the DB.
    //
    // Transitioning the orphan to HALTED prevents the deadman code path from
    // firing on it.  The halted branch in current_status_snapshot does NOT
    // call deadman_truth_for_run.
    mqk_db::halt_run(&pool, orphaned_run_id, Utc::now())
        .await
        .expect("QR-03: post-test halt of orphan failed");
}

// ---------------------------------------------------------------------------
// QR-04: restart_truth_snapshot is safe without DB — no phantom ownership
// ---------------------------------------------------------------------------

/// LO-02D / QR-04: `restart_truth_snapshot()` returns no-ownership defaults
/// when DB is unavailable.
///
/// When the DB pool is not configured (e.g. early boot, offline mode, or test
/// without MQK_DATABASE_URL), `restart_truth_snapshot()` must not fabricate
/// ownership or conflict.  All fields must reflect "no active run known."
///
/// This is a fail-safe proof: DB unavailability must not cause the system to
/// incorrectly believe it is conflicted with a durable run or owns active
/// execution authority.  A false positive here would expose the operator to
/// misleading quarantine signals.
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02d_qr04_restart_truth_snapshot_no_phantom_ownership_without_db() {
    let st = AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);

    let truth = st
        .restart_truth_snapshot()
        .await
        .expect("QR-04: restart_truth_snapshot failed");

    assert_eq!(
        truth.durable_active_run_id, None,
        "QR-04: durable_active_run_id must be None without DB \
         (no phantom active run may be claimed)"
    );
    assert_eq!(
        truth.local_owned_run_id, None,
        "QR-04: local_owned_run_id must be None (fresh boot, no execution loop)"
    );
    assert!(
        !truth.durable_active_without_local_ownership,
        "QR-04: durable_active_without_local_ownership must be false without DB \
         (no conflict can be reported without DB truth)"
    );
}
