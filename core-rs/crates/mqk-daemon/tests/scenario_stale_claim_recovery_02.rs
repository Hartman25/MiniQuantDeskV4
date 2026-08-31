//! PAPER-SOAK-STALE-CLAIM-RECOVERY-02/03 — acceptance tests B1-B11 + SCR03.
//!
//! # Root cause (see doc comment on
//! `mqk_db::clear_halted_run_and_reset_stale_claims` for the full analysis)
//!
//! The previously-pushed repair (`PAPER-SOAK-STALE-CLAIM-RECOVERY-01`) called
//! `outbox_reset_stale_claims` unconditionally inside
//! `AppState::build_execution_orchestrator`, before any runtime leadership
//! lease existed for that orchestrator, and for a crash-recovery scenario
//! (`RUNNING` run crashes, leaves a `CLAIMED` row behind) that the normal
//! start path can never actually reach — `create_or_reuse_run_for_start`
//! refuses to start when a durable active run exists without local
//! ownership, so `build_execution_orchestrator` is never invoked for that
//! run_id.
//!
//! # -02's fix, and -02's false assumption (corrected by -03)
//!
//! -02 moved stale-claim recovery into this repository's existing,
//! deliberately operator-mediated recovery path: `clear-halted-run`
//! (`mqk_db::clear_halted_run_and_reset_stale_claims`), and claimed durable
//! `HALTED` alone was "a complete, race-proof authority check — no runtime
//! lease inspection needed", reasoning that the orchestrator's Phase-0 I9-1
//! halt guard refuses ALL dispatch for ANY process once a run is durably
//! `HALTED`.
//!
//! That reasoning has a gap the B1-B11 tests below never exercised: the
//! Phase-0 guard is only checked at the *top* of a tick. A runtime that
//! already read `RUNNING` before a concurrent halt landed can proceed
//! deeper into that same tick — refreshing its `runtime_leader_lease`,
//! claiming outbox rows, submitting to the broker — without ever
//! re-observing `HALTED`, because the pre-03 `refresh_lease`/`acquire_lease`
//! primitives validate holder/epoch/expiry only, with no notion of which run
//! or its status. A durably `HALTED` run with a still-unexpired lease is
//! therefore NOT proof the prior owner has stopped acting.
//!
//! -03 closes this by adding a **Layer A quiescence** requirement (an
//! unexpired lease refuses `clear-halted-run` with zero mutation — see the
//! SCR03 section below) and by making lease acquire/refresh and outbox
//! claiming **run-aware and transactionally fenced** against the exact same
//! `runs` row lock `clear_halted_run_and_reset_stale_claims` uses. See
//! `mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run` and
//! `mqk_db::outbox_claim_batch_for_run_with_lease_authority` for the
//! mechanism.
//!
//! # B1-B11 test matrix (unchanged claims; some assertions updated for -03's
//! typed `ClearHaltedRunOutcome` return type — see inline comments)
//!
//! | Test | Claim                                                                |
//! |------|-----------------------------------------------------------------------|
//! | B1   | Real production recovery path: crashed RUNNING -> HALTED -> the real  |
//! |      | clear-halted-run HTTP route -> row safely reclaimable -> dispatches   |
//! |      | once via the real outbox_claim_batch_for_run primitive                |
//! | B2   | Live competing leader (run still RUNNING, not halted): refuses via    |
//! |      | StaleRunTransition, does not reset, does not dispatch                 |
//! | B3   | Orphaned leader, no lease row at all (HALTED): takeover succeeds,     |
//! |      | reset once                                                            |
//! | B3E  | Orphaned leader with an EXPIRED lease (HALTED): takeover succeeds,    |
//! |      | reset once, expired lease row cleaned up atomically (§12)             |
//! | B4   | Concurrent takeover attempts (Barrier): exactly one Success           |
//! | B5   | DISPATCHING row is never reset                                        |
//! | B6   | SENT row is never reset                                               |
//! | B7   | AMBIGUOUS row is never reset                                          |
//! | B8   | Stale claim belonging to another run is untouched                     |
//! | B9   | Recovered row produces exactly one broker submission opportunity      |
//! |      | (claim succeeds once, a second claim attempt finds nothing)           |
//! | B10  | Fresh normal start still works (no regression from removing the old   |
//! |      | unconditional reset call)                                             |
//! | B11  | Sticky-HALT fail-closed lifecycle guarantee remains intact (refusal   |
//! |      | is now a typed StaleRunTransition outcome, not an Err)                |
//! | NEG  | Negative control: the OLD unconditional primitive, run in the exact   |
//! |      | B2 scenario, DOES incorrectly reset a live run's claim — proving the  |
//! |      | ownership gate this patch adds is load-bearing, not incidental        |
//!
//! # SCR03 test matrix (new — see bottom of this file)
//!
//! | Test (this file, `mqk-daemon`)                            | Claim         |
//! |------------------------------------------------------------------------|
//! | scr03_neg1_s07_s08_active_lease_survives_halt_and_blocks_clear_with_zero_mutation | NEG-1/S07/S08: an active unexpired lease refuses clear-halted-run with zero mutation |
//! | scr03_neg2_neg6_no_lease_row_race_run_not_running_after_clear | NEG-2/NEG-6/§20: clear wins first (no lease existed); the stale runtime's subsequent run-aware acquire sees STOPPED, not RUNNING, and STOPPED is never rewritten to HALTED |
//! | scr03_neg3_stale_claimant_cannot_aba_dispatch              | NEG-3: A's stale claim cannot transition B's current claim to DISPATCHING |
//! | scr03_neg4_lost_dispatch_cas_is_zero_mutation              | NEG-4: outbox_mark_dispatching returning false leaves the row exactly as it was |
//! | scr03_s09_release_then_clear_succeeds_immediately            | S09: releasing the lease unblocks an immediate successful clear |
//! | scr03_s21_s22_s23_s24_fenced_claim_authority_matrix           | S21-S24: fenced claim succeeds only for exact RUNNING+matching unexpired lease |
//! | scr03_s25_s26_fenced_claim_refuses_halted_and_stopped          | S25/S26: fenced claim refuses on HALTED/STOPPED |
//! | scr03_operator_route_409_on_active_lease                      | §19: the real HTTP clear-halted-run route returns 409 with zero mutation when a lease is active |
//!
//! (S10/crash-expiry is covered by `b3e_halted_orphan_with_expired_lease_takeover_succeeds_and_cleans_up_lease`
//! above, in the B-series.)
//!
//! | Test (`mqk-db::runtime_lease` unit tests)                 | Claim         |
//! |------------------------------------------------------------------------|
//! | run_aware_acquire_succeeds_on_running_run                  | S01 |
//! | run_aware_refresh_succeeds_for_same_holder_epoch_on_running_run | S02 |
//! | run_aware_second_contender_cannot_acquire_active_lease      | S03 |
//! | run_aware_refuses_on_halted_run                             | S05 |
//! | run_aware_refuses_on_stopped_run_and_does_not_touch_lease   | S06 |
//! | run_aware_refresh_lost_on_expired_epoch_while_still_running | genuine lease loss while RUNNING is `Lost`, distinct from `RunNotRunning` |
//!
//! | Test (`mqk-runtime::orchestrator::tests`)                  | Claim         |
//! |------------------------------------------------------------------------|
//! | scr03_stale_runtime_tick_after_clear_is_fenced_and_never_rehalts_stopped | §18/NEG-6 (orchestrator level): a real `ExecutionOrchestrator::tick()` call against a run the operator already cleared to STOPPED is fenced (`RUNTIME_FENCED_RUN_NOT_RUNNING`) and never rewrites STOPPED back to HALTED |
//!
//! All tests use `mqk_db::run_isolated` (a genuinely disposable per-test
//! database), never `MQK_DATABASE_URL` shared state, and never a live broker.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use mqk_daemon::{
    routes::build_router,
    state::{AppState, OperatorAuthMode},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid, engine_id: &str) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "SCR02-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert_run");
}

/// Advance a freshly-inserted run to RUNNING (Created -> Armed -> Running),
/// exactly the production lifecycle transitions.
async fn advance_to_running(pool: &sqlx::PgPool, run_id: Uuid) {
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
}

/// Enqueue one outbox row and claim it, producing a CLAIMED (pre-DISPATCHING)
/// row for `run_id` — the exact "crashed mid-flight" shape this patch
/// targets. Returns the idempotency_key.
async fn seed_claimed_row(pool: &sqlx::PgPool, run_id: Uuid, key: &str) -> String {
    let inserted = mqk_db::outbox_enqueue(
        pool,
        run_id,
        key,
        serde_json::json!({"symbol": "AAPL", "qty": 1, "side": "buy"}),
    )
    .await
    .expect("outbox_enqueue");
    assert!(inserted, "seed_claimed_row: enqueue must insert a new row");

    let claimed =
        mqk_db::outbox_claim_batch_for_run(pool, run_id, 10, "crashed-dispatcher", Utc::now())
            .await
            .expect("outbox_claim_batch_for_run");
    assert_eq!(
        claimed.len(),
        1,
        "seed_claimed_row: exactly one row must be claimed"
    );
    key.to_string()
}

async fn outbox_status(pool: &sqlx::PgPool, key: &str) -> String {
    mqk_db::outbox_fetch_by_idempotency_key(pool, key)
        .await
        .expect("outbox_fetch_by_idempotency_key")
        .expect("row must exist")
        .status
}

// ---------------------------------------------------------------------------
// REACHABILITY: the normal start path can never reach build_execution_
// orchestrator for a crash-orphaned RUNNING run (part of PATCH-01's root
// cause, distinct from the ordering defect)
// ---------------------------------------------------------------------------

/// Proves the second half of the root cause: a durably-`RUNNING` run with no
/// local owner (exactly the shape a crashed process leaves behind) is
/// refused by `create_or_reuse_run_for_start` BEFORE
/// `build_execution_orchestrator` — and therefore before PATCH-01's
/// unconditional `outbox_reset_stale_claims` call — is ever reached. This is
/// why B1 must use the real `clear-halted-run` route rather than directly
/// calling `build_execution_orchestrator`: for this exact scenario, the
/// latter was always dead code.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn reachability_crashed_running_run_blocks_normal_start_before_orchestrator_build() {
    mqk_db::run_isolated("reachability", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        // No halt_run call: this run is durably RUNNING with no local
        // owner in this fresh AppState/process — precisely what a crash
        // leaves behind before any deadman/operator halt has landed.

        let st = Arc::new(AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        ));

        let result = st.create_or_reuse_run_for_start(&pool).await;
        let err = result.expect_err(
            "REACHABILITY: the normal start path must refuse a durable-RUNNING run \
             with no local owner, never silently adopting or bypassing it",
        );
        assert_eq!(
            err.fault_class(),
            "runtime.truth_mismatch.durable_active_without_local_owner",
            "REACHABILITY: refusal must be the durable-active/local-owner truth-mismatch \
             gate, confirming build_execution_orchestrator (and PATCH-01's reset call \
             inside it) is unreachable for this run_id via the normal start path"
        );

        // The run's CLAIMED-adjacent state is untouched by the refused
        // start attempt — nothing here could have reset anything, because
        // nothing reached build_execution_orchestrator at all.
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, mqk_db::RunStatus::Running));
    })
    .await;
}

// ---------------------------------------------------------------------------
// B1 + B9: real production recovery path, reclaimable, dispatches exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b1_b9_real_recovery_path_reclaims_and_dispatches_exactly_once() {
    mqk_db::run_isolated("b1_b9", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b1-order-1").await;

        // Simulate the crash being detected: the process that crashed never
        // called stop_run/halt_run itself (it's dead) — some other
        // mechanism (deadman enforcement or an operator) transitions the
        // durably-RUNNING orphaned run to HALTED. halt_run is that same
        // production transition, called here to set up the precondition.
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");
        assert_eq!(outbox_status(&pool, &key).await, "CLAIMED");

        // B1: the REAL production recovery authority — the clear-halted-run
        // HTTP route — not a direct call to build_execution_orchestrator.
        let st = Arc::new(AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let router = build_router(st);
        let (status, body) = post_action(
            router,
            serde_json::json!({ "action_key": "clear-halted-run" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "B1: real clear-halted-run route must accept; body: {body}"
        );

        // The row is now safely reclaimable.
        assert_eq!(
            outbox_status(&pool, &key).await,
            "PENDING",
            "B1: stale CLAIMED row must be reset to PENDING by the real recovery path"
        );
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run_after.status, mqk_db::RunStatus::Stopped),
            "B1: run must be STOPPED after the real clear-halted-run route"
        );

        // B9: exactly one broker submission opportunity — proven with the
        // real outbox_claim_batch_for_run primitive (the same one production
        // dispatch uses), not a hand-rolled query.
        let first_claim = mqk_db::outbox_claim_batch_for_run(
            &pool,
            run_id,
            10,
            "recovering-dispatcher",
            Utc::now(),
        )
        .await
        .expect("first claim");
        assert_eq!(
            first_claim.len(),
            1,
            "B9: the recovered row must be claimable exactly once"
        );
        assert_eq!(first_claim[0].row.idempotency_key, key);

        let second_claim =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "another-dispatcher", Utc::now())
                .await
                .expect("second claim");
        assert!(
            second_claim.is_empty(),
            "B9: a second claim attempt must find nothing — no duplicate opportunity"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B2: live competing leader (still RUNNING) — fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b2_live_running_run_recovery_fails_closed_and_does_not_reset() {
    mqk_db::run_isolated("b2", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b2-order-1").await;
        // Deliberately NOT halted — this is a live, actively-running owner.

        // PAPER-SOAK-STALE-CLAIM-RECOVERY-03: a refusal because the run is
        // not durably HALTED is a typed `Ok(StaleRunTransition)` outcome, not
        // an `Err` — it is an ordinary, expected disposition (e.g. a race
        // against a concurrent transition), not a failure condition.
        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("B2: DB call itself must not error");
        assert_eq!(
            outcome,
            mqk_db::ClearHaltedRunOutcome::StaleRunTransition {
                actual_status: "RUNNING".to_string()
            },
            "B2: recovery attempt against a live RUNNING run must refuse via StaleRunTransition"
        );

        assert_eq!(
            outbox_status(&pool, &key).await,
            "CLAIMED",
            "B2: CLAIMED row must NOT be reset when the run is not durably HALTED"
        );
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run_after.status, mqk_db::RunStatus::Running),
            "B2: run must remain RUNNING — recovery must not mutate a live run's status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B3: expired/orphaned leader (HALTED) — takeover succeeds, reset once
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b3_halted_orphan_takeover_succeeds_reset_once() {
    mqk_db::run_isolated("b3", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b3-order-1").await;
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");
        // No runtime_leader_lease row exists at all — the "absent lease"
        // half of -03's Layer A quiescence contract (§12/§20).

        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("B3: takeover of a HALTED orphan with no active lease must succeed");
        assert_eq!(
            outcome,
            mqk_db::ClearHaltedRunOutcome::Success {
                stale_claims_reset: 1
            },
            "B3: exactly one row must be reset"
        );
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");
    })
    .await;
}

// ---------------------------------------------------------------------------
// B3E: expired-lease orphan takeover succeeds (the OTHER half of Layer A's
// "no active lease" contract — an unexpired lease refuses, an expired one
// does not).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b3e_halted_orphan_with_expired_lease_takeover_succeeds_and_cleans_up_lease() {
    mqk_db::run_isolated("b3e", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b3e-order-1").await;

        // A crashed runtime's lease that has since expired -- not active
        // authority per §12.
        let t0 = Utc::now();
        let acquired = mqk_db::runtime_lease::acquire_lease(&pool, "crashed-runtime", t0, 5)
            .await
            .expect("acquire_lease");
        assert!(matches!(
            acquired,
            mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(_)
        ));

        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");

        let t_after_expiry = t0 + chrono::Duration::seconds(10);
        let outcome =
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, t_after_expiry)
                .await
                .expect("B3E: takeover after lease expiry must succeed");
        assert_eq!(
            outcome,
            mqk_db::ClearHaltedRunOutcome::Success {
                stale_claims_reset: 1
            },
            "B3E: exactly one row must be reset once the lease has expired"
        );
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");

        let lease_after = mqk_db::runtime_lease::fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease");
        assert!(
            lease_after.is_none(),
            "B3E: the expired lease row must be cleaned up as part of the same \
             atomic recovery transaction (§12)"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B4: concurrent takeover attempts (Barrier) — exactly one wins
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b4_concurrent_takeover_attempts_exactly_one_wins() {
    mqk_db::run_isolated("b4", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b4-order-1").await;
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let pool_a = pool.clone();
        let barrier_a = barrier.clone();
        let task_a = tokio::spawn(async move {
            barrier_a.wait().await;
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool_a, run_id, Utc::now()).await
        });

        let pool_b = pool.clone();
        let barrier_b = barrier.clone();
        let task_b = tokio::spawn(async move {
            barrier_b.wait().await;
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool_b, run_id, Utc::now()).await
        });

        let (result_a, result_b) = tokio::join!(task_a, task_b);
        let result_a = result_a
            .expect("task_a must not panic")
            .expect("task_a DB call must not error");
        let result_b = result_b
            .expect("task_b must not panic")
            .expect("task_b DB call must not error");

        // PAPER-SOAK-STALE-CLAIM-RECOVERY-03: both calls return `Ok` now (a
        // losing CAS is a typed `StaleRunTransition`, not an `Err`) — the
        // race-winner proof is "exactly one Success", not "exactly one Ok".
        let success_count = [&result_a, &result_b]
            .iter()
            .filter(|r| matches!(r, mqk_db::ClearHaltedRunOutcome::Success { .. }))
            .count();
        assert_eq!(
            success_count, 1,
            "B4: exactly one concurrent takeover attempt must win \
             (a={:?}, b={:?})",
            result_a, result_b
        );

        // The row must have been reset exactly once — not corrupted, not
        // double-reset, not left CLAIMED.
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, mqk_db::RunStatus::Stopped));
    })
    .await;
}

// ---------------------------------------------------------------------------
// B5/B6/B7: DISPATCHING / SENT / AMBIGUOUS rows are never reset
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b5_dispatching_row_is_never_reset() {
    mqk_db::run_isolated("b5", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b5-order-1").await;
        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("outbox_fetch_by_idempotency_key")
            .expect("row must exist");
        let advanced = mqk_db::outbox_mark_dispatching(
            &pool,
            row.outbox_id,
            &key,
            "crashed-dispatcher",
            "attempt-1",
            Utc::now(),
        )
        .await
        .expect("outbox_mark_dispatching");
        assert!(advanced, "precondition: row must reach DISPATCHING");
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("clear must still succeed (run is HALTED)");
        let reset_count = match outcome {
            mqk_db::ClearHaltedRunOutcome::Success { stale_claims_reset } => stale_claims_reset,
            other => panic!("B5: expected Success, got {other:?}"),
        };
        assert_eq!(
            reset_count, 0,
            "B5: a DISPATCHING row must never be counted/reset"
        );
        assert_eq!(
            outbox_status(&pool, &key).await,
            "DISPATCHING",
            "B5: DISPATCHING row must remain DISPATCHING — may have reached the broker"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b6_sent_row_is_never_reset() {
    mqk_db::run_isolated("b6", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b6-order-1").await;
        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("outbox_fetch_by_idempotency_key")
            .expect("row must exist");
        mqk_db::outbox_mark_dispatching(
            &pool,
            row.outbox_id,
            &key,
            "crashed-dispatcher",
            "attempt-1",
            Utc::now(),
        )
        .await
        .expect("outbox_mark_dispatching");
        let sent =
            mqk_db::outbox_mark_sent_with_broker_map(&pool, &key, "broker-order-1", Utc::now())
                .await
                .expect("outbox_mark_sent_with_broker_map");
        assert!(sent, "precondition: row must reach SENT");
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("clear must still succeed (run is HALTED)");
        let reset_count = match outcome {
            mqk_db::ClearHaltedRunOutcome::Success { stale_claims_reset } => stale_claims_reset,
            other => panic!("B6: expected Success, got {other:?}"),
        };
        assert_eq!(reset_count, 0, "B6: a SENT row must never be counted/reset");
        assert_eq!(
            outbox_status(&pool, &key).await,
            "SENT",
            "B6: SENT row must remain SENT — order reached the broker"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b7_ambiguous_row_is_never_reset() {
    mqk_db::run_isolated("b7", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b7-order-1").await;
        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("outbox_fetch_by_idempotency_key")
            .expect("row must exist");
        mqk_db::outbox_mark_dispatching(
            &pool,
            row.outbox_id,
            &key,
            "crashed-dispatcher",
            "attempt-1",
            Utc::now(),
        )
        .await
        .expect("outbox_mark_dispatching");
        let ambiguous = mqk_db::outbox_mark_ambiguous(&pool, &key)
            .await
            .expect("outbox_mark_ambiguous");
        assert!(ambiguous, "precondition: row must reach AMBIGUOUS");
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("clear must still succeed (run is HALTED)");
        let reset_count = match outcome {
            mqk_db::ClearHaltedRunOutcome::Success { stale_claims_reset } => stale_claims_reset,
            other => panic!("B7: expected Success, got {other:?}"),
        };
        assert_eq!(
            reset_count, 0,
            "B7: an AMBIGUOUS row must never be counted/reset"
        );
        assert_eq!(
            outbox_status(&pool, &key).await,
            "AMBIGUOUS",
            "B7: AMBIGUOUS row must remain quarantined — only \
             outbox_reset_ambiguous_to_pending may release it"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B8: a stale claim belonging to a different run is untouched
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b8_stale_claim_on_a_different_run_is_untouched() {
    mqk_db::run_isolated("b8", |pool| async move {
        let recovering_run = Uuid::new_v4();
        let other_run = Uuid::new_v4();
        seed_run(&pool, recovering_run, "mqk-daemon").await;
        seed_run(&pool, other_run, "mqk-daemon").await;
        advance_to_running(&pool, recovering_run).await;
        advance_to_running(&pool, other_run).await;

        let recovering_key = seed_claimed_row(&pool, recovering_run, "b8-recovering-order").await;
        let other_key = seed_claimed_row(&pool, other_run, "b8-other-order").await;

        // Only the recovering run is halted; the other run's CLAIMED row
        // belongs to a run that is (from this test's perspective) still
        // live/unrelated and must never be touched by this run's recovery.
        mqk_db::halt_run(&pool, recovering_run, Utc::now())
            .await
            .expect("halt_run");

        let outcome =
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool, recovering_run, Utc::now())
                .await
                .expect("recovering run's clear must succeed");
        assert!(
            matches!(outcome, mqk_db::ClearHaltedRunOutcome::Success { .. }),
            "B8: expected Success, got {outcome:?}"
        );

        assert_eq!(outbox_status(&pool, &recovering_key).await, "PENDING");
        assert_eq!(
            outbox_status(&pool, &other_key).await,
            "CLAIMED",
            "B8: another run's stale claim must be untouched by this run's recovery"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B10: fresh normal start still works
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b10_fresh_normal_start_unaffected_by_removed_reset_call() {
    mqk_db::run_isolated("b10", |pool| async move {
        // A brand-new run with no outbox activity at all — proves removing
        // the unconditional outbox_reset_stale_claims call from
        // build_execution_orchestrator does not disturb the ordinary case
        // (which never had a CLAIMED row to begin with).
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;

        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run.status, mqk_db::RunStatus::Created));

        // No outbox rows exist yet; a claim attempt must simply find nothing
        // — no error, no residue from a removed reset call.
        let claimed =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "fresh-dispatcher", Utc::now())
                .await
                .expect("outbox_claim_batch_for_run on a fresh run must not error");
        assert!(claimed.is_empty());
    })
    .await;
}

// ---------------------------------------------------------------------------
// B11: sticky-HALT fail-closed lifecycle guarantee remains intact
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b11_halt_remains_sticky_and_a_second_clear_attempt_is_refused() {
    mqk_db::run_isolated("b11", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "b11-order-1").await;
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        // First clear succeeds and consumes the HALTED state.
        let first = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("first clear must succeed");
        assert!(matches!(
            first,
            mqk_db::ClearHaltedRunOutcome::Success { .. }
        ));
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");

        // A second clear attempt on the now-STOPPED run must be refused —
        // the CAS guard is not a one-shot bypass of the lifecycle contract;
        // clear-halted-run remains exactly as sticky/idempotent-safe as the
        // pre-existing clear_halted_run always was. Under -03 this is a
        // typed `StaleRunTransition` outcome, not an `Err`.
        let second = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("second call's DB round trip must not error");
        assert_eq!(
            second,
            mqk_db::ClearHaltedRunOutcome::StaleRunTransition {
                actual_status: "STOPPED".to_string()
            },
            "B11: a second clear attempt on a non-HALTED run must refuse via StaleRunTransition"
        );
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, mqk_db::RunStatus::Stopped));
    })
    .await;
}

// ---------------------------------------------------------------------------
// NEG: negative control — the OLD primitive is unsafe in the exact B2 scenario
// ---------------------------------------------------------------------------

/// Negative control proving the fix is load-bearing.
///
/// This is the OLD, still-present `outbox_reset_stale_claims` primitive
/// (unchanged — it is a correct, tested building block; what was wrong was
/// calling it unconditionally with no ownership proof), invoked exactly the
/// way `PAPER-SOAK-STALE-CLAIM-RECOVERY-01` called it: with `stale_threshold
/// = Utc::now()` and no check on the run's status. Run against the identical
/// precondition as B2 (a genuinely live, still-RUNNING run with a CLAIMED
/// row that a real dispatcher may be about to move to DISPATCHING), it DOES
/// incorrectly reset the row — because `claimed_at_utc < now()` is true for
/// literally any already-claimed row, and the old call site had no ownership
/// gate at all. This is the exact defect PATCH-01's own comment denied
/// ("the runtime leadership lease this run start already went through
/// guarantees no other legitimate dispatcher is concurrently active").
///
/// Contrast with `b2_live_running_run_recovery_fails_closed_and_does_not_reset`
/// above: the NEW `clear_halted_run_and_reset_stale_claims`, given the exact
/// same precondition, correctly refuses. The only difference between "unsafe"
/// and "safe" here is the `WHERE status = 'HALTED'` ownership gate this patch
/// adds — proving that gate, not incidental test setup, is what makes B2 pass.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn neg_old_primitive_incorrectly_resets_a_live_runs_claim() {
    mqk_db::run_isolated("neg", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "neg-order-1").await;
        // Deliberately NOT halted — identical precondition to B2: a live,
        // actively-running owner that may be mid-flight on this exact row.

        // The OLD call shape: no ownership proof, threshold = now().
        let reset_count = mqk_db::outbox_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("outbox_reset_stale_claims");

        assert_eq!(
            reset_count, 1,
            "NEG: the old unconditional primitive DOES reset a live run's CLAIMED row \
             — this is the double-submission hazard PATCH-01 left open, and exactly \
             what B2 proves the new ownership-gated function refuses to do"
        );
        assert_eq!(
            outbox_status(&pool, &key).await,
            "PENDING",
            "NEG: the row was incorrectly freed for a second dispatcher to claim \
             while the original live owner may still be mid-flight on it"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// SCR03: PAPER-SOAK-STALE-CLAIM-RECOVERY-03 test matrix
// ---------------------------------------------------------------------------

/// NEG-1 / S07 / S08: an active, unexpired runtime leader lease survives a
/// halt and blocks `clear-halted-run` with zero mutation.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_neg1_s07_s08_active_lease_survives_halt_and_blocks_clear_with_zero_mutation() {
    mqk_db::run_isolated("scr03_neg1", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "scr03-neg1-order-1").await;

        // Runtime A: valid, unexpired lease acquired via the real run-aware
        // production entrypoint — models "A has already passed Phase 0's
        // RUNNING read and holds durable dispatch authority".
        let t0 = Utc::now();
        let lease_outcome = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            t0,
            300,
            300,
        )
        .await
        .expect("acquire");
        let (holder_id, epoch) = match lease_outcome {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(lease) => {
                (lease.holder_id, lease.epoch)
            }
            other => panic!("expected Acquired, got {other:?}"),
        };

        // Operator halts the run while A's lease is still valid.
        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");

        // First clear attempt: MUST refuse.
        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, t0)
            .await
            .expect("DB call must not error");
        match &outcome {
            mqk_db::ClearHaltedRunOutcome::ActiveRuntimeLease {
                holder_id: h,
                epoch: e,
                ..
            } => {
                assert_eq!(h, &holder_id);
                assert_eq!(*e, epoch);
            }
            other => panic!("NEG-1: expected ActiveRuntimeLease, got {other:?}"),
        }

        // S08: zero mutation proof — HALTED unchanged, CLAIMED unchanged,
        // lease unchanged.
        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, mqk_db::RunStatus::Halted));
        assert_eq!(outbox_status(&pool, &key).await, "CLAIMED");
        let lease_after = mqk_db::runtime_lease::fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease")
            .expect("lease row must still exist");
        assert_eq!(lease_after.holder_id, holder_id);
        assert_eq!(lease_after.epoch, epoch);
    })
    .await;
}

/// NEG-2 / §20: the absent-lease-row race. A observes RUNNING before
/// acquiring a lease; the operator's clear wins first (no lease exists to
/// block it); A's subsequent attempt to acquire dispatch authority must be
/// refused because the run is no longer RUNNING — and the run must remain
/// STOPPED, never rewritten back to HALTED (NEG-6's core claim, proven at
/// the DB-authority level here; see also the orchestrator-level proof in
/// `mqk-runtime`'s `scr03_stale_runtime_tick_after_clear_is_fenced_and_never_rehalts_stopped`).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_neg2_neg6_no_lease_row_race_run_not_running_after_clear() {
    mqk_db::run_isolated("scr03_neg2", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        // Runtime A observed RUNNING at Phase 0 but has NOT yet acquired a
        // lease -- no runtime_leader_lease row exists at all.

        let t0 = Utc::now();
        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");
        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, t0)
            .await
            .expect("clear with no active lease must succeed");
        assert!(matches!(
            outcome,
            mqk_db::ClearHaltedRunOutcome::Success { .. }
        ));

        // A now attempts to acquire dispatch authority for the run it still
        // believes is RUNNING.
        let outcome = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            t0,
            300,
            300,
        )
        .await
        .expect("acquire attempt must not error");
        assert_eq!(
            outcome,
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            },
            "NEG-2: a runtime that observed RUNNING before clear but never acquired a lease \
             must be refused after clear — it must not acquire authority"
        );
        let lease = mqk_db::runtime_lease::fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease");
        assert!(
            lease.is_none(),
            "NEG-2: the refused acquire attempt must not create a lease row"
        );

        // NEG-6: the run must still be STOPPED, never rewritten to HALTED by
        // A's refused authority check.
        let run_final = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(run_final.status, mqk_db::RunStatus::Stopped),
            "NEG-6: a refused authority check must never re-halt a STOPPED run, got {:?}",
            run_final.status
        );
    })
    .await;
}

/// NEG-3: stale claimant ABA. A's stranded claim is legitimately reset by
/// recovery; B later claims the same row; A's stale in-memory claim must not
/// be able to transition B's current claim to DISPATCHING.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_neg3_stale_claimant_cannot_aba_dispatch() {
    mqk_db::run_isolated("scr03_neg3", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        // claimed_by = "crashed-dispatcher" (see seed_claimed_row).
        let key = seed_claimed_row(&pool, run_id, "scr03-neg3-order-1").await;
        let original_outbox_id = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("fetch")
            .expect("row")
            .outbox_id;

        // Legitimate recovery resets the row.
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");
        mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, Utc::now())
            .await
            .expect("clear must succeed");
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");

        // A new legitimate dispatcher B claims it.
        let claimed_b =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "runtime-b", Utc::now())
                .await
                .expect("B claims");
        assert_eq!(claimed_b.len(), 1);
        assert_eq!(claimed_b[0].row.outbox_id, original_outbox_id);
        assert_eq!(claimed_b[0].row.claimed_by.as_deref(), Some("runtime-b"));

        // Stale A, still believing it owns this exact row, attempts to
        // advance it to DISPATCHING using its own (now-stale) claim_owner.
        let stale_attempt = mqk_db::outbox_mark_dispatching(
            &pool,
            original_outbox_id,
            &key,
            "crashed-dispatcher", // A's stale claim_owner
            "crashed-dispatcher-attempt-2",
            Utc::now(),
        )
        .await
        .expect("CAS call must not error");
        assert!(
            !stale_attempt,
            "NEG-3: stale claimant A must NOT be able to transition B's current claim \
             to DISPATCHING"
        );
        assert_eq!(
            outbox_status(&pool, &key).await,
            "CLAIMED",
            "NEG-3: the row must remain CLAIMED (by B), untouched by A's failed CAS"
        );

        // B, the true current claimant, CAN advance it.
        let b_attempt = mqk_db::outbox_mark_dispatching(
            &pool,
            original_outbox_id,
            &key,
            "runtime-b",
            "runtime-b-attempt-1",
            Utc::now(),
        )
        .await
        .expect("B's CAS call must not error");
        assert!(b_attempt, "NEG-3: the true current claimant B must succeed");
        assert_eq!(outbox_status(&pool, &key).await, "DISPATCHING");
    })
    .await;
}

/// NEG-4: a lost `outbox_mark_dispatching` CAS is a hard zero-mutation
/// refusal — the row's status/claim/dispatch fields are byte-for-byte
/// unchanged.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_neg4_lost_dispatch_cas_is_zero_mutation() {
    mqk_db::run_isolated("scr03_neg4", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "scr03-neg4-order-1").await;
        let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("fetch")
            .expect("row");

        let result = mqk_db::outbox_mark_dispatching(
            &pool,
            row.outbox_id,
            &key,
            "wrong-claim-owner",
            "attempt-1",
            Utc::now(),
        )
        .await
        .expect("CAS call must not error");
        assert!(!result, "NEG-4: CAS with the wrong claim owner must fail");

        let after = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
            .await
            .expect("fetch")
            .expect("row");
        assert_eq!(after.status, "CLAIMED", "NEG-4: status must be unchanged");
        assert_eq!(
            after.claimed_by, row.claimed_by,
            "NEG-4: claimed_by must be unchanged"
        );
        assert!(
            after.dispatching_at_utc.is_none(),
            "NEG-4: dispatching_at_utc must remain null on a failed CAS"
        );
        assert!(
            after.dispatch_attempt_id.is_none(),
            "NEG-4: dispatch_attempt_id must remain null on a failed CAS"
        );
    })
    .await;
}

/// S09: once the local runtime releases its lease (ordinary graceful
/// shutdown after observing HALTED), clear-halted-run succeeds immediately
/// — no need to wait for expiry.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_s09_release_then_clear_succeeds_immediately() {
    mqk_db::run_isolated("scr03_s09", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "scr03-s09-order-1").await;

        let t0 = Utc::now();
        let lease = match mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            t0,
            300,
            300,
        )
        .await
        .expect("acquire")
        {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(lease) => lease,
            other => panic!("expected Acquired, got {other:?}"),
        };

        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");

        let refused = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, t0)
            .await
            .expect("call must not error");
        assert!(matches!(
            refused,
            mqk_db::ClearHaltedRunOutcome::ActiveRuntimeLease { .. }
        ));

        // The local runtime observes HALTED, exits, and releases its lease.
        mqk_db::runtime_lease::release_lease(&pool, &lease.holder_id, lease.epoch)
            .await
            .expect("release_lease");

        // Clear now succeeds immediately.
        let outcome = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id, t0)
            .await
            .expect("clear must succeed after release");
        assert!(matches!(
            outcome,
            mqk_db::ClearHaltedRunOutcome::Success {
                stale_claims_reset: 1
            }
        ));
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");
    })
    .await;
}

/// S21-S24: the fenced production claim (`outbox_claim_batch_for_run_with_lease_authority`)
/// succeeds only for the exact RUNNING run + matching unexpired lease, and
/// refuses on wrong epoch, wrong holder, or expired lease.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_s21_s22_s23_s24_fenced_claim_authority_matrix() {
    mqk_db::run_isolated("scr03_s21", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        mqk_db::outbox_enqueue(
            &pool,
            run_id,
            "scr03-s21-order-1",
            serde_json::json!({"symbol": "AAPL", "qty": 1, "side": "buy"}),
        )
        .await
        .expect("outbox_enqueue");

        let t0 = Utc::now();
        let lease = match mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            t0,
            30,
            30,
        )
        .await
        .expect("acquire")
        {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(lease) => lease,
            other => panic!("expected Acquired, got {other:?}"),
        };

        // S22: wrong epoch fails.
        let wrong_epoch = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-a",
            lease.epoch + 1,
            10,
            "runtime-a",
            t0,
        )
        .await
        .expect("call must not error");
        assert_eq!(wrong_epoch, mqk_db::FencedClaimOutcome::LeaseInvalid);

        // S23: wrong holder fails.
        let wrong_holder = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-x",
            lease.epoch,
            10,
            "runtime-x",
            t0,
        )
        .await
        .expect("call must not error");
        assert_eq!(wrong_holder, mqk_db::FencedClaimOutcome::LeaseInvalid);

        // S24: expired lease fails.
        let past_expiry = lease.lease_expires_at + chrono::Duration::seconds(1);
        let expired = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-a",
            lease.epoch,
            10,
            "runtime-a",
            past_expiry,
        )
        .await
        .expect("call must not error");
        assert_eq!(expired, mqk_db::FencedClaimOutcome::LeaseInvalid);

        // S21: exact RUNNING + matching unexpired lease succeeds.
        let ok = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-a",
            lease.epoch,
            10,
            "runtime-a",
            t0,
        )
        .await
        .expect("call must not error");
        match ok {
            mqk_db::FencedClaimOutcome::Claimed(rows) => {
                assert_eq!(
                    rows.len(),
                    1,
                    "S21: exactly one PENDING row must be claimed"
                );
            }
            other => panic!("S21: expected Claimed, got {other:?}"),
        }
    })
    .await;
}

/// S25/S26: the fenced production claim refuses on HALTED and STOPPED.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_s25_s26_fenced_claim_refuses_halted_and_stopped() {
    mqk_db::run_isolated("scr03_s25", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;

        let t0 = Utc::now();
        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");

        let halted = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-a",
            1,
            10,
            "runtime-a",
            t0,
        )
        .await
        .expect("call must not error");
        assert_eq!(
            halted,
            mqk_db::FencedClaimOutcome::RunNotRunning {
                actual_status: "HALTED".to_string()
            }
        );

        mqk_db::clear_halted_run(&pool, run_id)
            .await
            .expect("clear_halted_run");

        let stopped = mqk_db::outbox_claim_batch_for_run_with_lease_authority(
            &pool,
            run_id,
            "runtime-a",
            1,
            10,
            "runtime-a",
            t0,
        )
        .await
        .expect("call must not error");
        assert_eq!(
            stopped,
            mqk_db::FencedClaimOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            }
        );
    })
    .await;
}

/// §19: the real HTTP `clear-halted-run` operator route returns a
/// deterministic conflict response with zero mutation when an active
/// runtime lease blocks recovery.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn scr03_operator_route_409_on_active_lease() {
    mqk_db::run_isolated("scr03_route", |pool| async move {
        let run_id = Uuid::new_v4();
        seed_run(&pool, run_id, "mqk-daemon").await;
        advance_to_running(&pool, run_id).await;
        let key = seed_claimed_row(&pool, run_id, "scr03-route-order-1").await;

        let t0 = Utc::now();
        mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            t0,
            300,
            300,
        )
        .await
        .expect("acquire");
        mqk_db::halt_run(&pool, run_id, t0).await.expect("halt_run");

        let st = Arc::new(AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let router = build_router(st);
        let (status, body) = post_action(
            router,
            serde_json::json!({ "action_key": "clear-halted-run" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "operator route must return 409 when an active runtime lease blocks \
             recovery; body: {body}"
        );
        assert_eq!(
            body.get("disposition").and_then(|v| v.as_str()),
            Some("active_runtime_lease")
        );

        let run_after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(matches!(run_after.status, mqk_db::RunStatus::Halted));
        assert_eq!(outbox_status(&pool, &key).await, "CLAIMED");
    })
    .await;
}
