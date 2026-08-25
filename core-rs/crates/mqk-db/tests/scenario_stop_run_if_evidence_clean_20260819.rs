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
//! |      | just pending) unacked row, the other unacked terminal-adjacent state
//! | t10  | PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: an unapplied oms_inbox row -> \
//! |      | UnappliedInbox, run stays RUNNING |
//! | t11  | PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: unmatched_broker_events > 0 (status \
//! |      | ok, every mismatch counter 0) -> ReconcileDirty, run stays RUNNING |
//! | t12  | PAPER-SOAK-ORPHAN-RECOVERY-ATOMIC-FENCE-01 (RACE-1/RACE-4): an unexpired runtime \
//! |      | leader lease refuses release via ActiveRuntimeLease even with otherwise-clean \
//! |      | evidence, zero mutation |
//! | t13  | RACE-2: a successful production outbox claim under a still-active lease, then \
//! |      | recovery -> ActiveRuntimeLease refusal; the CLAIMED row is never touched by this \
//! |      | test (no broker submission is ever made) |
//! | t14  | RACE-3: recovery with no active lease wins the row lock first -> Stopped; a \
//! |      | lease acquire/refresh and an outbox claim attempted afterward both see \
//! |      | RunNotRunning, never silently reacquiring authority over the stopped run |
//! | t15  | RACE-5: an *expired* lease does not block recovery -> Stopped; the stale holder \
//! |      | cannot refresh/reacquire afterward (RunNotRunning, not Lost) |
//! | t16  | RACE-10 (PENDING ENQUEUE ANALYSIS): the runs-row `FOR UPDATE` lock \
//! |      | `stop_run_if_evidence_clean` takes first genuinely blocks a concurrent \
//! |      | `outbox_enqueue` for the same run_id (proven via the FK's implicit `FOR KEY \
//! |      | SHARE` lock, not merely asserted); once unblocked, a PENDING row landing after \
//! |      | STOPPED is proven unclaimable via the production claim path |

use chrono::Utc;
use mqk_db::{
    arm_run, begin_run, halt_run, inbox_insert_deduped, insert_run, outbox_enqueue,
    persist_reconcile_status_state, stop_run, NewRun, PersistReconcileStatusState, RunStatus,
    StopRunIfEvidenceCleanOutcome,
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
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

// ---------------------------------------------------------------------------
// t10/t11: PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01 negative controls.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t10_unapplied_inbox_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t10", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t10");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        inbox_insert_deduped(
            &pool,
            run_id,
            "orphan-repair-t10-msg",
            serde_json::json!({"event_kind": "fill"}),
        )
        .await
        .expect("inbox_insert_deduped");

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::UnappliedInbox { unapplied_count: 1 },
            "t10: an unapplied inbox row must block release"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t10: a refused release must not mutate run status"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t11_unmatched_broker_events_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t11", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t11");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        persist_reconcile_status_state(
            &pool,
            &PersistReconcileStatusState {
                status: "ok",
                last_run_at_utc: Some(Utc::now()),
                snapshot_watermark_ms: Some(1),
                mismatched_positions: 0,
                mismatched_orders: 0,
                mismatched_fills: 0,
                unmatched_broker_events: 1,
                note: Some("test: simulated unmatched broker event"),
                updated_at_utc: Utc::now(),
            },
        )
        .await
        .expect("persist_reconcile_status_state unmatched_broker_events");

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::ReconcileDirty,
            "t11: unmatched_broker_events > 0 must block release even when status='ok' and \
             every mismatch counter is zero"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t11: a refused release must not mutate run status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// t12-t16: PAPER-SOAK-ORPHAN-RECOVERY-ATOMIC-FENCE-01 -- runtime-lease
// serialization races. See this file's module doc comment for the RACE-N
// mapping this closes.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t12_active_runtime_lease_blocks_release() {
    mqk_db::run_isolated("stop_orphan_t12", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t12");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let lease = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-t12",
            None,
            Utc::now(),
            300,
        )
        .await
        .expect("acquire lease");
        let (want_holder, want_epoch) = match lease {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(l) => (l.holder_id, l.epoch),
            other => panic!("t12 precondition: expected Acquired, got {other:?}"),
        };

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean");
        match outcome {
            StopRunIfEvidenceCleanOutcome::ActiveRuntimeLease {
                holder_id,
                epoch,
                ..
            } => {
                assert_eq!(holder_id, want_holder, "t12: body: {holder_id}");
                assert_eq!(epoch, want_epoch, "t12: body: {epoch}");
            }
            other => panic!("t12: expected ActiveRuntimeLease, got {other:?}"),
        }

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t12: an unexpired lease must refuse release with zero mutation -- \
             'no local owner' is only process-local evidence, a lease is durable proof another \
             runtime may still act"
        );
    })
    .await;
}

// RACE-2: a real runtime successfully claims under a still-active lease
// (production `outbox_claim_batch_for_run_with_lease_authority`), THEN
// recovery arrives. Recovery must refuse -- the claim proved current,
// unexpired authority over this exact run.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t13_claim_wins_first_recovery_then_refuses() {
    mqk_db::run_isolated("stop_orphan_t13", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t13");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        outbox_enqueue(
            &pool,
            run_id,
            "orphan-repair-t13-key",
            serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1}),
        )
        .await
        .expect("outbox_enqueue");

        let lease = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-t13",
            None,
            Utc::now(),
            300,
        )
        .await
        .expect("acquire lease");
        let (holder_id, epoch) = match lease {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(l) => (l.holder_id, l.epoch),
            other => panic!("t13 precondition: expected Acquired, got {other:?}"),
        };

        let claimed = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            &holder_id,
            epoch,
            10,
            "dispatcher-t13",
            Utc::now(),
        )
        .await
        .expect("claim");
        let claimed_rows = match claimed {
            mqk_db::FencedClaimOutcome::Claimed(rows) => rows,
            other => panic!("t13 precondition: expected Claimed, got {other:?}"),
        };
        assert_eq!(
            claimed_rows.len(),
            1,
            "t13 precondition: exactly one row must be claimed"
        );

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean");
        assert!(
            matches!(
                outcome,
                StopRunIfEvidenceCleanOutcome::ActiveRuntimeLease { .. }
            ),
            "t13: recovery arriving after a successful claim under the same still-active lease \
             must refuse via ActiveRuntimeLease; got {outcome:?}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, RunStatus::Running),
            "t13: a refused release must not mutate run status -- the CLAIMED row remains the \
             live runtime's responsibility; this test never submits it to a broker"
        );
    })
    .await;
}

// RACE-3: recovery observes no active lease and clean evidence, wins the
// row lock first, commits STOPPED. A lease acquire/refresh and an outbox
// claim attempted afterward must both see RunNotRunning -- proving the
// runs-row lock this function takes actually fences out every later
// contender via the same serialization boundary those primitives use.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t14_recovery_wins_first_then_lease_and_claim_refuse() {
    mqk_db::run_isolated("stop_orphan_t14", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t14");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::Stopped,
            "t14 precondition: no lease + clean evidence must release"
        );

        let lease_after = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-t14",
            None,
            Utc::now(),
            300,
        )
        .await
        .expect("lease attempt must not error");
        assert_eq!(
            lease_after,
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t14: a lease attempt after recovery already committed STOPPED must see \
             RunNotRunning, never silently acquire authority over a stopped run"
        );

        let claim_after = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-t14",
            1,
            10,
            "dispatcher-t14",
            Utc::now(),
        )
        .await
        .expect("claim attempt must not error");
        assert_eq!(
            claim_after,
            mqk_db::FencedClaimOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t14: an outbox claim attempt after recovery already committed STOPPED must see \
             RunNotRunning"
        );
    })
    .await;
}

// RACE-5: an expired lease is not active authority -- it must not block
// recovery, and the stale holder must not be able to revive it afterward.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t15_expired_lease_does_not_block_recovery() {
    mqk_db::run_isolated("stop_orphan_t15", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t15");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let acquire_at = Utc::now();
        let lease = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-t15",
            None,
            acquire_at,
            5,
        )
        .await
        .expect("acquire lease");
        let epoch = match lease {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(l) => l.epoch,
            other => panic!("t15 precondition: expected Acquired, got {other:?}"),
        };

        // Recovery observed strictly after the 5s TTL: durably expired, not
        // active authority.
        let recovery_at = acquire_at + chrono::Duration::seconds(10);
        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, recovery_at)
            .await
            .expect("stop_run_if_evidence_clean");
        assert_eq!(
            outcome,
            StopRunIfEvidenceCleanOutcome::Stopped,
            "t15: an expired lease must not block recovery"
        );

        let stale_refresh = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-t15",
            Some(epoch),
            recovery_at + chrono::Duration::seconds(1),
            5,
        )
        .await
        .expect("refresh attempt must not error");
        assert_eq!(
            stale_refresh,
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t15: the stale holder must not be able to refresh/reacquire authority once the \
             run is durably STOPPED"
        );
    })
    .await;
}

// RACE-10 / PENDING ENQUEUE ANALYSIS: prove, not merely assert, that the
// runs-row `SELECT ... FOR UPDATE` lock `stop_run_if_evidence_clean` takes
// first genuinely blocks a concurrent outbox insert for the same run_id.
// `oms_outbox.run_id REFERENCES runs(run_id)` -- inserting a referencing row
// makes Postgres acquire an implicit `FOR KEY SHARE` lock on the referenced
// parent row for the FK check, which conflicts with an already-held
// `FOR UPDATE` lock, so the insert cannot proceed until the lock-holding
// transaction commits or rolls back.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t16_concurrent_outbox_insert_blocks_on_run_row_lock_then_lands_inert() {
    mqk_db::run_isolated("stop_orphan_t16", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair.t16");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");
        begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        // Hold the exact same runs-row lock stop_run_if_evidence_clean takes
        // FIRST, directly -- this proves the underlying Postgres
        // serialization guarantee independent of any one caller.
        let mut tx = pool.begin().await.expect("begin tx");
        let _status: String =
            sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await
                .expect("lock runs row");

        let pool_for_insert = pool.clone();
        let mut insert_task = tokio::spawn(async move {
            outbox_enqueue(
                &pool_for_insert,
                run_id,
                "orphan-repair-t16-key",
                serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1}),
            )
            .await
        });

        // Bounded wait, not a coordination sleep: proving genuine blocking
        // (a liveness negative) has no Notify-based equivalent here, since
        // the blocked task cannot signal "I am now blocked" from inside a
        // single non-instrumented SQL call -- this is the standard technique
        // for proving a lock blocks.
        let raced =
            tokio::time::timeout(std::time::Duration::from_millis(800), &mut insert_task).await;
        assert!(
            raced.is_err(),
            "t16: a concurrent outbox insert for the locked run_id must block on the FK's \
             implicit FOR KEY SHARE lock against the still-open FOR UPDATE, not proceed \
             immediately -- if this fails, the PENDING ENQUEUE ANALYSIS race is open and the \
             mission requires STOPPING, not silently declaring it harmless"
        );

        // Release the lock exactly like a completed recovery transaction
        // would.
        tx.commit().await.expect("commit (release lock)");

        let inserted = insert_task
            .await
            .expect("insert task must not panic")
            .expect("outbox_enqueue must succeed once the lock is released");
        assert!(inserted, "t16: the insert must land once unblocked");

        // Prove the landed row is provably inert once the run is STOPPED: a
        // PENDING row for a non-RUNNING run can never be claimed, so a
        // stranded post-STOPPED enqueue -- however it lands -- can never
        // reach dispatch.
        stop_run(&pool, run_id).await.expect("stop_run");
        let claim = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-t16",
            1,
            10,
            "dispatcher-t16",
            Utc::now(),
        )
        .await
        .expect("claim attempt must not error");
        assert_eq!(
            claim,
            mqk_db::FencedClaimOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t16: a PENDING row stranded against a STOPPED run must never be claimable -- the \
             claim path's own runs-row FOR UPDATE + status='RUNNING' check refuses it \
             regardless of ordering"
        );
    })
    .await;
}
