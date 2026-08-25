//! PAPER-SOAK-OUTBOX-ENQUEUE-RUN-STATE-FENCE-01: proof tests for
//! `mqk_db::outbox_enqueue_for_running_run`.
//!
//! ## Why this exists
//!
//! `stop_run_if_evidence_clean` (PAPER-SOAK-ORPHAN-RECOVERY-ATOMIC-FENCE-01,
//! commit 4125d364) correctly locks the `runs` row with `SELECT ... FOR
//! UPDATE` before transitioning an orphaned run to `STOPPED`, closing the
//! race against `outbox_claim_batch_for_run_with_lease_authority`. But its
//! own concurrency proof (the retired `t16` in
//! `scenario_stop_run_if_evidence_clean_20260819.rs`) exposed a second race
//! on the *other* side of that same lock: `oms_outbox.run_id REFERENCES
//! runs(run_id)` makes a concurrent `INSERT` acquire an implicit `FOR KEY
//! SHARE` lock on the parent `runs` row, so it blocks behind recovery's
//! `FOR UPDATE` -- but once recovery commits `STOPPED` and releases the
//! lock, the old unfenced `outbox_enqueue` resumed and inserted the row as
//! `PENDING` anyway, because it only proves `run_id` exists as an FK
//! target, never that `runs.status == 'RUNNING'`. `t16` only proved that
//! row was *unclaimable while STOPPED* -- but `STOPPED -> ARMED` rearm is a
//! normal lifecycle transition (`arm_run`'s CAS guard explicitly accepts
//! `CREATED` or `STOPPED`), so a stale `PENDING` row survives recovery and
//! becomes claimable future work under the next run cycle: a durable
//! economic-intent leak across a run boundary.
//!
//! `outbox_enqueue_for_running_run` closes this by locking the same `runs`
//! row first and requiring `status = 'RUNNING'` before performing the
//! insert, inside one transaction -- the identical serialization boundary
//! `stop_run_if_evidence_clean` and
//! `outbox_claim_batch_for_run_with_lease_authority` already use.
//!
//! All tests require `MQK_DATABASE_URL` and run against an isolated
//! disposable database via `mqk_db::run_isolated` (never the shared/Paper
//! database). Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --features testkit \
//!   --test scenario_outbox_enqueue_run_state_fence_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! ## Test matrix
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | t1   | RECOVERY WINS, genuine concurrency: a concurrent enqueue attempt for the locked \
//! |      | run_id genuinely blocks on the runs-row FOR UPDATE (proven via bounded timeout, \
//! |      | replacing the retired t16's insufficient "unclaimable while STOPPED" proof), then \
//! |      | unblocks to RunNotRunning once the row transitions STOPPED -- zero PENDING row |
//! | t2   | ENQUEUE WINS: a real enqueue commits PENDING first; real stop_run_if_evidence_clean \
//! |      | called after it then refuses via UnacknowledgedOutbox -- the run stays RUNNING |
//! | t3   | STOPPED run -> RunNotRunning, zero row |
//! | t4   | ARMED (never begun) run -> RunNotRunning, zero row |
//! | t5   | HALTED run -> RunNotRunning, zero row |
//! | t6   | RUNNING run, duplicate idempotency key -> Duplicate, distinct from RunNotRunning |
//! | t7   | RUNNING run, two distinct idempotency keys -> both Enqueued, no intent loss |
//! | t8   | stale runtime snapshot: caller observes RUNNING, run is stopped before the final \
//! |      | DB enqueue call -> still refuses; disproves reliance on an earlier snapshot read |
//! | t9   | post-recovery rearm safety: after t1's refusal and a STOPPED -> ARMED rearm, the \
//! |      | refused idempotency key still has zero durable row -- no resurrection |

use chrono::Utc;
use mqk_db::{
    arm_run, begin_run, halt_run, insert_run, persist_reconcile_status_state, stop_run, NewRun,
    OutboxEnqueueOutcome, PersistReconcileStatusState,
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

async fn seed_running(pool: &sqlx::PgPool, run_id: Uuid) {
    seed_run(pool, run_id).await;
    arm_run(pool, run_id).await.expect("arm_run");
    begin_run(pool, run_id).await.expect("begin_run");
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

fn order_json() -> serde_json::Value {
    serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1})
}

// T1 / RECOVERY WINS -- genuine concurrency, replaces the retired t16.
//
// A raw-SQL transaction holds the exact same runs-row `FOR UPDATE` lock
// `stop_run_if_evidence_clean` takes first (the manual hold stands in for
// "recovery is mid-transaction", exactly as the retired t16 did for the
// insert-side proof -- the evidence-check business logic itself is proven
// independently and unchanged by `scenario_stop_run_if_evidence_clean_20260819.rs`,
// which this patch does not modify). While the lock is held, a concurrent
// call to the real `outbox_enqueue_for_running_run` is spawned and proven to
// genuinely block (bounded timeout, not a sleep) on the FK's implicit `FOR
// KEY SHARE` lock. The holder then performs the identical CAS-guarded
// `STOPPED` transition `stop_run_if_evidence_clean` performs and commits,
// releasing the lock. The previously-blocked enqueue must unblock to
// `RunNotRunning` and must create zero row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t1_recovery_wins_concurrent_enqueue_blocks_then_refuses() {
    mqk_db::run_isolated("outbox_fence_t1", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t1");
        seed_running(&pool, run_id).await;

        let mut tx = pool.begin().await.expect("begin tx");
        let status: String =
            sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await
                .expect("lock runs row");
        assert_eq!(status, "RUNNING", "t1 precondition: run must be RUNNING");

        let pool_for_enqueue = pool.clone();
        let key = "outbox-fence-t1-key".to_string();
        let mut enqueue_task = tokio::spawn(async move {
            mqk_db::outbox_enqueue_for_running_run(&pool_for_enqueue, run_id, &key, order_json())
                .await
        });

        // Bounded wait, not a coordination sleep: proving genuine blocking
        // (a liveness negative) has no Notify-based equivalent here, since
        // the blocked task cannot signal "I am now blocked" from inside a
        // single non-instrumented SQL call -- same technique the retired
        // t16 used for this exact lock.
        let raced =
            tokio::time::timeout(std::time::Duration::from_millis(800), &mut enqueue_task).await;
        assert!(
            raced.is_err(),
            "t1: a concurrent fenced enqueue for the locked run_id must block on the FK's \
             implicit FOR KEY SHARE lock against the still-open FOR UPDATE, not proceed \
             immediately -- if this fails, the enqueue-side race is open"
        );

        // Perform the identical CAS-guarded STOPPED transition
        // stop_run_if_evidence_clean performs, while still holding the lock
        // this task has held since before the enqueue was spawned.
        let now = Utc::now();
        let done = sqlx::query(
            r#"
            update runs
               set status = 'STOPPED',
                   stopped_at_utc = $2
             where run_id = $1
               and status in ('ARMED', 'RUNNING')
            "#,
        )
        .bind(run_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .expect("stop update");
        assert_eq!(done.rows_affected(), 1, "t1: STOPPED transition must apply");
        tx.commit().await.expect("commit (release lock)");

        let outcome = enqueue_task
            .await
            .expect("enqueue task must not panic")
            .expect("enqueue call must not error");
        assert_eq!(
            outcome,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t1: an enqueue unblocked after the run transitioned STOPPED must refuse via \
             RunNotRunning, never silently create a PENDING row"
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t1-key")
            .await
            .expect("fetch by key must not error");
        assert!(
            row.is_none(),
            "t1: zero PENDING row must exist for a refused enqueue -- the mutation this patch \
             exists to close"
        );
    })
    .await;
}

// T2 / ENQUEUE WINS: a real enqueue commits PENDING first (using only the
// production fenced function); a real stop_run_if_evidence_clean call
// after it must then refuse via UnacknowledgedOutbox because the outbox is
// no longer clean -- the run must stay RUNNING, proving the other required
// ordering with no raw SQL at all.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t2_enqueue_wins_recovery_then_refuses() {
    mqk_db::run_isolated("outbox_fence_t2", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t2");
        seed_running(&pool, run_id).await;

        let enqueued = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t2-key",
            order_json(),
        )
        .await
        .expect("enqueue must not error");
        assert_eq!(
            enqueued,
            OutboxEnqueueOutcome::Enqueued,
            "t2 precondition: enqueue against a RUNNING run must succeed"
        );

        let outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean must not error");
        assert!(
            matches!(
                outcome,
                mqk_db::StopRunIfEvidenceCleanOutcome::UnacknowledgedOutbox { .. }
            ),
            "t2: recovery arriving after a PENDING row was legitimately enqueued must refuse \
             via UnacknowledgedOutbox, not silently stop the run out from under live intent; \
             got {outcome:?}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Running),
            "t2: a refused recovery must not mutate run status"
        );
    })
    .await;
}

// T3: a durably STOPPED run refuses new economic intent.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t3_stopped_run_refuses() {
    mqk_db::run_isolated("outbox_fence_t3", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t3");
        seed_running(&pool, run_id).await;
        stop_run(&pool, run_id).await.expect("stop_run");

        let outcome = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t3-key",
            order_json(),
        )
        .await
        .expect("enqueue call must not error");
        assert_eq!(
            outcome,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            }
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t3-key")
            .await
            .expect("fetch by key must not error");
        assert!(row.is_none(), "t3: zero row for a STOPPED run");
    })
    .await;
}

// T4: an ARMED-but-never-begun run refuses new economic intent -- only
// RUNNING accepts.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t4_armed_run_refuses() {
    mqk_db::run_isolated("outbox_fence_t4", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t4");
        seed_run(&pool, run_id).await;
        arm_run(&pool, run_id).await.expect("arm_run");

        let outcome = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t4-key",
            order_json(),
        )
        .await
        .expect("enqueue call must not error");
        assert_eq!(
            outcome,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "ARMED".to_string()
            }
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t4-key")
            .await
            .expect("fetch by key must not error");
        assert!(row.is_none(), "t4: zero row for an ARMED run");
    })
    .await;
}

// T5: a HALTED run refuses new economic intent.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t5_halted_run_refuses() {
    mqk_db::run_isolated("outbox_fence_t5", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t5");
        seed_running(&pool, run_id).await;
        halt_run(&pool, run_id, Utc::now()).await.expect("halt_run");

        let outcome = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t5-key",
            order_json(),
        )
        .await
        .expect("enqueue call must not error");
        assert_eq!(
            outcome,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "HALTED".to_string()
            }
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t5-key")
            .await
            .expect("fetch by key must not error");
        assert!(row.is_none(), "t5: zero row for a HALTED run");
    })
    .await;
}

// T6: duplicate idempotency key against a RUNNING run returns Duplicate,
// distinct from RunNotRunning -- callers must not conflate "already
// exists" with "durable run no longer permits creation of economic
// intent".
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t6_duplicate_key_is_distinct_from_run_not_running() {
    mqk_db::run_isolated("outbox_fence_t6", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t6");
        seed_running(&pool, run_id).await;

        let first = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t6-key",
            order_json(),
        )
        .await
        .expect("first enqueue must not error");
        assert_eq!(first, OutboxEnqueueOutcome::Enqueued);

        let second = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t6-key",
            order_json(),
        )
        .await
        .expect("second enqueue must not error");
        assert_eq!(
            second,
            OutboxEnqueueOutcome::Duplicate,
            "t6: a repeated idempotency key against the same RUNNING run must report Duplicate"
        );
        assert_ne!(
            second,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "RUNNING".to_string()
            },
            "t6: Duplicate must never be conflated with RunNotRunning"
        );
    })
    .await;
}

// T7: two distinct idempotency keys against a RUNNING run both succeed
// serially -- the fence must not cause any intent loss for legitimate
// concurrent-in-time-but-distinct enqueues.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t7_two_distinct_keys_both_enqueue() {
    mqk_db::run_isolated("outbox_fence_t7", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t7");
        seed_running(&pool, run_id).await;

        let a = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t7-key-a",
            order_json(),
        )
        .await
        .expect("enqueue a must not error");
        let b = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t7-key-b",
            order_json(),
        )
        .await
        .expect("enqueue b must not error");

        assert_eq!(a, OutboxEnqueueOutcome::Enqueued, "t7: key a must enqueue");
        assert_eq!(b, OutboxEnqueueOutcome::Enqueued, "t7: key b must enqueue");
    })
    .await;
}

// T8 / STALE HIGHER-LEVEL SNAPSHOT: a caller observes RUNNING via
// `fetch_run` (mirroring the advisory `status.state == "running"` checks in
// `mqk-daemon`'s decision/execution/strategy routes), then the durable run
// transitions away from RUNNING before the caller's final DB enqueue call.
// The fenced enqueue -- the durable enforcement point -- must still refuse,
// disproving reliance on the earlier, now-stale snapshot.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t8_stale_snapshot_does_not_bypass_fence() {
    mqk_db::run_isolated("outbox_fence_t8", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t8");
        seed_running(&pool, run_id).await;

        // The caller's advisory snapshot read -- exactly what
        // mqk-daemon's routes do before calling the enqueue seam.
        let snapshot = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(snapshot.status, mqk_db::RunStatus::Running),
            "t8 precondition: snapshot must observe RUNNING"
        );

        // Between the snapshot and the caller's eventual DB write, an
        // unrelated operator/runtime action stops the run.
        stop_run(&pool, run_id).await.expect("stop_run");

        // The caller proceeds to enqueue using the (now stale) RUNNING
        // observation -- exactly the shape of decision.rs/execution.rs/
        // strategy.rs, which check a snapshot in a separate transaction
        // before calling the enqueue seam.
        let outcome = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t8-key",
            order_json(),
        )
        .await
        .expect("enqueue call must not error");
        assert_eq!(
            outcome,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t8: the durable enqueue call must refuse even though the caller's earlier snapshot \
             observed RUNNING -- a stale higher-level check must never substitute for the \
             durable fence"
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t8-key")
            .await
            .expect("fetch by key must not error");
        assert!(row.is_none(), "t8: zero row despite the stale snapshot");
    })
    .await;
}

// T9 / POST-RECOVERY REARM SAFETY: after a recovery-wins ordering refuses
// an enqueue (mirroring t1) and the run is later legitimately rearmed
// (STOPPED -> ARMED, a normal lifecycle transition), the previously-refused
// idempotency key must still have zero durable row -- rearm must never
// resurrect a stale intent the fence already refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t9_post_recovery_rearm_does_not_resurrect_refused_intent() {
    mqk_db::run_isolated("outbox_fence_t9", |pool| async move {
        let run_id = run_id_for("mqk-daemon.outbox-fence.t9");
        seed_running(&pool, run_id).await;
        seed_clean_reconcile(&pool).await;

        // Recovery wins first (sequential composition of the real
        // functions is sufficient here -- t1 already proves the genuine
        // concurrent-blocking shape of this same ordering).
        let stop_outcome = mqk_db::stop_run_if_evidence_clean(&pool, run_id, Utc::now())
            .await
            .expect("stop_run_if_evidence_clean must not error");
        assert_eq!(
            stop_outcome,
            mqk_db::StopRunIfEvidenceCleanOutcome::Stopped,
            "t9 precondition: recovery must succeed"
        );

        let refused = mqk_db::outbox_enqueue_for_running_run(
            &pool,
            run_id,
            "outbox-fence-t9-key",
            order_json(),
        )
        .await
        .expect("enqueue call must not error");
        assert_eq!(
            refused,
            OutboxEnqueueOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "t9 precondition: the enqueue after recovery must be refused"
        );

        // Legitimate rearm: STOPPED -> ARMED.
        arm_run(&pool, run_id).await.expect("rearm via arm_run");
        let after_rearm = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after_rearm.status, mqk_db::RunStatus::Armed),
            "t9 precondition: rearm must succeed"
        );

        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, "outbox-fence-t9-key")
            .await
            .expect("fetch by key must not error");
        assert!(
            row.is_none(),
            "t9: rearm must never resurrect an intent the fence already refused -- zero row \
             must remain zero indefinitely"
        );
    })
    .await;
}
