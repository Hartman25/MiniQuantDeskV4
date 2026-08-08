//! RUN-LIFECYCLE-CAS-01
//!
//! # Problem addressed
//!
//! Every run-lifecycle transition (`arm_run`/`begin_run`/`stop_run`/
//! `clear_halted_run`/`heartbeat_run`) used to read `status` via a separate
//! `fetch_run`, check it in Rust, then issue an unconditional
//! `UPDATE ... WHERE run_id = $1` with no `status` guard at all -- a
//! classic check-then-act race. Two concurrent callers observing the same
//! valid pre-state would both pass their own pre-check and both execute
//! their unconditional `UPDATE`; whichever committed last silently won,
//! with no error to either caller. Concretely: a `stop_run` racing a
//! `halt_run` could silently overwrite `HALTED` back to `STOPPED`,
//! breaking `HALTED`'s required sticky/fail-closed semantics.
//!
//! # Fix under test
//!
//! Every transition's `UPDATE` now includes its own required prior
//! `status` in the `WHERE` clause (`halt_run` is deliberately excluded --
//! it is the unconditional, always-wins override by design). A CAS miss
//! (`rows_affected() == 0`) re-reads the row and returns a typed
//! `mqk_db::StaleRunTransition` instead of silently applying nothing or
//! guessing.
//!
//! # Coverage
//!
//! | Test | Claim                                                              |
//! |------|---------------------------------------------------------------------|
//! | S01  | arm_run on an already-RUNNING row returns a typed conflict           |
//! | S02  | begin_run cannot overwrite a concurrent stop (sequential ordering)   |
//! | S03  | stop_run cannot overwrite a concurrent halt (sequential ordering)    |
//! | S04  | heartbeat_run cannot revive a stopped run                            |
//! | S05  | heartbeat_run cannot revive a halted run                              |
//! | S06  | clear_halted_run is CAS-guarded against a second concurrent clear    |
//! | C01  | Genuine concurrent race: stop vs. halt -- halt always wins the final |
//! |      | committed state regardless of interleaving                           |
//! | C02  | Genuine concurrent race: two simultaneous begin_run calls -- exactly |
//! |      | one wins                                                              |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn require_pool(url: &str) -> Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

async fn make_run(pool: &PgPool, engine: &str) -> Result<Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "cas01-test".to_string(),
            config_hash: "cas01-cfg".to_string(),
            config_json: json!({}),
            host_fingerprint: "cas01-host".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

fn downcast_stale(err: &anyhow::Error) -> &mqk_db::StaleRunTransition {
    err.downcast_ref::<mqk_db::StaleRunTransition>()
        .unwrap_or_else(|| panic!("expected StaleRunTransition, got: {err:#}"))
}

// ---------------------------------------------------------------------------
// S01 — stale-state: arm_run on an already-RUNNING row.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s01_arm_run_on_running_row_returns_typed_conflict() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s01-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;

    let err = mqk_db::arm_run(&pool, run_id)
        .await
        .expect_err("S01: arm_run on a RUNNING row must fail");
    let stale = downcast_stale(&err);
    assert_eq!(stale.run_id, run_id);
    assert_eq!(stale.operation, "arm_run");
    assert_eq!(stale.actual, "RUNNING");

    let row = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(row.status, mqk_db::RunStatus::Running),
        "S01: status must remain RUNNING, not silently reset toward ARMED"
    );

    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// S02 — begin_run cannot overwrite a concurrent stop.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s02_begin_run_cannot_overwrite_a_concurrent_stop() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s02-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    // A concurrent stop lands first (modeled sequentially: happens-before
    // the begin_run attempt below, proving the CAS guard rejects a begin
    // that observed a stale ARMED pre-state).
    mqk_db::stop_run(&pool, run_id).await?;

    let err = mqk_db::begin_run(&pool, run_id)
        .await
        .expect_err("S02: begin_run must refuse once the row has moved to STOPPED");
    let stale = downcast_stale(&err);
    assert_eq!(stale.operation, "begin_run");
    assert_eq!(stale.actual, "STOPPED");

    let row = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(row.status, mqk_db::RunStatus::Stopped),
        "S02: status must remain STOPPED, never silently moved to RUNNING"
    );

    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// S03 — stop_run cannot overwrite a concurrent halt.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s03_stop_run_cannot_overwrite_a_concurrent_halt() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s03-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    // A concurrent halt lands first.
    mqk_db::halt_run(&pool, run_id, Utc::now()).await?;

    let err = mqk_db::stop_run(&pool, run_id)
        .await
        .expect_err("S03: stop_run must refuse once the row has moved to HALTED");
    let stale = downcast_stale(&err);
    assert_eq!(stale.operation, "stop_run");
    assert_eq!(stale.actual, "HALTED");

    let row = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(row.status, mqk_db::RunStatus::Halted),
        "S03: HALTED must remain sticky -- stop_run must never overwrite it with STOPPED"
    );

    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// S04/S05 — heartbeat_run cannot revive a stopped or halted run.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s04_heartbeat_run_cannot_revive_a_stopped_run() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s04-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::heartbeat_run(&pool, run_id, Utc::now()).await?;
    let before = mqk_db::fetch_run(&pool, run_id).await?;
    let heartbeat_before_stop = before.last_heartbeat_utc;

    mqk_db::stop_run(&pool, run_id).await?;

    let stale_heartbeat_at = Utc::now();
    let err = mqk_db::heartbeat_run(&pool, run_id, stale_heartbeat_at)
        .await
        .expect_err("S04: heartbeat_run must refuse on a STOPPED row");
    let stale = downcast_stale(&err);
    assert_eq!(stale.operation, "heartbeat_run");
    assert_eq!(stale.actual, "STOPPED");

    let after = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(
        after.last_heartbeat_utc, heartbeat_before_stop,
        "S04: last_heartbeat_utc must not advance on a stopped run -- \
         a stale heartbeat must never make a dead run look alive"
    );

    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
async fn s05_heartbeat_run_cannot_revive_a_halted_run() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s05-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::halt_run(&pool, run_id, Utc::now()).await?;

    let err = mqk_db::heartbeat_run(&pool, run_id, Utc::now())
        .await
        .expect_err("S05: heartbeat_run must refuse on a HALTED row");
    let stale = downcast_stale(&err);
    assert_eq!(stale.actual, "HALTED");

    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// S06 — clear_halted_run is CAS-guarded against a second concurrent clear.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s06_clear_halted_run_second_call_is_refused() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-s06-{}", Uuid::new_v4())).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::halt_run(&pool, run_id, Utc::now()).await?;

    mqk_db::clear_halted_run(&pool, run_id).await?;
    let err = mqk_db::clear_halted_run(&pool, run_id)
        .await
        .expect_err("S06: a second clear_halted_run call must be refused");
    let stale = downcast_stale(&err);
    assert_eq!(stale.operation, "clear_halted_run");
    assert_eq!(stale.actual, "STOPPED");

    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// C01 — genuine concurrent race: stop vs. halt.
// ---------------------------------------------------------------------------

/// Launches `stop_run` and `halt_run` from two tasks released simultaneously
/// via a `tokio::sync::Barrier` (not a sleep), repeated across several fresh
/// rows to sample both possible lock-acquisition orderings. Regardless of
/// which task's `UPDATE` physically commits first, the final committed
/// status must always be `HALTED` -- proving the sticky-halt invariant
/// holds under real concurrency, not merely under a hand-reasoned
/// sequential ordering.
#[tokio::test]
async fn c01_concurrent_stop_and_halt_halt_always_wins_final_state() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;

    for attempt in 0..8 {
        let run_id = make_run(&pool, &format!("cas01-c01-{attempt}-{}", Uuid::new_v4())).await?;
        mqk_db::arm_run(&pool, run_id).await?;
        mqk_db::begin_run(&pool, run_id).await?;

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let pool_a = pool.clone();
        let barrier_a = barrier.clone();
        let stop_task = tokio::spawn(async move {
            barrier_a.wait().await;
            mqk_db::stop_run(&pool_a, run_id).await
        });

        let pool_b = pool.clone();
        let barrier_b = barrier.clone();
        let halt_task = tokio::spawn(async move {
            barrier_b.wait().await;
            mqk_db::halt_run(&pool_b, run_id, Utc::now()).await
        });

        let (stop_result, halt_result) = tokio::join!(stop_task, halt_task);
        stop_result.expect("stop task must not panic").ok();
        halt_result
            .expect("halt task must not panic")
            .expect("halt_run is unconditional and must always succeed");

        let row = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(row.status, mqk_db::RunStatus::Halted),
            "C01 attempt {attempt}: final state must always be HALTED regardless of race \
             interleaving, got {}",
            row.status.as_str()
        );

        cleanup_run(&pool, run_id).await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// C02 — genuine concurrent race: two simultaneous begin_run calls.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c02_concurrent_begin_run_exactly_one_wins() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run_id = make_run(&pool, &format!("cas01-c02-{}", Uuid::new_v4())).await?;
    mqk_db::arm_run(&pool, run_id).await?;

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

    let pool_a = pool.clone();
    let barrier_a = barrier.clone();
    let task_a = tokio::spawn(async move {
        barrier_a.wait().await;
        mqk_db::begin_run(&pool_a, run_id).await
    });

    let pool_b = pool.clone();
    let barrier_b = barrier.clone();
    let task_b = tokio::spawn(async move {
        barrier_b.wait().await;
        mqk_db::begin_run(&pool_b, run_id).await
    });

    let (result_a, result_b) = tokio::join!(task_a, task_b);
    let ok_a = result_a.expect("task a must not panic").is_ok();
    let ok_b = result_b.expect("task b must not panic").is_ok();

    assert!(
        ok_a != ok_b,
        "C02: exactly one of the two concurrent begin_run calls must succeed (a={ok_a}, b={ok_b})"
    );

    let row = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(matches!(row.status, mqk_db::RunStatus::Running));

    cleanup_run(&pool, run_id).await;
    Ok(())
}
