//! PRE-SOAK-RUNTIME-HALT-FENCE-CAS-01 — `halt_and_disarm_run_if_active` matrix.
//!
//! # Defect closed
//!
//! `PAPER-SOAK-STALE-CLAIM-RECOVERY-03` (3197f4e4) closed the pre-broker-
//! submit races: fenced lease acquire/refresh
//! (`runtime_lease::acquire_or_refresh_lease_for_running_run`) and fenced
//! outbox claim (`outbox_claim_batch_for_run_with_lease_authority`), both of
//! which decide authority under the same `runs` row lock
//! `clear_halted_run_and_reset_stale_claims` uses. It left one narrower
//! TOCTOU open: when either of those functions returns `Lost` / `HeldByOther`
//! / `LeaseInvalid`, that decision was reached in a transaction that had
//! *already released* the run row lock before returning. The runtime then
//! persisted its safety halt as a **separate**, later DB operation — the old
//! unconditional `halt_run` (`UPDATE runs SET status='HALTED' WHERE
//! run_id=$1`, no status predicate) followed by a separate `persist_arm_state`
//! call. Between "authority lost" and "halt persisted", an operator's
//! `clear_halted_run_and_reset_stale_claims` could win the row lock, observe
//! `HALTED`, and commit `STOPPED` — and the stale runtime's later write would
//! then silently overwrite `STOPPED` back to `HALTED`.
//!
//! `halt_and_disarm_run_if_active` closes this by making the halt decision
//! and the halt mutation share one atomic, serialized authority boundary:
//! `runs.status` and `sys_arm_state` are both re-checked/written under the
//! same `SELECT ... FOR UPDATE` row lock every other recovery-boundary
//! function in this module uses.
//!
//! # Coverage
//!
//! | Test  | Claim                                                              |
//! |-------|---------------------------------------------------------------------|
//! | RHF-01/06 | A genuine live safety fault on a RUNNING run halts + disarms atomically, exactly as before |
//! | RHF-07 | A repeated safety halt against an already-HALTED run is idempotent/fail-closed; never revives execution |
//! | RHF-08 | A safety halt against a STOPPED run is zero mutation |
//! | RHF-09 | A safety halt against a CREATED run is zero mutation |
//! | RHF-02/03/04/05 | A stale safety halt reaching this seam *after* a successful `clear_halted_run_and_reset_stale_claims` has already committed STOPPED is fenced with zero mutation — the DB-level shape shared by every caller (`Lost`, `HeldByOther`, claim-time `LeaseInvalid`) regardless of which one originally lost authority |
//! | §13   | A stale safety halt reaching this seam after a successful clear *and* a legitimate operator re-arm (STOPPED -> ARMED, both `runs.status` and the global `sys_arm_state`) must not overwrite either the re-armed run status or the re-armed `sys_arm_state` |
//!
//! RHF-10 (zero broker calls across every authority-lost/fenced scenario) and
//! the genuine concurrent real-`ExecutionOrchestrator::tick()` proof
//! (RHF-NEG-1) live in `mqk-runtime::orchestrator::tests` — this file only
//! proves the DB primitive itself, which every runtime caller shares.
//!
//! # Proof boundary
//!
//! `sys_arm_state` is a global singleton row (not scoped by `run_id`), so
//! every test here uses `mqk_db::run_isolated` (a genuinely disposable
//! per-test database) rather than the shared `MQK_DATABASE_URL` database —
//! two tests asserting on `sys_arm_state` concurrently against shared state
//! would be flaky by construction (FULL-AUDIT-FAIL-017).

use chrono::{TimeZone, Utc};
use uuid::Uuid;

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("rhf01-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: ts(0),
            git_hash: "RHF01-TEST".to_string(),
            config_hash: format!("cfg-{run_id}"),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert_run");
}

fn ts(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().expect("valid ts")
}

async fn make_run_running(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4(); // allow: test-only — isolated disposable-DB fixture, never called from production paths
    seed_run(pool, run_id).await;
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    run_id
}

// ---------------------------------------------------------------------------
// RHF-01 / RHF-06 — genuine live safety fault on RUNNING halts + disarms
// atomically.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf01_06_halt_on_running_run_commits_halted_and_disarmed_atomically() {
    mqk_db::run_isolated("rhf01_06", |pool| async move {
        let run_id = make_run_running(&pool).await;

        let outcome =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(1_000), "ReconcileDrift")
                .await
                .expect("halt_and_disarm_run_if_active must not error on a RUNNING run");
        assert_eq!(outcome, mqk_db::SafetyHaltOutcome::Halted);

        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));
        assert_eq!(run.halted_at_utc, Some(ts(1_000)));

        let arm = mqk_db::load_arm_state(&pool)
            .await
            .expect("load_arm_state")
            .expect("arm state row must exist after a safety halt");
        assert_eq!(arm.0, "DISARMED");
        assert_eq!(arm.1.as_deref(), Some("ReconcileDrift"));
    })
    .await;
}

// ---------------------------------------------------------------------------
// RHF-07 — repeated halt against an already-HALTED run is idempotent.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf07_halt_on_already_halted_run_is_idempotent_and_fail_closed() {
    mqk_db::run_isolated("rhf07", |pool| async move {
        let run_id = make_run_running(&pool).await;
        let first =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(2_000), "IntegrityViolation")
                .await
                .expect("first halt");
        assert_eq!(first, mqk_db::SafetyHaltOutcome::Halted);

        let second =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(2_100), "LeaderLeaseLost")
                .await
                .expect("second halt against an already-HALTED run must not error");
        assert_eq!(second, mqk_db::SafetyHaltOutcome::AlreadyHalted);

        // Never revives execution: status/halted_at_utc from the FIRST halt
        // are untouched by the second, idempotent call.
        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));
        assert_eq!(
            run.halted_at_utc,
            Some(ts(2_000)),
            "RHF-07: halted_at_utc must remain the FIRST halt's timestamp, not be reset by \
             an idempotent repeat"
        );

        // sys_arm_state DISARMED reason IS refreshed (fail-closed: the most
        // recent safety concern is the one an operator should see).
        let arm = mqk_db::load_arm_state(&pool)
            .await
            .expect("load_arm_state")
            .expect("arm state row exists");
        assert_eq!(arm.0, "DISARMED");
        assert_eq!(arm.1.as_deref(), Some("LeaderLeaseLost"));
    })
    .await;
}

// ---------------------------------------------------------------------------
// RHF-08 / RHF-09 — halt against STOPPED/CREATED is zero mutation.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf08_halt_on_stopped_run_is_zero_mutation() {
    mqk_db::run_isolated("rhf08", |pool| async move {
        let run_id = make_run_running(&pool).await;
        mqk_db::halt_run(&pool, run_id, ts(3_000))
            .await
            .expect("halt_run");
        let clear = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, ts(3_100))
            .await
            .expect("clear must succeed");
        assert!(matches!(
            clear,
            mqk_db::ClearHaltedRunOutcome::Success { .. }
        ));

        let outcome =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(3_200), "LeaderLeaseLost")
                .await
                .expect("halt against STOPPED must not error");
        assert_eq!(
            outcome,
            mqk_db::SafetyHaltOutcome::RunNotActive {
                actual_status: "STOPPED".to_string()
            }
        );

        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run.status, mqk_db::RunStatus::Stopped),
            "RHF-08: zero mutation — status must remain STOPPED"
        );
        assert!(
            mqk_db::load_arm_state(&pool)
                .await
                .expect("load_arm_state")
                .is_none(),
            "RHF-08: zero mutation — sys_arm_state must never have been written"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf09_halt_on_created_run_is_zero_mutation() {
    mqk_db::run_isolated("rhf09", |pool| async move {
        let run_id = Uuid::new_v4(); // allow: test-only — isolated disposable-DB fixture, never called from production paths
        seed_run(&pool, run_id).await;

        let outcome =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(4_000), "CancelTargetMissing")
                .await
                .expect("halt against CREATED must not error");
        assert_eq!(
            outcome,
            mqk_db::SafetyHaltOutcome::RunNotActive {
                actual_status: "CREATED".to_string()
            }
        );

        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run.status, mqk_db::RunStatus::Created),
            "RHF-09: zero mutation — status must remain CREATED"
        );
        assert!(
            mqk_db::load_arm_state(&pool)
                .await
                .expect("load_arm_state")
                .is_none(),
            "RHF-09: zero mutation — sys_arm_state must never have been written"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// RHF-02/03/04/05 — a stale safety halt reaching this seam AFTER a
// successful clear has already committed STOPPED is fenced with zero
// mutation. The DB-level shape is identical regardless of which authority
// path (Lost, HeldByOther, claim-time LeaseInvalid) the caller originally
// lost — see `mqk-runtime::orchestrator::tests::rhf_neg1_*` for the real
// `ExecutionOrchestrator::tick()` proof of the `Lost` path specifically.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf02_03_04_05_stale_safety_halt_after_successful_clear_is_fenced_and_zero_mutation() {
    mqk_db::run_isolated("rhf02_05", |pool| async move {
        let run_id = make_run_running(&pool).await;

        // Models the operator's real recovery path winning first: a
        // concurrent runtime's Lost/HeldByOther/LeaseInvalid decision (made
        // while the run was genuinely RUNNING) has already returned by this
        // point, but that runtime has not yet called the safety-halt seam.
        mqk_db::halt_run(&pool, run_id, ts(5_000))
            .await
            .expect("operator halt_run");
        let clear = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, ts(5_100))
            .await
            .expect("operator clear must succeed");
        assert!(matches!(
            clear,
            mqk_db::ClearHaltedRunOutcome::Success { .. }
        ));
        let run_after_clear = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after_clear.status, mqk_db::RunStatus::Stopped));

        // The stale runtime's deferred safety-halt call finally executes.
        for reason in [
            "LeaderLeaseLost",
            "LeaderLeaseUnavailable",
            "ReconcileDrift",
        ] {
            let outcome = mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(5_200), reason)
                .await
                .unwrap_or_else(|e| panic!("stale halt (reason={reason}) must not error: {e}"));
            assert_eq!(
                outcome,
                mqk_db::SafetyHaltOutcome::RunNotActive {
                    actual_status: "STOPPED".to_string()
                },
                "reason={reason}"
            );
        }

        let run_final = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run_final.status, mqk_db::RunStatus::Stopped),
            "RHF-02/03/04/05: a stale safety halt must never turn a recovered STOPPED run \
             back into HALTED, got status={:?}",
            run_final.status
        );
        assert!(
            mqk_db::load_arm_state(&pool)
                .await
                .expect("load_arm_state")
                .is_none(),
            "RHF-02/03/04/05: zero mutation — sys_arm_state must never have been written by \
             the stale, superseded safety halt"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// §13 — a stale safety halt reaching this seam after a successful clear AND
// a legitimate operator re-arm must not overwrite the re-armed state,
// either at the `runs.status` layer or the global `sys_arm_state` layer.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn rhf13_stale_safety_halt_does_not_overwrite_post_clear_re_arm() {
    mqk_db::run_isolated("rhf13", |pool| async move {
        let run_id = make_run_running(&pool).await;
        mqk_db::halt_run(&pool, run_id, ts(6_000))
            .await
            .expect("operator halt_run");
        let clear = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, ts(6_100))
            .await
            .expect("operator clear must succeed");
        assert!(matches!(
            clear,
            mqk_db::ClearHaltedRunOutcome::Success { .. }
        ));

        // Legitimate operator re-arm: the real production sequence is
        // `arm_run` (runs.status STOPPED -> ARMED) plus the operator's own
        // `/ops/action arm` route (sys_arm_state -> ARMED via
        // `persist_arm_state_canonical`) — both real production calls.
        mqk_db::arm_run(&pool, run_id).await.expect("re-arm");
        mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
            .await
            .expect("operator re-arm sys_arm_state");

        let run_rearmed = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_rearmed.status, mqk_db::RunStatus::Armed));

        // The stale runtime's deferred safety-halt call finally executes,
        // believing it still holds authority over a run it last observed
        // RUNNING.
        let outcome =
            mqk_db::halt_and_disarm_run_if_active(&pool, run_id, ts(6_200), "LeaderLeaseLost")
                .await
                .expect("stale halt against a re-armed run must not error");
        assert_eq!(
            outcome,
            mqk_db::SafetyHaltOutcome::RunNotActive {
                actual_status: "ARMED".to_string()
            }
        );

        let run_final = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run_final.status, mqk_db::RunStatus::Armed),
            "§13: a stale safety halt must never turn a freshly re-armed run back into \
             HALTED, got status={:?}",
            run_final.status
        );

        let arm = mqk_db::load_arm_state(&pool)
            .await
            .expect("load_arm_state")
            .expect("arm state row exists from the operator re-arm");
        assert_eq!(
            arm.0, "ARMED",
            "§13: a stale safety halt must never overwrite a freshly re-armed sys_arm_state \
             back to DISARMED"
        );
        assert_eq!(arm.1, None, "ARMED state carries no disarm reason");
    })
    .await;
}
