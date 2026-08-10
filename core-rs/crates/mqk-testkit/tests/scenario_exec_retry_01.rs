//! Scenario: Bounded Dispatch Retry — EXEC-RETRY-01
//!
//! # Invariants under test
//!
//! These tests prove the full EXEC-RETRY-01 contract against a live Postgres DB:
//!
//! ## R-01 — First retry: WillRetry, count incremented, backoff written
//!
//! After outbox_record_retry on a DISPATCHING row:
//!   - Returns WillRetry { attempt: 1 }
//!   - Row status is PENDING
//!   - dispatch_attempt_count is 1
//!   - next_dispatch_after_utc is in the future
//!
//! ## R-02 — Backoff gate: claim batch skips backed-off row
//!
//! outbox_claim_batch with claimed_at = now does NOT return a row whose
//! next_dispatch_after_utc is in the future.  The row is invisible to the
//! dispatcher until its window expires.
//!
//! ## R-03 — Backoff expiry: claim batch returns row after window
//!
//! outbox_claim_batch with claimed_at = next_dispatch_after_utc + 1s returns
//! the row.  Once the window expires the dispatcher can re-claim it.
//!
//! ## R-04 — Attempt ceiling: ExhaustedFailed after MAX_DISPATCH_ATTEMPTS
//!
//! Successive calls: WillRetry{1}, WillRetry{2}, ExhaustedFailed{3}.
//! On ExhaustedFailed the row is FAILED — permanently quarantined.
//!
//! ## R-05 — Non-retry reset path regression
//!
//! outbox_reset_dispatching_to_pending still works for paths that do not use
//! the retry helper (e.g., crash-recovery reaper).  The row returns to PENDING
//! with dispatch_attempt_count unchanged.
//!
//! # PROOF LANE
//!
//! DB-backed tests skip gracefully when MQK_DATABASE_URL is absent.
//! Set MQK_DATABASE_URL to a CLEAN Postgres instance (no prior migration
//! checksum) to run the full proof.
//! If these tests skip, EXEC-RETRY-01 is PARTIAL, not CLOSED.

use chrono::{Duration, Utc};
use mqk_db::{RetryDispatchOutcome, MAX_DISPATCH_ATTEMPTS};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Unique run UUIDs per invocation — avoids UNIQUE conflicts across test reruns.
// Each helper generates a fresh run ID so tests are safe to rerun against a
// persistent DB without cleanup.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

fn db_url() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

async fn connect(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("EXEC-RETRY-01: cannot connect to DB: {e}"))
}

async fn apply_migrations(pool: &PgPool) {
    mqk_db::migrate(pool)
        .await
        .unwrap_or_else(|e| panic!("EXEC-RETRY-01: migration failed: {e}"));
}

async fn seed(pool: &PgPool, run_id: Uuid, idem_key: &str) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "exec-retry-01-test".into(),
            mode: "PAPER".into(),
            started_at_utc: Utc::now(),
            git_hash: "exec-retry-01".into(),
            config_hash: "exec-retry-01".into(),
            config_json: json!({}),
            host_fingerprint: "exec-retry-01".into(),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("seed run {run_id}: {e}"));

    mqk_db::outbox_enqueue(pool, run_id, idem_key, json!({"symbol":"SPY","qty":1}))
        .await
        .unwrap_or_else(|e| panic!("seed outbox {idem_key}: {e}"));
}

/// Claim a batch and advance one row to DISPATCHING.  Returns the idempotency
/// key of the claimed row (panics if no row was available).
async fn claim_and_dispatch(pool: &PgPool, run_id: Uuid, dispatcher_id: &str) -> String {
    let now = Utc::now();
    let claimed = mqk_db::outbox_claim_batch_for_run(pool, run_id, 1, dispatcher_id, now)
        .await
        .expect("claim_batch");
    assert_eq!(
        claimed.len(),
        1,
        "expected exactly one claimable PENDING row"
    );
    let row = &claimed[0].row;
    let idem_key = row.idempotency_key.clone();
    let outbox_id = row.outbox_id;
    mqk_db::outbox_mark_dispatching(
        pool,
        outbox_id,
        &idem_key,
        dispatcher_id,
        dispatcher_id,
        now,
    )
    .await
    .expect("mark_dispatching");
    idem_key
}

// ---------------------------------------------------------------------------
// R-01: first retry — WillRetry, count=1, backoff set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r01_first_retry_increments_count_and_sets_backoff() {
    let Some(url) = db_url() else {
        eprintln!("EXEC-RETRY-01/R-01: MQK_DATABASE_URL not set — skipping");
        return;
    };
    let pool = connect(&url).await;
    apply_migrations(&pool).await;

    let run_id = Uuid::new_v4();
    let idem_key = format!("exec-retry-01-r01-{}", Uuid::new_v4());

    seed(&pool, run_id, &idem_key).await;
    claim_and_dispatch(&pool, run_id, "test-dispatcher-r01").await;

    let now = Utc::now();
    let outcome = mqk_db::outbox_record_retry(&pool, &idem_key, "transport error", now)
        .await
        .expect("outbox_record_retry");

    assert_eq!(
        outcome,
        RetryDispatchOutcome::WillRetry { attempt: 1 },
        "first retry must return WillRetry {{ attempt: 1 }}"
    );

    // Verify DB state directly.
    let row: (String, i32, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "select status, dispatch_attempt_count, next_dispatch_after_utc \
         from oms_outbox where idempotency_key = $1",
    )
    .bind(&idem_key)
    .fetch_one(&pool)
    .await
    .expect("fetch row");

    assert_eq!(row.0, "PENDING", "row must be PENDING after first retry");
    assert_eq!(row.1, 1, "dispatch_attempt_count must be 1");
    let backoff = row.2.expect("next_dispatch_after_utc must be set");
    assert!(
        backoff > now,
        "backoff timestamp must be in the future (got {backoff}, now {now})"
    );
}

// ---------------------------------------------------------------------------
// R-02: backoff gate — claim batch skips row with active backoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r02_claim_skips_row_within_backoff_window() {
    let Some(url) = db_url() else {
        eprintln!("EXEC-RETRY-01/R-02: MQK_DATABASE_URL not set — skipping");
        return;
    };
    let pool = connect(&url).await;
    apply_migrations(&pool).await;

    let run_id = Uuid::new_v4();
    let idem_key = format!("exec-retry-01-r02-{}", Uuid::new_v4());

    seed(&pool, run_id, &idem_key).await;
    claim_and_dispatch(&pool, run_id, "test-dispatcher-r02").await;

    let now = Utc::now();
    mqk_db::outbox_record_retry(&pool, &idem_key, "rate limit", now)
        .await
        .expect("outbox_record_retry");

    // claimed_at = now → backoff is now + 30s, so the row must NOT be returned.
    let claimed = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "test-dispatcher-r02", now)
        .await
        .expect("claim_batch");

    assert!(
        claimed.is_empty(),
        "claim batch must not return row within active backoff window (got {} rows)",
        claimed.len()
    );
}

// ---------------------------------------------------------------------------
// R-03: backoff expiry — claim batch returns row after window elapses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r03_claim_returns_row_after_backoff_expires() {
    let Some(url) = db_url() else {
        eprintln!("EXEC-RETRY-01/R-03: MQK_DATABASE_URL not set — skipping");
        return;
    };
    let pool = connect(&url).await;
    apply_migrations(&pool).await;

    let run_id = Uuid::new_v4();
    let idem_key = format!("exec-retry-01-r03-{}", Uuid::new_v4());

    seed(&pool, run_id, &idem_key).await;
    claim_and_dispatch(&pool, run_id, "test-dispatcher-r03").await;

    let now = Utc::now();
    mqk_db::outbox_record_retry(&pool, &idem_key, "transport error", now)
        .await
        .expect("outbox_record_retry");

    // Simulate time advancing past the backoff window (now + 31s).
    let after_backoff = now + Duration::seconds(31);
    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "test-dispatcher-r03", after_backoff)
            .await
            .expect("claim_batch after backoff");

    assert_eq!(
        claimed.len(),
        1,
        "claim batch must return exactly one row after backoff expires"
    );
    assert_eq!(
        claimed[0].row.idempotency_key, idem_key,
        "returned row must be the backed-off row"
    );
}

// ---------------------------------------------------------------------------
// R-04: attempt ceiling — ExhaustedFailed after MAX_DISPATCH_ATTEMPTS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r04_exhausted_failed_after_max_attempts() {
    let Some(url) = db_url() else {
        eprintln!("EXEC-RETRY-01/R-04: MQK_DATABASE_URL not set — skipping");
        return;
    };
    let pool = connect(&url).await;
    apply_migrations(&pool).await;

    let run_id = Uuid::new_v4();
    let idem_key = format!("exec-retry-01-r04-{}", Uuid::new_v4());
    seed(&pool, run_id, &idem_key).await;

    // --- cycle 1: claim → dispatch → retry → WillRetry{1} ---
    let t0 = Utc::now();
    let claimed = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "disp-r04", t0)
        .await
        .expect("claim 1");
    assert_eq!(claimed.len(), 1, "must claim row on attempt 1");
    assert_eq!(claimed[0].row.idempotency_key, idem_key);
    mqk_db::outbox_mark_dispatching(
        &pool,
        claimed[0].row.outbox_id,
        &idem_key,
        "disp-r04",
        "disp-r04",
        t0,
    )
    .await
    .expect("dispatch 1");

    let o1 = mqk_db::outbox_record_retry(&pool, &idem_key, "err1", t0)
        .await
        .expect("retry 1");
    assert_eq!(
        o1,
        RetryDispatchOutcome::WillRetry { attempt: 1 },
        "attempt 1 must be WillRetry"
    );

    // Verify: row is PENDING, backoff active → claim at t0 returns nothing.
    let skipped = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "disp-r04", t0)
        .await
        .expect("claim at t0 (should skip)");
    assert!(skipped.is_empty(), "row must be invisible within backoff");

    // --- cycle 2: re-claim after backoff → dispatch → retry → WillRetry{2} ---
    // MAX_DISPATCH_ATTEMPTS = 3, so attempt 2 is still below the ceiling.
    let t1 = t0 + Duration::seconds(31); // past the 30s backoff
    let claimed2 = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "disp-r04", t1)
        .await
        .expect("claim 2");
    assert_eq!(claimed2.len(), 1, "must re-claim after backoff expires");
    assert_eq!(claimed2[0].row.idempotency_key, idem_key);
    mqk_db::outbox_mark_dispatching(
        &pool,
        claimed2[0].row.outbox_id,
        &idem_key,
        "disp-r04",
        "disp-r04",
        t1,
    )
    .await
    .expect("dispatch 2");

    let o2 = mqk_db::outbox_record_retry(&pool, &idem_key, "err2", t1)
        .await
        .expect("retry 2");
    assert_eq!(
        o2,
        RetryDispatchOutcome::WillRetry { attempt: 2 },
        "attempt 2 must be WillRetry"
    );

    // --- cycle 3: re-claim → dispatch → retry → ExhaustedFailed{3} ---
    let t2 = t1 + Duration::seconds(31);
    let claimed3 = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "disp-r04", t2)
        .await
        .expect("claim 3");
    assert_eq!(claimed3.len(), 1, "must re-claim for final attempt");
    assert_eq!(claimed3[0].row.idempotency_key, idem_key);
    mqk_db::outbox_mark_dispatching(
        &pool,
        claimed3[0].row.outbox_id,
        &idem_key,
        "disp-r04",
        "disp-r04",
        t2,
    )
    .await
    .expect("dispatch 3");

    let o3 = mqk_db::outbox_record_retry(&pool, &idem_key, "err3", t2)
        .await
        .expect("retry 3");
    assert_eq!(
        o3,
        RetryDispatchOutcome::ExhaustedFailed {
            attempts: MAX_DISPATCH_ATTEMPTS
        },
        "attempt 3 must be ExhaustedFailed"
    );

    // Row must be permanently FAILED.
    let (status,): (String,) =
        sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
            .bind(&idem_key)
            .fetch_one(&pool)
            .await
            .expect("fetch final status");
    assert_eq!(status, "FAILED", "row must be FAILED after ceiling reached");

    // Claim must not return it even after its window would have passed.
    let t3 = t2 + Duration::seconds(31);
    let post_fail = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "disp-r04", t3)
        .await
        .expect("claim after FAILED");
    assert!(
        post_fail.is_empty(),
        "FAILED row must never be re-claimed (got {} rows)",
        post_fail.len()
    );
}

// ---------------------------------------------------------------------------
// R-05: non-retry reset path regression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r05_reset_dispatching_to_pending_unaffected() {
    let Some(url) = db_url() else {
        eprintln!("EXEC-RETRY-01/R-05: MQK_DATABASE_URL not set — skipping");
        return;
    };
    let pool = connect(&url).await;
    apply_migrations(&pool).await;

    let run_id = Uuid::new_v4();
    let idem_key = format!("exec-retry-01-r05-{}", Uuid::new_v4());

    seed(&pool, run_id, &idem_key).await;
    claim_and_dispatch(&pool, run_id, "test-dispatcher-r05").await;

    // Reset via the legacy helper (used by crash-recovery paths, not the retry path).
    let reset = mqk_db::outbox_reset_dispatching_to_pending(&pool, &idem_key)
        .await
        .expect("outbox_reset_dispatching_to_pending");

    assert!(
        reset,
        "outbox_reset_dispatching_to_pending must return true"
    );

    // Row must be PENDING with dispatch_attempt_count still 0.
    let (status, count): (String, i32) = sqlx::query_as(
        "select status, dispatch_attempt_count from oms_outbox where idempotency_key = $1",
    )
    .bind(&idem_key)
    .fetch_one(&pool)
    .await
    .expect("fetch row");

    assert_eq!(status, "PENDING", "row must be PENDING after direct reset");
    assert_eq!(
        count, 0,
        "dispatch_attempt_count must remain 0 after direct reset (non-retry path)"
    );
}
