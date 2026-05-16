//! DEADMAN-EXPIRED-AFTER-START-01: Focused proof tests.
//!
//! Proves the 120-second `DEADMAN_TTL_SECONDS` constant accommodates the
//! observed ~33-second Alpaca REST `fetch_events` block that caused false
//! deadman halts on a fresh paper run start.
//!
//! Root cause: the initial `orchestrator.tick()` in `start_execution_runtime`
//! blocks while `fetch_events` makes a synchronous HTTP call with no timeout.
//! The heartbeat written before `tick()` becomes stale.  When the execution
//! loop spawns and runs its first pre-tick deadman check (TTL was 5 s), the
//! check sees a 32-second-old heartbeat and halts the run immediately.
//!
//! Fix: (1) DEADMAN_TTL_SECONDS raised to 120 s (> RUNTIME_LEASE_TTL=90 s),
//! (2) fresh heartbeat written after initial tick in `start_execution_runtime`,
//! (3) post-tick silent exit in `loop_runner` made non-silent (halt+disarm).
//!
//! | Test  | Kind       | Proof                                                    |
//! |-------|------------|----------------------------------------------------------|
//! | DAS01 | DB-backed  | 32-second-old heartbeat is NOT expired (TTL=120)         |
//! | DAS02 | DB-backed  | NULL heartbeat is always expired regardless of TTL       |
//! | DAS03 | DB-backed  | 130-second-old heartbeat IS expired (TTL=120)            |
//! | DAS04 | DB-backed  | enforce_deadman_or_halt halts run with 130 s stale hb    |
//! | DAS05 | DB-backed  | 119-second-old heartbeat is NOT expired (boundary proof) |

use chrono::Utc;
use uuid::Uuid;

// -------------------------------------------------------------------------
// Canonical TTL value that the daemon uses at runtime.
// If the daemon constant changes, these tests must be updated to match.
// -------------------------------------------------------------------------
const EXPECTED_DEADMAN_TTL: i64 = 120;

// -------------------------------------------------------------------------
// DB helpers
// -------------------------------------------------------------------------

/// Returns Some(pool) if MQK_DATABASE_URL is set, otherwise None (test skips).
async fn pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.is_empty() => v,
        _ => return None,
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()?;
    mqk_db::migrate(&pool).await.ok()?;
    Some(pool)
}

/// Create a RUNNING run with `last_heartbeat_utc = now() - age_secs`.
async fn create_running_run(pool: &sqlx::PgPool, age_secs: i64) -> Uuid {
    let run_id = Uuid::new_v4();
    let engine_id = format!("test-das-{}", Uuid::new_v4());
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    let hb = Utc::now() - chrono::Duration::seconds(age_secs);
    sqlx::query("UPDATE runs SET last_heartbeat_utc = $1 WHERE run_id = $2")
        .bind(hb)
        .bind(run_id)
        .execute(pool)
        .await
        .expect("set heartbeat age");
    run_id
}

/// Create a RUNNING run with NULL `last_heartbeat_utc`.
async fn create_running_run_null_hb(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    let engine_id = format!("test-das-{}", Uuid::new_v4());
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    // last_heartbeat_utc remains NULL — begin_run does not set it.
    run_id
}

// -------------------------------------------------------------------------
// DAS01: 32-second-old heartbeat is NOT expired under TTL=120.
//
// Before the fix, DEADMAN_TTL was 5 s; a 32-second blocked initial tick
// would produce a 32-second-old heartbeat, which fired the deadman on the
// first loop pre-tick check.  With TTL=120 this heartbeat is well within
// bounds and the run survives.
// -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn das01_32s_heartbeat_not_expired_under_new_ttl() {
    let pool = match pool_or_skip().await {
        Some(p) => p,
        None => {
            eprintln!("DAS01: skipped (no MQK_DATABASE_URL)");
            return;
        }
    };
    let run_id = create_running_run(&pool, 32).await;
    let expired = mqk_db::deadman_expired(&pool, run_id, EXPECTED_DEADMAN_TTL, Utc::now())
        .await
        .expect("deadman_expired");
    assert!(
        !expired,
        "DAS01: 32-second-old heartbeat must NOT be expired with TTL={EXPECTED_DEADMAN_TTL}; \
         the initial tick blocking for 32 s must not deadman-halt the run"
    );
}

// -------------------------------------------------------------------------
// DAS02: NULL heartbeat is always expired regardless of TTL.
//
// A RUNNING run with no heartbeat at all is fail-closed: NULL means the loop
// never wrote a heartbeat (or it was wiped), which is unsafe.
// -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn das02_null_heartbeat_is_always_expired() {
    let pool = match pool_or_skip().await {
        Some(p) => p,
        None => {
            eprintln!("DAS02: skipped (no MQK_DATABASE_URL)");
            return;
        }
    };
    let run_id = create_running_run_null_hb(&pool).await;
    let expired = mqk_db::deadman_expired(&pool, run_id, EXPECTED_DEADMAN_TTL, Utc::now())
        .await
        .expect("deadman_expired");
    assert!(
        expired,
        "DAS02: NULL last_heartbeat_utc must always be expired — RUNNING with no heartbeat is unsafe"
    );
}

// -------------------------------------------------------------------------
// DAS03: 130-second-old heartbeat IS expired under TTL=120.
//
// A truly dead loop (no heartbeat for > TTL) must still be detected and
// halted.  Proves the deadman safety property is preserved.
// -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn das03_130s_heartbeat_is_expired_under_new_ttl() {
    let pool = match pool_or_skip().await {
        Some(p) => p,
        None => {
            eprintln!("DAS03: skipped (no MQK_DATABASE_URL)");
            return;
        }
    };
    let run_id = create_running_run(&pool, 130).await;
    let expired = mqk_db::deadman_expired(&pool, run_id, EXPECTED_DEADMAN_TTL, Utc::now())
        .await
        .expect("deadman_expired");
    assert!(
        expired,
        "DAS03: 130-second-old heartbeat must be expired with TTL={EXPECTED_DEADMAN_TTL}; \
         real deadman safety must be preserved"
    );
}

// -------------------------------------------------------------------------
// DAS04: enforce_deadman_or_halt halts the run when heartbeat is 130 s stale.
//
// Proves the full halt path: deadman detection → run status HALTED.
// -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn das04_enforce_deadman_halts_run_with_stale_heartbeat() {
    let pool = match pool_or_skip().await {
        Some(p) => p,
        None => {
            eprintln!("DAS04: skipped (no MQK_DATABASE_URL)");
            return;
        }
    };
    let run_id = create_running_run(&pool, 130).await;
    let halted = mqk_db::enforce_deadman_or_halt(&pool, run_id, EXPECTED_DEADMAN_TTL, Utc::now())
        .await
        .expect("enforce_deadman_or_halt");
    assert!(
        halted,
        "DAS04: enforce_deadman_or_halt must return true for 130-second-old heartbeat"
    );
    let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Halted),
        "DAS04: run must be HALTED after deadman enforcement; got {:?}",
        run.status
    );
}

// -------------------------------------------------------------------------
// DAS05: 119-second-old heartbeat is NOT expired (boundary proof).
//
// One second under the TTL boundary must remain safe.
// -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn das05_119s_heartbeat_not_expired_boundary() {
    let pool = match pool_or_skip().await {
        Some(p) => p,
        None => {
            eprintln!("DAS05: skipped (no MQK_DATABASE_URL)");
            return;
        }
    };
    let run_id = create_running_run(&pool, 119).await;
    let expired = mqk_db::deadman_expired(&pool, run_id, EXPECTED_DEADMAN_TTL, Utc::now())
        .await
        .expect("deadman_expired");
    assert!(
        !expired,
        "DAS05: 119-second-old heartbeat must NOT be expired (TTL={EXPECTED_DEADMAN_TTL}; \
         boundary: strictly >TTL is expired)"
    );
}
