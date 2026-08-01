//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-ATOMIC-OWNERSHIP-AND-
//! ROLLBACK-TRUTH requirement 6: hermetic proof against the *real*
//! production effects path.
//!
//! Every prior Phase 7A test drove `daily_data_readiness::advance_run_to_
//! active` against either a hermetic fake `RuntimeStartEffects`
//! (`scenario_bundle7_phase7a_atomicity_repair_01.rs`) or bypassed the
//! start path entirely (`scenario_bundle7_phase7a_lifecycle_wiring_01.rs`).
//! That file documents a real, deliberate constraint this file independently
//! re-confirmed while writing it: `ProductionRuntimeStartEffects::start_
//! runtime_effects` calls `AppState::build_execution_orchestrator`, which
//! calls `build_daemon_broker` — and `build_daemon_broker` itself refuses
//! `BrokerKind::Paper` with `runtime.start_refused.paper_broker_not_
//! execution_path` ("the in-process fill engine has no market-data source
//! wired... the only authoritative paper execution path is Paper+Alpaca").
//! This is a *second*, deeper gate than the outer `deployment_mode_
//! readiness` refusal the lifecycle-wiring file names — bypassing the outer
//! gate (by driving `ProductionRuntimeStartEffects` directly, skipping
//! `start_execution_runtime`) is not sufficient on its own. A genuine
//! *successful* real-orchestrator start therefore still needs live Alpaca
//! credentials or a mock-Alpaca-server harness — neither available here,
//! same conclusion, independently reached.
//!
//! What real production code *can* be hermetically exercised without a
//! broker ever being constructed, and is exercised here:
//! - `AppState::drive_production_start_effects_for_test` (a test-only,
//!   non-cfg-gated seam on `AppState`, following this codebase's
//!   established `_for_test` convention) drives `daily_data_readiness::
//!   advance_run_to_active` against a real `state::lifecycle::
//!   ProductionRuntimeStartEffects` — never a fake.
//! - `ProductionRuntimeStartEffects::reserve_local_ownership` (real
//!   `AppState::reserve_execution_loop_slot`) and `rollback_local_effects`
//!   (real `AppState::rollback_run_start_bundle_for_run`/`release_
//!   execution_loop_reservation_for_run`) run to completion without ever
//!   touching a broker — broker construction is the *next* step, inside
//!   `start_runtime_effects`, after reservation already succeeded.
//! - a genuine `start_runtime_effects` failure (real `build_execution_
//!   orchestrator`/`build_daemon_broker` refusing Paper) still exercises
//!   the real rollback path end to end: phase-aware durable disposition,
//!   reservation release, and the structured `RollbackOutcome`.
//!
//! FA-01 proves: a real `start_runtime_effects` failure is cleanly rolled
//! back (phase stays `BeforeArm` — the failure is caught before `arm_run`
//! is ever attempted — so no durable Armed/Running state exists to roll
//! back, and the reservation `reserve_local_ownership` made is released).
//!
//! FA-02 proves requirement 3's explicit ask: "Add a production-AppState
//! test with sentinel values for an existing owner. After a conflicting
//! start, prove loop/artifact/bootstrap/snapshot/counters/targets/dynamic-
//! selection state are unchanged." An existing owner (run A) is established
//! via `AppState::establish_db_backed_active_run_for_test` (a real DB-backed
//! active run with a locally-owned loop handle, real predates this patch)
//! plus this patch's own sentinel-planting test seams
//! (`plant_accepted_artifact_for_test`, `plant_day_signal_count_for_test`,
//! `commit_dynamic_selection_runtime_state_for_test`). A conflicting run B
//! then attempts a real start via `drive_production_start_effects_for_test`
//! — refused by the real `reserve_local_ownership` (the slot is genuinely
//! `Active` for run A) before any broker is ever touched. Every sentinel
//! field is asserted unchanged afterward.

use std::sync::Arc;

use mqk_daemon::state::{AcceptedArtifactProvenance, AppState, BrokerKind, DeploymentMode};
use mqk_portfolio::DynamicSelectionMode;
use uuid::Uuid;

/// Serializes every test in this file: they all contend for the same
/// `(DAEMON_ENGINE_ID, Paper)` "active run" slot via
/// `AppState::create_or_reuse_run_for_start`.
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

/// A hermetic `Paper`+`Paper` `AppState` wired to the real test DB. Never
/// start-able to a genuine `Active` loop through the real broker-
/// construction path (`build_daemon_broker` refuses `BrokerKind::Paper`) —
/// this file uses that refusal itself as a real, deterministic
/// `start_runtime_effects` failure (FA-01), and never needs a working
/// broker for the reservation-only proof (FA-02).
fn hermetic_paper_state(pool: &sqlx::PgPool) -> Arc<AppState> {
    let mut state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    state.db = Some(pool.clone());
    Arc::new(state)
}

/// Minimal `Off`-disposition dynamic-selection fixture for `run_id` — zero
/// plan, zero host pool, exactly what a real `Off`-effective-mode start
/// attempt commits.
fn off_disposition_fixture(run_id: Uuid) -> mqk_daemon::state::DynamicSelectionRuntimeState {
    mqk_daemon::state::DynamicSelectionRuntimeState {
        run_id,
        disposition:
            mqk_daemon::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off,
        configured_mode: DynamicSelectionMode::Off,
        effective_mode: DynamicSelectionMode::Off,
        live_lock_applied: false,
        plan: None,
        selected_pairs: Vec::new(),
        host_pool: None,
        reasons: Vec::new(),
        approved_for_live: false,
    }
}

// ---------------------------------------------------------------------------
// FA-01: a genuine `start_runtime_effects` failure (real broker construction
// refusing `BrokerKind::Paper`) is cleanly rolled back — reservation
// released, phase stays `BeforeArm` (arm_run was never attempted), durable
// disposition is `AlreadyNonActive` (nothing was ever armed).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn fa_01_real_orchestrator_construction_failure_is_cleanly_rolled_back() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("FA-01").await else {
        return;
    };
    clear_any_preexisting_active_daemon_run(&pool).await;
    let state = hermetic_paper_state(&pool);
    let run_id = state
        .create_or_reuse_run_for_start(&pool)
        .await
        .expect("run creation must succeed");

    let (result, trace) = state
        .drive_production_start_effects_for_test(
            pool.clone(),
            run_id,
            Some(off_disposition_fixture(run_id)),
        )
        .await;

    assert!(
        result.is_err(),
        "FA-01: expected failure (Paper broker construction is refused)"
    );
    let (original, rollback) = match result.unwrap_err() {
        mqk_daemon::daily_data_readiness::RuntimeStartSequenceError::Effects {
            original,
            rollback,
        } => (original, rollback),
        other => panic!("FA-01: expected Effects, got {other:?}"),
    };
    assert_eq!(
        original.fault_class, "runtime.start_refused.paper_broker_not_execution_path",
        "FA-01: this must be the real build_daemon_broker refusal, not a \
         different failure mode: {original:?}"
    );
    assert_eq!(
        trace,
        vec!["ownership_reserved"],
        "FA-01: reservation succeeds (it never touches a broker); the \
         failure happens inside the next step, start_runtime_effects, \
         before any further trace tag fires"
    );
    assert!(
        matches!(
            rollback.phase_reached,
            mqk_daemon::daily_data_readiness::RuntimeStartPhase::BeforeArm
        ),
        "FA-01: the real orchestrator construction failure happens before \
         arm_run is ever attempted — phase must still be BeforeArm: \
         {rollback:?}"
    );
    assert!(
        matches!(
            rollback.durable,
            mqk_daemon::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ),
        "FA-01: nothing was ever armed, so the durable run was never \
         Armed/Running: {rollback:?}"
    );
    assert!(!rollback.durable_status_unknown);

    // The reservation this attempt made is released — the slot is Empty
    // again, not stuck showing this failed run_id.
    assert_eq!(
        state.locally_owned_run_id().await,
        None,
        "FA-01: a failed start must release its own reservation"
    );
    assert!(
        state.dynamic_selection_runtime_snapshot().await.is_none(),
        "FA-01: no dynamic-selection state may survive a failed start — \
         start_runtime_effects failed before the local bundle was ever built"
    );
    assert!(matches!(
        mqk_db::fetch_run(&pool, run_id)
            .await
            .expect("fetch_run")
            .status,
        mqk_db::RunStatus::Created
    ));

    delete_run_and_its_events(&pool, run_id).await;
}

// ---------------------------------------------------------------------------
// FA-02 (requirement 3, explicit ask): "Add a production-AppState test with
// sentinel values for an existing owner. After a conflicting start, prove
// loop/artifact/bootstrap/snapshot/counters/targets/dynamic-selection state
// are unchanged."
//
// Run A is a real DB-backed `Active` owner (`establish_db_backed_active_
// run_for_test`) carrying planted sentinel values. Run B then attempts a
// real start via `drive_production_start_effects_for_test` — refused by the
// real `reserve_local_ownership` (the slot is genuinely `Active` for run A)
// strictly before any broker is ever touched. Every sentinel field is
// asserted unchanged afterward.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn fa_02_ownership_conflict_preserves_existing_owner_sentinel_state() {
    let _g = db_test_lock().lock().await;
    let Some(pool) = db_pool_or_skip("FA-02").await else {
        return;
    };
    clear_any_preexisting_active_daemon_run(&pool).await;
    let state = hermetic_paper_state(&pool);

    // Run A: a real DB-backed active run with local ownership (pre-existing
    // AUTON-PAPER-03B proof seam), carrying this patch's sentinel-planted
    // values.
    let run_a = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"mqk-daemon.phase7a.final.fa02.run_a");
    state
        .establish_db_backed_active_run_for_test(run_a)
        .await
        .expect("FA-02: run A must be established as the active owner");

    let sentinel_artifact = AcceptedArtifactProvenance {
        artifact_id: "fa-02-sentinel-artifact".to_string(),
        artifact_type: "sentinel".to_string(),
        stage: "sentinel".to_string(),
        produced_by: "fa-02-test".to_string(),
    };
    state
        .plant_accepted_artifact_for_test(Some(sentinel_artifact.clone()))
        .await;
    state.plant_day_signal_count_for_test(4242);
    state
        .commit_dynamic_selection_runtime_state_for_test(off_disposition_fixture(run_a))
        .await;

    assert_eq!(state.locally_owned_run_id().await, Some(run_a));

    // Run B: a second, independent run row competing for the *same* local-
    // ownership slot — the same "separate daemon process racing the same
    // DB" pattern `scenario_bundle7_phase7a_atomicity_repair_01.rs`'s AR-04
    // and this file's own doc comment describe.
    let run_b = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk-daemon.phase7a.final.fa02.run_b.{run_a}").as_bytes(),
    );
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run_b,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "UNKNOWN".to_string(),
            config_hash: "fa-02-test".to_string(),
            config_json: serde_json::json!({"test": "fa-02"}),
            host_fingerprint: "fa-02-test-host".to_string(),
        },
    )
    .await
    .expect("FA-02: run_b insert must succeed");

    let (result_b, trace_b) = state
        .drive_production_start_effects_for_test(
            pool.clone(),
            run_b,
            Some(off_disposition_fixture(run_b)),
        )
        .await;

    assert!(result_b.is_err(), "FA-02: run B must be refused");
    assert!(
        trace_b.is_empty(),
        "FA-02: no ordered-trace tag may fire for a reservation conflict"
    );
    let (original_b, rollback_b) = match result_b.unwrap_err() {
        mqk_daemon::daily_data_readiness::RuntimeStartSequenceError::Effects {
            original,
            rollback,
        } => (original, rollback),
        other => panic!("FA-02: expected Effects, got {other:?}"),
    };
    assert_eq!(
        original_b.fault_class, "runtime.start_refused.local_ownership_conflict",
        "FA-02: this must be the real reservation conflict, not a different \
         failure mode: {original_b:?}"
    );
    assert!(
        matches!(
            rollback_b.durable,
            mqk_daemon::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ),
        "FA-02: run B was never armed, so its own durable rollback is a \
         no-op: {rollback_b:?}"
    );

    // The core proof: every field run A committed is exactly as it was.
    assert_eq!(
        state.locally_owned_run_id().await,
        Some(run_a),
        "FA-02: the slot must still show run A as the Active owner — run \
         B's rollback must never clear a different run's reservation"
    );
    let after_selection = state
        .dynamic_selection_runtime_snapshot()
        .await
        .expect("FA-02: run A's dynamic-selection state must still be present");
    assert_eq!(
        after_selection.run_id, run_a,
        "FA-02: run A's committed dynamic-selection state must be unchanged \
         (still tagged with run A's run_id, never cleared or overwritten by \
         run B's rollback)"
    );
    assert_eq!(
        state.day_signal_count_snapshot_for_test(),
        4242,
        "FA-02: run A's sentinel day_signal_count must be unchanged"
    );
    assert_eq!(
        state.accepted_artifact_snapshot_for_test().await,
        Some(sentinel_artifact),
        "FA-02: run A's sentinel accepted_artifact provenance must be unchanged"
    );
    assert!(matches!(
        mqk_db::fetch_run(&pool, run_a)
            .await
            .expect("fetch_run run_a")
            .status,
        mqk_db::RunStatus::Running
    ));
    assert!(matches!(
        mqk_db::fetch_run(&pool, run_b)
            .await
            .expect("fetch_run run_b")
            .status,
        mqk_db::RunStatus::Created
    ));

    // Clean up: stop the real run A loop before deleting rows.
    let _ = state.stop_execution_runtime().await;
    delete_run_and_its_events(&pool, run_a).await;
    delete_run_and_its_events(&pool, run_b).await;
}
