//! PAPER-SOAK-REPAIR-20260819-ORPHANED-RUN-RECOVERY-01: proof tests for
//! `mqk_db::stop_run_if_evidence_clean`.
//!
//! ## Why this exists
//!
//! A daemon process crash/reboot can leave a `runs` row durably `ARMED`/
//! `RUNNING` with no local runtime owner. Before this patch, production had
//! no authorized path to move that row to `STOPPED` without either raw SQL
//! (prohibited) or the unguarded `mqk_db::stop_run` primitive (no
//! independent unacked-outbox/reconcile proof of its own — its safety has
//! always come from its callers, and no caller existed for this exact
//! crash-orphan shape). `reconcile_durable_run_without_local_owner`
//! (mqk-daemon) already *detects* this shape correctly and fails closed to
//! `controller_degraded`/`manual_intervention_required` — but nothing could
//! ever release it, because nothing ever proved the run itself terminal.
//!
//! `stop_run_if_evidence_clean` closes that gap: it reuses the exact same
//! unacked-outbox (`outbox_list_unacked_for_run`) and global-reconcile
//! (`load_reconcile_status_state`) evidence already trusted for this
//! question, then applies `stop_run`'s existing CAS-guarded transition.
//!
//! All tests require `MQK_DATABASE_URL` and run against an isolated
//! disposable database via `mqk_db::run_isolated` (never the shared/Paper
//! database). Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_stop_run_if_evidence_clean_20260819 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! ## Test matrix
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | t01  | RUNNING + zero unacked outbox + clean reconcile -> Stopped, durable STOPPED |
//! | t02  | ARMED (never began) + zero unacked outbox + clean reconcile -> Stopped |
//! | t03  | already STOPPED -> NotActive, zero mutation |
//! | t04  | HALTED -> NotActive, zero mutation (never overwrites a sticky halt) |
//! | t05  | unacked outbox row (PENDING) -> UnacknowledgedOutbox, run stays RUNNING |
//! | t06  | dirty reconcile status (mismatched_positions > 0) -> ReconcileDirty, zero mutation |
//! | t07  | no reconcile status row at all -> ReconcileDirty (absence is not evidence of agreement) |
//! | t08  | a bound `sys_autonomous_daily_operations` row is never touched by this function -- \
//! |      | only `runs` changes; the coordinator's own next tick owns operation reconciliation |
//! | t09  | mutation control: reverting the outbox check (simulated by asserting the pre-fix \
//! |      | shape would have wrongly reported Stopped) -- documented via t05's own RED/GREEN proof \
//! |      | in its doc comment; t09 proves the GREEN (fixed) path end-to-end for a claimed (not \
//! |      | just pending) unacked row, the other unacked terminal-adjacent state |

use chrono::Utc;
use mqk_db::{
    arm_run, begin_run, halt_run, insert_run, outbox_enqueue, persist_reconcile_status_state,
    stop_run, NewRun, PersistReconcileStatusState, RunStatus, StopRunIfEvidenceCleanOutcome,
};
use uuid::Uuid;

fn run_id_for(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
}

async fn seed_clean_reconcile(pool: &sqlx::PgPool) {
    persist_reconcile_status_state(
        pool,
        &PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(Utc::now()),
            snapshot_watermark_ms: Some(1),
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: Utc::now(),
        },
    )
    .await
    .expect("persist_reconcile_status_state clean");
}

async fn seed_dirty_reconcile(pool: &sqlx::PgPool) {
    persist_reconcile_status_state(
        pool,
        &PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(Utc::now()),
            snapshot_watermark_ms: Some(1),
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: injected mismatch"),
            updated_at_utc: Utc::now(),
        },
    )
    .await
    .expect("persist_reconcile_status_state dirty");
}

// ---------------------------------------------------------------------------
// t01: the exact crash-orphan shape -- RUNNING, zero unacked outbox, clean
// reconcile -> Stopped, durable.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t01_running_clean_evidence_is_stopped() {
    mqk_db::run_isolated("stop_orphan_t01", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t01");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::Stopped,
            "t01: RUNNING + clean evidence must be released"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Stopped),
            "t01: run must be durably STOPPED after release; got {:?}",
            after.status
        );
        assert!(
            after.stopped_at_utc.is_some(),
            "t01: stopped_at_utc must be recorded"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t02: ARMED (never actually began -- e.g. crash between arm and begin)
// with clean evidence -> Stopped.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t02_armed_never_begun_clean_evidence_is_stopped() {
    mqk_db::run_isolated("stop_orphan_t02", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t02");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        seed_clean_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::Stopped,
            "t02: ARMED + clean evidence must be released"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t03/t04: negative controls -- not durably ARMED/RUNNING at all.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t03_already_stopped_is_not_active() {
    mqk_db::run_isolated("stop_orphan_t03", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t03");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        stop_run(&pool, run_id).await.expect("stop_run");
        seed_clean_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::NotActive {
                actual_status: "STOPPED".to_string()
            },
            "t03: an already-STOPPED run must report NotActive, zero mutation"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t04_halted_is_never_overwritten() {
    mqk_db::run_isolated("stop_orphan_t04", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t04");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        halt_run(&pool, run_id, Utc::now()).await.expect("halt_run");
        seed_clean_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::NotActive {
                actual_status: "HALTED".to_string()
            },
            "t04: a sticky HALTED run must never be presented as recoverable via this path"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Halted),
            "t04: HALTED must remain HALTED -- zero mutation on refusal"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t05: unacked outbox negative control.
//
// RED/GREEN mutation proof (performed manually during development, recorded
// here): commenting out the `if !unacked.is_empty()` early-return in
// `stop_run_if_evidence_clean` reproduces this test failing with
// `Stopped` instead of `UnacknowledgedOutbox` -- restoring the check returns
// it to green. This is the same evidence class
// `reconcile_durable_run_without_local_owner` already gates on for the
// operation-level question; this test proves the run-level primitive gates
// on it identically.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t05_unacked_outbox_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t05", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t05");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        outbox_enqueue(
            &pool,
            run_id,
            "orphan-repair-t05-key",
            serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1}),
        )
        .await
        .expect("outbox_enqueue");

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::UnacknowledgedOutbox { unacked_count: 1 },
            "t05: a PENDING outbox row must block release"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t05: a refused release must not mutate run status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t06/t07: reconcile negative controls.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t06_dirty_reconcile_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t06", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t06");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_dirty_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::ReconcileDirty,
            "t06: mismatched_positions > 0 must block release"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t06: a refused release must not mutate run status"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t07_absent_reconcile_status_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t07", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t07");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        // Deliberately no persist_reconcile_status_state call: sys_reconcile_status_state
        // is empty for this isolated DB.

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::ReconcileDirty,
            "t07: no durable reconcile status at all is not evidence of agreement -- must \
             fail closed exactly like a dirty one, never assumed clean"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t08: no fabricated stop/finalization at the operation layer.
//
// `stop_run_if_evidence_clean` must touch only `runs` -- a bound
// `sys_autonomous_daily_operations` row's own state/stopped_at_utc/
// finalized_at_utc are untouched by this call. The coordinator
// (`reconcile_durable_run_without_local_owner`, reached from
// `handle_running`/`handle_session_close`/the `controller_degraded` tick
// arm in mqk-daemon) owns reconciling the operation itself on its own next
// tick -- this function's job stops at the run row.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t08_bound_operation_row_is_untouched() {
    use mqk_db::{
        create_or_recover_autonomous_daily_operation, transition_autonomous_daily_operation,
        AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs,
        CreateOrRecoverAutonomousDailyOperationOutcome, TransitionAutonomousDailyOperationArgs,
        STATE_AWAITING_OPEN, STATE_AWAITING_PREOPEN, STATE_PREPARING_DATA, STATE_RUNNING,
        STATE_START_RETRYING,
    };
    use chrono::{Duration as ChronoDuration, NaiveDate, TimeZone};

    mqk_db::run_isolated("stop_orphan_t08", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t08");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let market_date = NaiveDate::from_ymd_opt(2026, 8, 19).unwrap();
        let open = Utc
            .with_ymd_and_hms(2026, 8, 19, 13, 30, 0)
            .unwrap();
        let close = open + ChronoDuration::hours(6) + ChronoDuration::minutes(30);
        let preopen = open - ChronoDuration::minutes(30);
        let postclose = close + ChronoDuration::minutes(15);
        let operation_id = run_id_for("mqk-daemon.orphan-repair.t08.operation");
        let args = CreateAutonomousDailyOperationArgs {
            operation_id,
            market_date,
            deployment_mode: "PAPER".to_string(),
            adapter_id: "orphan-repair-t08".to_string(),
            session_plan_identity: "orphan-repair-t08-plan".to_string(),
            assignment_identity: "orphan-repair-t08-assignment".to_string(),
            runtime_binding_identity: "orphan-repair-t08-binding".to_string(),
            calendar_source: "nyse_weekdays_heuristic".to_string(),
            calendar_coverage_state: "active".to_string(),
            schedule_source: "nyse_weekdays_heuristic".to_string(),
            effective_operation_open_utc: open,
            effective_operation_close_utc: close,
            exchange_session_open_utc: open,
            exchange_session_close_utc: close,
            exchange_is_early_close: false,
            previous_trading_date: market_date - ChronoDuration::days(3),
            preopen_start_utc: preopen,
            postclose_finalize_utc: postclose,
            initial_state: STATE_AWAITING_PREOPEN.to_string(),
            data_refresh_state: "not_started".to_string(),
            occurred_at_utc: preopen,
            bounded_detail: "t08 test setup".to_string(),
            stop_attempt_count: 0,
        };
        let created = match create_or_recover_autonomous_daily_operation(&pool, &args)
            .await
            .expect("create operation")
        {
            CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => r,
            other => panic!("expected Created, got {other:?}"),
        };

        // Advance the operation to `running`, bound to run_id -- the exact
        // shape Wednesday's real stuck operation was found in.
        async fn advance(
            pool: &sqlx::PgPool,
            row: mqk_db::AutonomousDailyOperationRecord,
            new_state: &str,
            run_id: Option<Uuid>,
            ts: chrono::DateTime<Utc>,
        ) -> mqk_db::AutonomousDailyOperationRecord {
            let args = TransitionAutonomousDailyOperationArgs {
                operation_id: row.operation_id,
                expected_state: row.state.clone(),
                expected_state_version: row.state_version,
                new_state: new_state.to_string(),
                reason_code: None,
                blocker_signature: None,
                occurred_at_utc: ts,
                run_id,
                bounded_detail: format!("t08 test setup: -> {new_state}"),
            };
            match transition_autonomous_daily_operation(pool, &args)
                .await
                .expect("transition")
            {
                AutonomousDailyTransitionOutcome::Applied(r) => r,
                other => panic!("expected Applied transitioning to {new_state}, got {other:?}"),
            }
        }
        let row = advance(&pool, created, STATE_PREPARING_DATA, None, preopen).await;
        let row = advance(&pool, row, STATE_AWAITING_OPEN, None, preopen).await;
        let row = advance(&pool, row, STATE_START_RETRYING, None, preopen).await;
        let bound = advance(&pool, row, STATE_RUNNING, Some(run_id), preopen).await;
        assert_eq!(bound.state, STATE_RUNNING);
        assert_eq!(bound.run_id, Some(run_id));
        assert!(bound.stopped_at_utc.is_none());

        let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
            .await
            .expect("fetch before")
            .expect("operation row must exist");

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(outcome, StopRunIfEvidenceCleanOutcome::Stopped);

        let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
            .await
            .expect("fetch after")
            .expect("operation row must still exist");
        assert_eq!(
            after.state, before.state,
            "t08: stop_run_if_evidence_clean must never mutate the bound operation's state"
        );
        assert_eq!(
            after.state_version, before.state_version,
            "t08: stop_run_if_evidence_clean must never advance the operation's state_version"
        );
        assert_eq!(
            after.stopped_at_utc, before.stopped_at_utc,
            "t08: stop_run_if_evidence_clean must never fabricate the operation's \
             stopped_at_utc -- that remains the coordinator's own next-tick job"
        );
        assert_eq!(
            after.finalized_at_utc, before.finalized_at_utc,
            "t08: stop_run_if_evidence_clean must never fabricate finalization"
        );

        // The run itself IS durably stopped -- only the run, not the operation.
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, RunStatus::Stopped));
    })
    .await;
}

// ---------------------------------------------------------------------------
// t09: the other unacked-terminal-adjacent outbox status (CLAIMED) also
// blocks release -- not just PENDING.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t09_claimed_outbox_row_also_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t09", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t09");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        outbox_enqueue(
            &pool,
            run_id,
            "orphan-repair-t09-key",
            serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1}),
        )
        .await
        .expect("outbox_enqueue");
        sqlx::query(
            "update oms_outbox set status = 'CLAIMED', claimed_at_utc = now(), \
             claimed_by = 'test-claimer' where run_id = $1",
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("mark outbox row CLAIMED");

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::UnacknowledgedOutbox { unacked_count: 1 },
            "t09: a stranded CLAIMED row (never reached DISPATCHING/ACKED) must also block \
             release -- an order may still be in flight"
        );
    })
    .await;
}
