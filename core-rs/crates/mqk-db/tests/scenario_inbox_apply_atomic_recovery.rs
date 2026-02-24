//! Patch D2 — Crash-safe inbox apply: `applied_at_utc` recovery marker.
//!
//! Invariants under test:
//!
//! 1. A fill inserted but NOT marked applied surfaces in the recovery list
//!    (simulates crash between DB insert and portfolio apply completing).
//!
//! 2. A fill inserted AND marked applied is absent from the recovery list
//!    (normal happy path — no spurious replays).
//!
//! 3. A duplicate fill cannot re-open the apply gate: the dedupe key
//!    (broker_message_id) is already present, so inserted=false, apply is
//!    skipped, and no double-apply of PnL/exposure occurs.
//!
//! Requires a live PostgreSQL instance reachable via MQK_DATABASE_URL.
//! Skipped automatically when that variable is absent (CI without a DB).

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn make_run(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "TEST".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TEST".to_string(),
        },
    )
    .await
    .expect("insert_run");
    run_id
}

// ---------------------------------------------------------------------------
// Test 1: crash before mark_applied → fill surfaces in recovery list
// ---------------------------------------------------------------------------

/// A fill that was DB-inserted but whose apply step did not complete (simulated
/// crash) must appear in inbox_load_unapplied_for_run so the recovery path can
/// replay it.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn unapplied_fill_surfaces_in_recovery_list() {
    let db_url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");

    let run_id = make_run(&pool).await;
    let msg_id = format!("d2-unapplied-{}", Uuid::new_v4());

    // Receive the fill — insert succeeds.
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"qty": 10}))
        .await
        .expect("inbox_insert_deduped");
    assert!(inserted, "first inbox insert must return true");

    // Intentionally skip inbox_mark_applied — simulates a crash between the
    // DB insert and the portfolio apply completing.

    // Recovery: the unapplied fill must appear for this run.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run");

    let found = unapplied.iter().any(|r| r.broker_message_id == msg_id);
    assert!(
        found,
        "fill inserted but not marked applied must appear in recovery list"
    );
}

// ---------------------------------------------------------------------------
// Test 2: normal path (insert + mark applied) → absent from recovery list
// ---------------------------------------------------------------------------

/// A fill that completes the full happy path (insert → apply → mark_applied)
/// must NOT appear in the recovery list — no spurious replays.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn marked_applied_fill_absent_from_recovery_list() {
    let db_url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");

    let run_id = make_run(&pool).await;
    let msg_id = format!("d2-applied-{}", Uuid::new_v4());

    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"qty": 5}))
        .await
        .expect("inbox_insert_deduped");
    assert!(inserted, "first inbox insert must return true");

    // Normal path: apply completed, stamp the row.
    mqk_db::inbox_mark_applied(&pool, &msg_id)
        .await
        .expect("inbox_mark_applied");

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run");

    let found = unapplied.iter().any(|r| r.broker_message_id == msg_id);
    assert!(
        !found,
        "marked-applied fill must NOT appear in recovery list"
    );
}

// ---------------------------------------------------------------------------
// Test 3: duplicate fill cannot double-apply PnL/exposure
// ---------------------------------------------------------------------------

/// The broker_message_id dedupe key ensures that even if the same fill is
/// delivered twice, the apply gate opens exactly once.  Under crash/restart the
/// recovery list correctly shows zero unapplied rows for a fully applied fill.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn duplicate_fill_cannot_double_apply() {
    let db_url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");

    let run_id = make_run(&pool).await;
    let msg_id = format!("d2-dedupe-{}", Uuid::new_v4());
    let mut apply_count = 0u32;

    // First delivery: insert succeeds, apply runs, mark applied.
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"qty": 20}))
        .await
        .expect("first inbox_insert_deduped");
    if inserted {
        apply_count += 1;
        mqk_db::inbox_mark_applied(&pool, &msg_id)
            .await
            .expect("inbox_mark_applied");
    }
    assert_eq!(
        apply_count, 1,
        "fill must be applied exactly once on first delivery"
    );

    // Duplicate delivery: insert returns false → apply gate stays closed.
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"qty": 20}))
        .await
        .expect("second inbox_insert_deduped");
    if inserted {
        apply_count += 1;
    }
    assert_eq!(
        apply_count, 1,
        "duplicate fill must not open the apply gate (double-apply prevention)"
    );

    // Recovery list must be empty for this run: the one fill was fully applied.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run");
    let found = unapplied.iter().any(|r| r.broker_message_id == msg_id);
    assert!(
        !found,
        "fully applied fill must not appear in recovery list"
    );
}
