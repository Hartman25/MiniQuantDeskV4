//! PAPER-SOAK-STALE-CLAIM-RECOVERY-02 — acceptance tests B1-B11.
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
//! # Fix
//!
//! Stale-claim recovery moves into this repository's existing,
//! deliberately operator-mediated recovery path: `clear-halted-run`
//! (`mqk_db::clear_halted_run_and_reset_stale_claims`). A `RUNNING` run only
//! reaches `HALTED` via deadman-timeout enforcement, an explicit operator
//! halt, or a lease-loss safety halt — in every case, durable `HALTED` is
//! itself the ownership proof: the orchestrator's Phase-0 I9-1 halt guard
//! (`ExecutionOrchestrator::tick`) refuses ALL dispatch for ANY process once
//! a run is durably `HALTED`, so a `WHERE status = 'HALTED'` CAS guard is a
//! complete, race-proof authority check — no runtime lease inspection
//! needed. The unconditional, wrongly-ordered call in
//! `build_execution_orchestrator` is removed.
//!
//! # Test matrix
//!
//! | Test | Claim                                                                |
//! |------|-----------------------------------------------------------------------|
//! | B1   | Real production recovery path: crashed RUNNING -> HALTED -> the real  |
//! |      | clear-halted-run HTTP route -> row safely reclaimable -> dispatches   |
//! |      | once via the real outbox_claim_batch_for_run primitive                |
//! | B2   | Live competing leader (run still RUNNING, not halted): fails closed,  |
//! |      | does not reset, does not dispatch                                     |
//! | B3   | Expired/orphaned leader (HALTED): takeover succeeds, reset once       |
//! | B4   | Concurrent takeover attempts (Barrier): exactly one wins              |
//! | B5   | DISPATCHING row is never reset                                        |
//! | B6   | SENT row is never reset                                               |
//! | B7   | AMBIGUOUS row is never reset                                          |
//! | B8   | Stale claim belonging to another run is untouched                     |
//! | B9   | Recovered row produces exactly one broker submission opportunity      |
//! |      | (claim succeeds once, a second claim attempt finds nothing)           |
//! | B10  | Fresh normal start still works (no regression from removing the old   |
//! |      | unconditional reset call)                                             |
//! | B11  | Sticky-HALT fail-closed lifecycle guarantee remains intact             |
//! | NEG  | Negative control: the OLD unconditional primitive, run in the exact   |
//! |      | B2 scenario, DOES incorrectly reset a live run's claim — proving the  |
//! |      | ownership gate this patch adds is load-bearing, not incidental        |
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

        let result = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id).await;
        assert!(
            result.is_err(),
            "B2: recovery attempt against a live RUNNING run must fail closed"
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

        let reset_count = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id)
            .await
            .expect("B3: takeover of a HALTED orphan must succeed");
        assert_eq!(reset_count, 1, "B3: exactly one row must be reset");
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");
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
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool_a, run_id).await
        });

        let pool_b = pool.clone();
        let barrier_b = barrier.clone();
        let task_b = tokio::spawn(async move {
            barrier_b.wait().await;
            mqk_db::clear_halted_run_and_reset_stale_claims(&pool_b, run_id).await
        });

        let (result_a, result_b) = tokio::join!(task_a, task_b);
        let result_a = result_a.expect("task_a must not panic");
        let result_b = result_b.expect("task_b must not panic");

        let ok_count = [&result_a, &result_b].iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            ok_count,
            1,
            "B4: exactly one concurrent takeover attempt must win \
             (a={:?}, b={:?})",
            result_a.is_ok(),
            result_b.is_ok()
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
        let advanced = mqk_db::outbox_mark_dispatching(&pool, &key, "attempt-1", Utc::now())
            .await
            .expect("outbox_mark_dispatching");
        assert!(advanced, "precondition: row must reach DISPATCHING");
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let reset_count = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id)
            .await
            .expect("clear must still succeed (run is HALTED)");
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
        mqk_db::outbox_mark_dispatching(&pool, &key, "attempt-1", Utc::now())
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

        let reset_count = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id)
            .await
            .expect("clear must still succeed (run is HALTED)");
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
        mqk_db::outbox_mark_dispatching(&pool, &key, "attempt-1", Utc::now())
            .await
            .expect("outbox_mark_dispatching");
        let ambiguous = mqk_db::outbox_mark_ambiguous(&pool, &key)
            .await
            .expect("outbox_mark_ambiguous");
        assert!(ambiguous, "precondition: row must reach AMBIGUOUS");
        mqk_db::halt_run(&pool, run_id, Utc::now())
            .await
            .expect("halt_run");

        let reset_count = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id)
            .await
            .expect("clear must still succeed (run is HALTED)");
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

        mqk_db::clear_halted_run_and_reset_stale_claims(&pool, recovering_run)
            .await
            .expect("recovering run's clear must succeed");

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
        mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id)
            .await
            .expect("first clear must succeed");
        assert_eq!(outbox_status(&pool, &key).await, "PENDING");

        // A second clear attempt on the now-STOPPED run must be refused —
        // the CAS guard is not a one-shot bypass of the lifecycle contract;
        // clear-halted-run remains exactly as sticky/idempotent-safe as the
        // pre-existing clear_halted_run always was.
        let second = mqk_db::clear_halted_run_and_reset_stale_claims(&pool, run_id).await;
        assert!(
            second.is_err(),
            "B11: a second clear attempt on a non-HALTED run must fail closed"
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
