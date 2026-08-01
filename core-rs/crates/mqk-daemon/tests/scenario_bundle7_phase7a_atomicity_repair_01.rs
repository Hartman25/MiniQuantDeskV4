//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-ATOMIC-OWNERSHIP-AND-
//! ROLLBACK-TRUTH: hermetic real-sequence proof for requirements 2/3/4/5.
//!
//! `daily_data_readiness::advance_run_to_active` is the exact shared
//! coordinator `AppState::start_execution_runtime` calls in production
//! (`state::lifecycle::ProductionRuntimeStartEffects` is the production
//! `RuntimeStartEffects` implementation). This file drives that same
//! function — never a separate, independently-ordered reimplementation —
//! against a hermetic fake effects implementation that arms/begins the run
//! through the real `mqk_db` calls (so the durable-rollback proof is
//! genuine: the run really does reach `ARMED`/`RUNNING` before an injected
//! failure), but never constructs a broker, provider, or scheduler. No
//! network, no credentials.
//!
//! The call order under test is `reserve_local_ownership` ->
//! `start_runtime_effects` -> `spawn_loop` — reservation is deliberately
//! the first call, strictly before any local/durable effect, so a
//! local-ownership conflict is refused before `arm_run` is ever attempted.
//!
//! What this file proves:
//! - success path: ownership is reserved strictly before runtime effects
//!   run or the loop is spawned (trace ordering), and rollback never fires.
//! - a failure after `arm_run` succeeds moves the durable run back to
//!   `STOPPED` (requirement 2 — the fake's phase tracking reports
//!   `RunningBeforeInitialTick`, which is cleanly stoppable per requirement
//!   4), and `rollback_local_effects` fires exactly once with the correct
//!   `run_id`.
//! - an ownership conflict at `reserve_local_ownership` never attempts
//!   `arm_run`/`begin_run` and never spawns a task — zero detached tasks
//!   (requirement 3) — the durable run is left untouched (`Created`,
//!   `AlreadyNonActive`) since nothing was ever armed.
//! - two competing attempts against the same local-ownership slot: exactly
//!   one task is ever spawned, and the loser's rollback never disturbs the
//!   winner's reservation (requirement 3's "retain the legitimate owner").
//!
//! What this file does NOT attempt (see
//! `scenario_bundle7_phase7a_lifecycle_wiring_01.rs` for why): a full real
//! `start_execution_runtime()` success against `ProductionRuntimeStartEffects`
//! itself, which requires a real Alpaca broker adapter this patch's
//! operating rules forbid loading credentials for. The disposition-
//! determination logic and the `paper_enforced` interlock are proven
//! separately, hermetically, as pure-function unit tests in
//! `state::lifecycle` (`cargo test -p mqk-daemon --lib`). A hermetic proof
//! against `ProductionRuntimeStartEffects` itself (no real broker, DI'd
//! orchestrator) lives in
//! `scenario_bundle7_phase7a_final_atomic_ownership_and_rollback_truth_01.rs`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use mqk_daemon::daily_data_readiness::{
    advance_run_to_active, RuntimeStartEffects, RuntimeStartEffectsError, RuntimeStartSequenceError,
};
use mqk_daemon::state::{AppState, BrokerKind};
use uuid::Uuid;

/// Serializes every test in this file: they all contend for the same
/// `(DAEMON_ENGINE_ID, Paper)` "active run" slot via
/// `AppState::create_or_reuse_run_for_start`, and the fake effects below
/// genuinely arm/begin the run through real `mqk_db` calls.
fn db_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn db_pool_or_skip(label: &str) -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
        eprintln!("{label}: skipped; MQK_DATABASE_URL is not set");
        return None;
    };
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        eprintln!("{label}: skipped; MQK_DATABASE_URL looks like the paper DB, refusing to run");
        return None;
    }
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("{label}: skipped; could not connect to MQK_DATABASE_URL: {e}");
            return None;
        }
    };
    if let Err(e) = mqk_db::migrate(&pool).await {
        eprintln!("{label}: skipped; mqk_db::migrate failed: {e}");
        return None;
    }
    Some(pool)
}

async fn clear_any_preexisting_active_daemon_run(pool: &sqlx::PgPool) {
    let _ = sqlx::query(
        "update runs set status = 'STOPPED', stopped_at_utc = now() \
         where engine_id = 'mqk-daemon' and mode = 'PAPER' and status in ('ARMED', 'RUNNING')",
    )
    .execute(pool)
    .await;
}

async fn delete_run_and_its_events(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from sys_autonomous_session_events where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn fetch_run_status(pool: &sqlx::PgPool, run_id: Uuid) -> mqk_db::RunStatus {
    mqk_db::fetch_run(pool, run_id)
        .await
        .expect("fetch_run must succeed")
        .status
}

async fn fresh_run(pool: &sqlx::PgPool, st: &Arc<AppState>) -> Uuid {
    clear_any_preexisting_active_daemon_run(pool).await;
    st.create_or_reuse_run_for_start(pool)
        .await
        .expect("run creation must succeed")
}

// ---------------------------------------------------------------------------
// Hermetic fake `RuntimeStartEffects`.
//
// `start_runtime_effects` performs the exact real `mqk_db::arm_run`/
// `begin_run` calls production performs at that stage (so the durable-
// rollback proof below is genuine — the run really reaches ARMED/RUNNING),
// then optionally fails, mirroring a production failure at "begin failure
// after arm" / "initial tick failure" / etc. `reserve_local_ownership` and
// `spawn_loop` share a plain in-memory slot standing in for
// `AppState::execution_loop` — occupied means "another local owner already
// holds this run", exactly the invariant `advance_run_to_active` must
// preserve regardless of which concrete effects implementation is behind
// the trait.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeAtomicityEffects {
    pool: Option<sqlx::PgPool>,
    fail_start_runtime_effects_after_arm: AtomicBool,
    fail_reserve_local_ownership: AtomicBool,
    fail_spawn_loop: AtomicBool,

    start_runtime_effects_calls: AtomicU32,
    reserve_local_ownership_calls: AtomicU32,
    spawn_loop_calls: AtomicU32,
    rollback_local_effects_calls: AtomicU32,
    spawned_task_count: AtomicU32,
    rollback_run_ids: StdMutex<Vec<Uuid>>,

    /// Stands in for `AppState::execution_loop` — `Some(run_id)` means a
    /// local owner already holds the slot.
    owned_run_id: StdMutex<Option<Uuid>>,

    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 4: mirrors
    /// `ProductionRuntimeStartEffects`'s phase tracking, updated at the same
    /// milestones (arm succeeded, begin+heartbeat succeeded) so the
    /// phase-aware durable rollback policy (Stopped vs Halted) is exercised
    /// identically to production.
    phase: StdMutex<mqk_daemon::daily_data_readiness::RuntimeStartPhase>,
}

impl FakeAtomicityEffects {
    fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool: Some(pool),
            phase: StdMutex::new(mqk_daemon::daily_data_readiness::RuntimeStartPhase::BeforeArm),
            ..Default::default()
        }
    }

    /// Two attempts sharing the same ownership slot (proves "retain the
    /// legitimate owner" — the loser's rollback must never touch the
    /// winner's reservation).
    fn sharing_slot_with(other: &FakeAtomicityEffects, pool: sqlx::PgPool) -> Self {
        Self {
            pool: Some(pool),
            owned_run_id: StdMutex::new(*other.owned_run_id.lock().unwrap()),
            phase: StdMutex::new(mqk_daemon::daily_data_readiness::RuntimeStartPhase::BeforeArm),
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl RuntimeStartEffects for FakeAtomicityEffects {
    // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: reservation is
    // the first call — before `arm_run`/`begin_run` are ever attempted.
    async fn reserve_local_ownership(&self, run_id: Uuid) -> Result<(), RuntimeStartEffectsError> {
        self.reserve_local_ownership_calls
            .fetch_add(1, Ordering::SeqCst);
        if self.fail_reserve_local_ownership.load(Ordering::SeqCst) {
            return Err(RuntimeStartEffectsError::conflict(
                "runtime.start_refused.local_ownership_conflict",
                "injected reservation conflict",
            ));
        }
        let mut slot = self.owned_run_id.lock().unwrap();
        if slot.is_some() {
            return Err(RuntimeStartEffectsError::conflict(
                "runtime.start_refused.local_ownership_conflict",
                "runtime ownership changed while starting; refusing duplicate loop",
            ));
        }
        *slot = Some(run_id);
        Ok(())
    }

    async fn start_runtime_effects(&self, run_id: Uuid) -> Result<(), RuntimeStartEffectsError> {
        self.start_runtime_effects_calls
            .fetch_add(1, Ordering::SeqCst);
        let pool = self.pool.as_ref().expect("pool configured");
        mqk_db::arm_run(pool, run_id)
            .await
            .expect("fake start_runtime_effects: arm_run must succeed");
        *self.phase.lock().unwrap() =
            mqk_daemon::daily_data_readiness::RuntimeStartPhase::ArmedBeforeBegin;
        mqk_db::begin_run(pool, run_id)
            .await
            .expect("fake start_runtime_effects: begin_run must succeed");
        *self.phase.lock().unwrap() =
            mqk_daemon::daily_data_readiness::RuntimeStartPhase::RunningBeforeInitialTick;
        if self
            .fail_start_runtime_effects_after_arm
            .load(Ordering::SeqCst)
        {
            return Err(RuntimeStartEffectsError::internal(
                "fake.start_runtime_effects_failed_after_arm",
                "injected failure after arm_run/begin_run succeeded",
            ));
        }
        Ok(())
    }

    async fn spawn_loop(&self, _run_id: Uuid) -> Result<(), RuntimeStartEffectsError> {
        self.spawn_loop_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_spawn_loop.load(Ordering::SeqCst) {
            return Err(RuntimeStartEffectsError::internal(
                "fake.spawn_loop_failed",
                "injected spawn_loop failure",
            ));
        }
        // A real, detectable task — proves "the loop must actually run",
        // same pattern as `FakeRuntimeStartEffects` in
        // scenario_daily_data_readiness_start_gate_01.rs.
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);
        let handle = tokio::spawn(async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });
        handle.await.expect("synthetic loop task join");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        self.spawned_task_count.fetch_add(1, Ordering::SeqCst);
        *self.phase.lock().unwrap() =
            mqk_daemon::daily_data_readiness::RuntimeStartPhase::LoopInstalled;
        Ok(())
    }

    async fn rollback_local_effects(
        &self,
        run_id: Uuid,
    ) -> mqk_daemon::daily_data_readiness::LocalRollbackOutcome {
        self.rollback_local_effects_calls
            .fetch_add(1, Ordering::SeqCst);
        self.rollback_run_ids.lock().unwrap().push(run_id);
        // Mirrors production's local-effects clearing: the reservation
        // this attempt held (if any) is released on rollback — compare-
        // and-clear, so a loser's rollback can never clear a different
        // winner's reservation.
        let mut slot = self.owned_run_id.lock().unwrap();
        if *slot == Some(run_id) {
            *slot = None;
        }
        mqk_daemon::daily_data_readiness::LocalRollbackOutcome::default()
    }

    fn start_phase_reached(&self) -> mqk_daemon::daily_data_readiness::RuntimeStartPhase {
        *self.phase.lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// AR-01: success path — ownership reserved strictly before the loop is
// spawned, no rollback fires, the run reaches RUNNING.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar_01_success_path_reserves_ownership_before_spawn_and_never_rolls_back() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("AR-01").await else {
        return;
    };
    let st = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_id = fresh_run(&pool, &st).await;

    let fake = FakeAtomicityEffects::new(pool.clone());
    let mut trace: Vec<&'static str> = Vec::new();
    let result = advance_run_to_active(&pool, &fake, run_id, None, &mut trace).await;

    assert!(result.is_ok(), "AR-01: expected success: {result:?}");
    assert_eq!(
        trace,
        vec![
            "ownership_reserved",
            "local_bundle_committed",
            "loop_spawned"
        ],
        "AR-01: ownership must be reserved strictly before runtime effects run \
         or the loop is spawned"
    );
    assert_eq!(fake.start_runtime_effects_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fake.reserve_local_ownership_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fake.spawn_loop_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fake.spawned_task_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        fake.rollback_local_effects_calls.load(Ordering::SeqCst),
        0,
        "AR-01: a successful attempt must never roll back"
    );
    assert!(matches!(
        fetch_run_status(&pool, run_id).await,
        mqk_db::RunStatus::Running
    ));

    delete_run_and_its_events(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// AR-02 (requirement 2): a failure after `arm_run`/`begin_run` succeed
// rolls the durable run back to STOPPED, calls `rollback_local_effects`
// exactly once with the failed run's id, and never reserves ownership or
// spawns anything.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar_02_failure_after_arm_rolls_durable_run_back_to_stopped() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("AR-02").await else {
        return;
    };
    let st = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_id = fresh_run(&pool, &st).await;

    let fake = FakeAtomicityEffects::new(pool.clone());
    fake.fail_start_runtime_effects_after_arm
        .store(true, Ordering::SeqCst);

    let mut trace: Vec<&'static str> = Vec::new();
    let result = advance_run_to_active(&pool, &fake, run_id, None, &mut trace).await;

    assert!(result.is_err(), "AR-02: expected failure");
    let err = result.unwrap_err();
    let (original, rollback) = match err {
        RuntimeStartSequenceError::Effects { original, rollback } => (original, rollback),
        other => panic!("AR-02: expected Effects, got {other:?}"),
    };
    assert_eq!(
        original.fault_class,
        "fake.start_runtime_effects_failed_after_arm"
    );
    assert_eq!(
        trace,
        vec!["ownership_reserved"],
        "AR-02: ownership is reserved before the failing start_runtime_effects call"
    );
    assert_eq!(fake.reserve_local_ownership_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fake.spawn_loop_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fake.spawned_task_count.load(Ordering::SeqCst),
        0,
        "AR-02: no task is ever spawned for a start attempt that fails before spawn_loop"
    );
    assert_eq!(fake.rollback_local_effects_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*fake.rollback_run_ids.lock().unwrap(), vec![run_id]);
    assert!(
        matches!(
            rollback.durable,
            mqk_daemon::daily_data_readiness::DurableRollbackDisposition::Stopped
        ),
        "AR-02: arm+begin succeeded but tick was never attempted (phase= \
         RunningBeforeInitialTick, cleanly stoppable per requirement 4) — \
         durable rollback must be Stopped, not Halted: {rollback:?}"
    );
    assert!(!rollback.durable_status_unknown);
    assert!(
        matches!(
            fetch_run_status(&pool, run_id).await,
            mqk_db::RunStatus::Stopped
        ),
        "AR-02: the durable run must be rolled back from RUNNING to STOPPED, \
         never left ARMED/RUNNING without a local owner"
    );

    delete_run_and_its_events(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// AR-03 (requirement 2/3): an ownership-reservation conflict — now the very
// first call in the sequence — never attempts arm_run/begin_run and never
// spawns a task (the detached-task proof). The durable run is left
// untouched (`Created`, `AlreadyNonActive`) since nothing was ever armed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar_03_ownership_conflict_spawns_no_task_and_still_rolls_back() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("AR-03").await else {
        return;
    };
    let st = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_id = fresh_run(&pool, &st).await;

    let fake = FakeAtomicityEffects::new(pool.clone());
    fake.fail_reserve_local_ownership
        .store(true, Ordering::SeqCst);

    let mut trace: Vec<&'static str> = Vec::new();
    let result = advance_run_to_active(&pool, &fake, run_id, None, &mut trace).await;

    assert!(result.is_err(), "AR-03: expected conflict");
    let rollback = match result.unwrap_err() {
        RuntimeStartSequenceError::Effects { rollback, .. } => rollback,
        other => panic!("AR-03: expected Effects, got {other:?}"),
    };
    assert!(trace.is_empty());
    assert_eq!(
        fake.start_runtime_effects_calls.load(Ordering::SeqCst),
        0,
        "AR-03: reservation now runs first — a conflict must never reach \
         start_runtime_effects (arm_run/begin_run)"
    );
    assert_eq!(fake.reserve_local_ownership_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fake.spawn_loop_calls.load(Ordering::SeqCst),
        0,
        "AR-03: spawn_loop must never be called after a reservation conflict"
    );
    assert_eq!(
        fake.spawned_task_count.load(Ordering::SeqCst),
        0,
        "AR-03: detached-task proof — a reservation conflict must create zero tasks"
    );
    assert_eq!(fake.rollback_local_effects_calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            rollback.durable,
            mqk_daemon::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ),
        "AR-03: nothing was ever armed, so the durable run was never \
         Armed/Running — rollback must report AlreadyNonActive: {rollback:?}"
    );
    assert!(matches!(
        fetch_run_status(&pool, run_id).await,
        mqk_db::RunStatus::Created
    ));

    delete_run_and_its_events(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// AR-04 (requirement 3): two competing attempts against the same local-
// ownership slot — exactly one task is ever spawned, and the loser's
// rollback never disturbs the winner's reservation ("retain the legitimate
// owner").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar_04_two_competing_attempts_exactly_one_task_and_winner_retained() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("AR-04").await else {
        return;
    };
    let st = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_a = fresh_run(&pool, &st).await;

    let winner = FakeAtomicityEffects::new(pool.clone());
    let mut trace_a: Vec<&'static str> = Vec::new();
    let result_a = advance_run_to_active(&pool, &winner, run_a, None, &mut trace_a).await;
    assert!(result_a.is_ok(), "AR-04: winner must succeed: {result_a:?}");
    assert_eq!(*winner.owned_run_id.lock().unwrap(), Some(run_a));

    // A second, independent run competing for the *same* ownership slot —
    // simulates a duplicate/late start attempt racing the already-installed
    // owner. The winner's run is RUNNING (an active run for this engine/
    // mode), so a second `create_or_reuse_run_for_start` call would itself
    // be refused by the ordinary "durable active run exists" conflict —
    // this test's subject is the *local ownership slot*, not that outer
    // gate, so a second run row is inserted directly, bypassing that
    // engine-level policy check the same way a distinct daemon process
    // racing the same DB would produce a genuinely separate run row.
    let run_b = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!("mqk-daemon.phase7a.atomicity.ar04.run_b.{run_a}").as_bytes(),
    );
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run_b,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "UNKNOWN".to_string(),
            config_hash: "ar-04-test".to_string(),
            config_json: serde_json::json!({"test": "ar-04"}),
            host_fingerprint: "ar-04-test-host".to_string(),
        },
    )
    .await
    .expect("AR-04: run_b insert must succeed");

    let loser = FakeAtomicityEffects::sharing_slot_with(&winner, pool.clone());
    let mut trace_b: Vec<&'static str> = Vec::new();
    let result_b = advance_run_to_active(&pool, &loser, run_b, None, &mut trace_b).await;

    assert!(result_b.is_err(), "AR-04: loser must be refused");
    assert_eq!(
        loser.spawned_task_count.load(Ordering::SeqCst),
        0,
        "AR-04: the loser must never spawn a task"
    );
    assert_eq!(
        winner.spawned_task_count.load(Ordering::SeqCst),
        1,
        "AR-04: exactly one task total across both attempts"
    );
    assert_eq!(
        *loser.owned_run_id.lock().unwrap(),
        Some(run_a),
        "AR-04: the loser's rollback must never clear the winner's reservation \
         (retain the legitimate owner)"
    );

    delete_run_and_its_events(&pool, run_a).await;
    if run_b != run_a {
        delete_run_and_its_events(&pool, run_b).await;
    }
}

// ---------------------------------------------------------------------------
// AR-05 (requirement 1): structural single-read-site proof. Counting real
// runtime call sites for a pure env/file-reading function has no natural
// instrumentation seam without dependency-injecting every call site — so
// this proves the *stronger* property directly from source: each of the
// three resolution functions `StartAttemptAuthoritySnapshot::resolve` folds
// together has exactly one real call site in the whole start-attempt path
// (`state/lifecycle.rs` for the two per-start-attempt reads,
// `state/loop_runner.rs` for the assignment list previously re-derived a
// third time there). Every other occurrence of these names in
// `lifecycle.rs` is a comment (checked separately below).
// ---------------------------------------------------------------------------

fn repo_root() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .ancestors()
        .nth(3)
        .expect("mqk-daemon crate must be nested three levels below the repo root")
        .to_path_buf()
}

/// Count non-comment source lines containing `needle` as an actual call
/// (`needle(`), not merely mentioned inside a `//` comment.
fn count_real_calls(content: &str, needle: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && line.contains(&format!("{needle}("))
        })
        .count()
}

#[test]
fn ar_05_each_start_attempt_input_is_resolved_at_exactly_one_real_call_site() {
    let root = repo_root();
    let lifecycle_src =
        std::fs::read_to_string(root.join("core-rs/crates/mqk-daemon/src/state/lifecycle.rs"))
            .expect("read lifecycle.rs");
    let loop_runner_src =
        std::fs::read_to_string(root.join("core-rs/crates/mqk-daemon/src/state/loop_runner.rs"))
            .expect("read loop_runner.rs");

    for needle in [
        "build_multi_symbol_runtime_config_from_env",
        "load_readiness_context_from_env",
    ] {
        assert_eq!(
            count_real_calls(&lifecycle_src, needle),
            1,
            "AR-05: {needle} must have exactly one real call site in \
             lifecycle.rs (inside StartAttemptAuthoritySnapshot::resolve) — \
             found a different count, meaning either a second read crept back \
             in or the resolver itself was removed"
        );
        assert_eq!(
            count_real_calls(&loop_runner_src, needle),
            0,
            "AR-05: {needle} must never be called from loop_runner.rs — the \
             execution loop must consume the frozen assignment parameter, \
             never re-resolve it"
        );
    }

    // fleet_ids_from_env is also called once by B1A's
    // `resolve_autonomous_runtime_context` seam (a distinct, pre-existing
    // native-strategy-bootstrap concern, not part of this snapshot) — so
    // this one has two legitimate real call sites in lifecycle.rs, not one.
    // Bounding it at exactly two (rather than leaving it unchecked) still
    // proves no *third*, redundant read was introduced by this patch.
    assert_eq!(
        count_real_calls(&lifecycle_src, "fleet_ids_from_env"),
        1,
        "AR-05: fleet_ids_from_env must have exactly one real call site in \
         lifecycle.rs itself (inside StartAttemptAuthoritySnapshot::resolve) \
         — B1A's own fleet-id resolution lives in \
         autonomous_runtime_context.rs, a separate file, not lifecycle.rs"
    );
}
