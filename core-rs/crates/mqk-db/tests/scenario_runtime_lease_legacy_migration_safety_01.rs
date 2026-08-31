//! RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01: migration-level proofs
//! for 0069's quiescence-gated reconciliation of a legacy (`run_id IS NULL`)
//! `runtime_leader_lease` row.
//!
//! Every test here creates its own disposable database (never the shared
//! `mqk_test` database) and drops it when done -- these scenarios seed a
//! legacy lease row and/or `runs` rows directly via SQL before applying
//! migration 0069, which is exactly the kind of global/singleton-table setup
//! that must never run against a database shared with unrelated tests.
//!
//! Requires `MQK_DATABASE_URL` to point at an isolated test Postgres server
//! (127.0.0.1:5434 in this repo's dev setup, used only as the admin
//! connection to create/drop each disposable database); tests are skipped
//! (not failed) when it is absent.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, TimeZone, Utc};
use uuid::Uuid;

struct AdminConn {
    admin_url: String,
    base_url_without_db: String,
}

fn parse_admin_conn() -> Option<AdminConn> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    let (prefix_and_host, rest) = url.rsplit_once('/')?;
    let dbname_only = rest.split('?').next().unwrap_or(rest);
    if dbname_only.is_empty() {
        return None;
    }
    let base_url_without_db = format!("{prefix_and_host}/");
    let admin_url = format!("{base_url_without_db}postgres");
    Some(AdminConn {
        admin_url,
        base_url_without_db,
    })
}

async fn create_disposable_database(admin: &AdminConn, db_name: &str) -> anyhow::Result<String> {
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin.admin_url)
        .await?;
    sqlx::query(&format!(r#"CREATE DATABASE "{db_name}""#))
        .execute(&admin_pool)
        .await?;
    admin_pool.close().await;
    Ok(format!("{}{}", admin.base_url_without_db, db_name))
}

async fn drop_disposable_database(admin: &AdminConn, db_name: &str) {
    let Ok(admin_pool) = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin.admin_url)
        .await
    else {
        return;
    };
    let _ = sqlx::query(
        r#"SELECT pg_terminate_backend(pid) FROM pg_stat_activity
           WHERE datname = $1 AND pid <> pg_backend_pid()"#,
    )
    .bind(db_name)
    .execute(&admin_pool)
    .await;
    let _ = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{db_name}""#))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
}

fn disposable_db_name(label: &str) -> String {
    format!("mqk_lease_legacy_safety_{}_{}", label, Uuid::new_v4().simple())
}

/// Copies migration files with numeric version prefix <= `max_version`,
/// byte-for-byte, into a fresh temp directory -- mirrors
/// `scenario_ingest_job_cancelled_status_constraint_01.rs`'s established
/// pattern for testing "database at version N, then upgrade" scenarios.
fn copy_migrations_up_to(max_version: i64) -> tempfile::TempDir {
    let migrations_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let dest = tempfile::tempdir().expect("create temp migrations dir");

    for entry in std::fs::read_dir(&migrations_root).expect("read migrations dir") {
        let entry = entry.expect("read migrations dir entry");
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("migration file name must be utf8");
        let version_str = file_name.split('_').next().unwrap_or("");
        let Ok(version) = version_str.parse::<i64>() else {
            continue;
        };
        if version > max_version {
            continue;
        }
        std::fs::copy(&path, dest.path().join(file_name)).expect("copy migration file");
    }

    dest
}

async fn migrate_to(pool: &sqlx::PgPool, max_version: i64) -> anyhow::Result<()> {
    let capped_dir = copy_migrations_up_to(max_version);
    let migrator = sqlx::migrate::Migrator::new(Path::new(capped_dir.path())).await?;
    migrator.run(pool).await?;
    Ok(())
}

async fn migrate_fresh(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    mqk_db::migrate(pool).await
}

fn ts(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().expect("valid timestamp")
}

async fn insert_run_with_status(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    status: &str,
    last_heartbeat_utc: Option<DateTime<Utc>>,
) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("lease-legacy-safety-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: ts(0),
            git_hash: "TEST".to_string(),
            config_hash: format!("cfg-{run_id}"),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert_run");

    sqlx::query("UPDATE runs SET status = $2, last_heartbeat_utc = $3 WHERE run_id = $1")
        .bind(run_id)
        .bind(status)
        .bind(last_heartbeat_utc)
        .execute(pool)
        .await
        .expect("force run status/heartbeat for migration-safety fixture");
}

async fn seed_legacy_null_lease(pool: &sqlx::PgPool, lease_expires_at: DateTime<Utc>, updated_at: DateTime<Utc>) {
    sqlx::query(
        r#"
        INSERT INTO runtime_leader_lease (id, run_id, holder_id, epoch, lease_expires_at, updated_at)
        VALUES (1, NULL, 'legacy-holder', 1, $1, $2)
        "#,
    )
    .bind(lease_expires_at)
    .bind(updated_at)
    .execute(pool)
    .await
    .expect("seed legacy run_id-IS-NULL lease row");
}

// ---------------------------------------------------------------------------
// 1. Fresh database: migrations through 0069 apply cleanly end to end.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_01_fresh_database_applies_through_0069() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_01: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("fresh");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_fresh(&pool)
        .await
        .expect("fresh chain through 0069 must apply cleanly");

    let (fk_action,): (String,) = sqlx::query_as(
        r#"
        SELECT confdeltype::text FROM pg_constraint
         WHERE conname = 'runtime_leader_lease_run_id_fkey'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("fk lookup");
    assert_eq!(fk_action, "r", "expected ON DELETE RESTRICT ('r'), got confdeltype={fk_action}");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 2. 0068 database, no lease row at all: 0069 is a no-op success.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_02_0068_database_with_no_lease_row_succeeds() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_02: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("nolease");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    migrate_fresh(&pool)
        .await
        .expect("0068 -> 0069 upgrade with no lease row must apply cleanly");

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 0);

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 3. 0068 database, stale legacy lease, no active run: 0069 safely deletes
//    the ambiguous row and completes.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_03_quiescent_stale_legacy_lease_completes_safely() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_03: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("quiescent");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    // Raw-expired well past any plausible deadman window, and no ARMED/
    // fresh-RUNNING run exists at all -- unambiguously quiescent.
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let stopped_run = Uuid::new_v4();
    insert_run_with_status(&pool, stopped_run, "STOPPED", None).await;

    migrate_fresh(&pool)
        .await
        .expect("0068 -> 0069 upgrade must safely delete the quiescent legacy lease");

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(
        lease_count, 0,
        "the ambiguous legacy lease must be removed once the system is quiescent"
    );

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 4. 0068 database, legacy lease + an ARMED run: 0069 refuses atomically,
//    no partial schema mutation (the FK action change must not have taken
//    effect either).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_04_legacy_lease_with_armed_run_fails_closed_atomically() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_04: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("armed");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let armed_run = Uuid::new_v4();
    insert_run_with_status(&pool, armed_run, "ARMED", None).await;

    let err = migrate_fresh(&pool)
        .await
        .expect_err("0069 must refuse while an ARMED run exists");
    assert!(
        format!("{err:?}").to_lowercase().contains("runtime_leader_lease legacy migration safety"),
        "unexpected error: {err}"
    );

    // No partial mutation: the legacy row must still exist untouched, and
    // the FK action must still be the pre-0069 CASCADE.
    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 1, "the refused migration must not have deleted the legacy row");

    let (fk_action,): (String,) = sqlx::query_as(
        r#"
        SELECT confdeltype::text FROM pg_constraint
         WHERE conname = 'runtime_leader_lease_run_id_fkey'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("fk lookup");
    assert_eq!(fk_action, "c", "a failed migration must not have applied the RESTRICT FK change");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 5. 0068 database, legacy lease + a deadman-fresh RUNNING run: 0069 refuses
//    atomically -- the RUNNING counterpart of mig_04.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_05_legacy_lease_with_deadman_fresh_running_run_fails_closed() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_05: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("running");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let running_run = Uuid::new_v4();
    // Heartbeat 10s old: well inside the 120s deadman window.
    insert_run_with_status(&pool, running_run, "RUNNING", Some(now - Duration::seconds(10))).await;

    let err = migrate_fresh(&pool)
        .await
        .expect_err("0069 must refuse while a deadman-fresh RUNNING run exists");
    assert!(
        format!("{err:?}").to_lowercase().contains("runtime_leader_lease legacy migration safety"),
        "unexpected error: {err}"
    );

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 1, "the refused migration must not have deleted the legacy row");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 6. 0068 database, legacy lease + a RUNNING run whose heartbeat is stale
//    (past the deadman window): quiescence must be judged by run status
//    alone, never by heartbeat freshness, so 0069 must ALSO refuse here.
//    runs.rs::begin_run's ARMED -> RUNNING transition does not atomically
//    establish a heartbeat (heartbeat_run is a separate, later write), so a
//    RUNNING run legitimately starting/resuming can show exactly this
//    stale-heartbeat shape -- treating it as proof of absence would let
//    migration race a starting runtime.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_06_running_run_with_stale_heartbeat_still_blocks_migration() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_06: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("stalehb");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let starting_running_run = Uuid::new_v4();
    insert_run_with_status(
        &pool,
        starting_running_run,
        "RUNNING",
        Some(now - Duration::hours(4)),
    )
    .await;

    let err = migrate_fresh(&pool)
        .await
        .expect_err("a RUNNING run must block migration regardless of heartbeat staleness");
    assert!(
        format!("{err:?}").to_lowercase().contains("runtime_leader_lease legacy migration safety"),
        "unexpected error: {err}"
    );

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 1, "the refused migration must not have deleted the legacy row");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 8. 0068 database, legacy lease + a RUNNING run with NULL heartbeat (the
//    realistic shape immediately after begin_run, before the first
//    heartbeat_run call ever lands): 0069 must refuse -- NULL heartbeat is
//    never evidence that RUNNING authority is absent.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_08_running_run_with_null_heartbeat_blocks_migration() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_08: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("nullhb");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let just_begun_run = Uuid::new_v4();
    insert_run_with_status(&pool, just_begun_run, "RUNNING", None).await;

    let err = migrate_fresh(&pool)
        .await
        .expect_err("a RUNNING run with NULL heartbeat must block migration");
    assert!(
        format!("{err:?}").to_lowercase().contains("runtime_leader_lease legacy migration safety"),
        "unexpected error: {err}"
    );

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 1, "the refused migration must not have deleted the legacy row");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 9. 0068 database, legacy lease + a HALTED run with a stale heartbeat: 0069
//    must succeed. `acquire_or_refresh_lease_for_running_run` refuses to
//    acquire or refresh for any non-RUNNING status (runtime_lease.rs, "if
//    status != RUNNING"), so a HALTED run can never hold live leadership
//    authority through the production path -- it carries no ambiguity for
//    this migration to protect against.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_09_halted_run_does_not_block_quiescent_migration() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_09: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("halted");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let halted_run = Uuid::new_v4();
    insert_run_with_status(&pool, halted_run, "HALTED", Some(now - Duration::hours(4))).await;

    migrate_fresh(&pool)
        .await
        .expect("a HALTED run must not block quiescent migration");

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 0);

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 10. 0068 database, legacy lease + a CREATED-only run (never armed): 0069
//     must succeed. CREATED has no execution authority.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_10_created_only_run_does_not_block_quiescent_migration() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_10: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("created");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let created_run = Uuid::new_v4();
    insert_run_with_status(&pool, created_run, "CREATED", None).await;

    migrate_fresh(&pool)
        .await
        .expect("a CREATED-only run must not block quiescent migration");

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 0);

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 11. Concurrency: begin_run commits (via the real production arm_run/
//     begin_run seam) before the migration acquires its lock -- migration
//     must refuse. Proves the strict quiescence check sees a genuinely
//     production-produced RUNNING row (heartbeat still NULL, since
//     heartbeat_run was never called), not just a directly-forced fixture.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_11_begin_run_committed_before_migration_blocks_migration() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_11: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("beginfirst");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_to(&pool, 68)
        .await
        .expect("database must reach exactly 0068");

    let now = Utc::now();
    seed_legacy_null_lease(&pool, now - Duration::hours(2), now - Duration::hours(3)).await;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("lease-legacy-safety-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: ts(0),
            git_hash: "TEST".to_string(),
            config_hash: format!("cfg-{run_id}"),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin_run");

    let err = migrate_fresh(&pool)
        .await
        .expect_err("migration must refuse once begin_run has committed RUNNING status");
    assert!(
        format!("{err:?}").to_lowercase().contains("runtime_leader_lease legacy migration safety"),
        "unexpected error: {err}"
    );

    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(&pool)
        .await
        .expect("count runtime_leader_lease");
    assert_eq!(lease_count, 1, "the refused migration must not have deleted the legacy row");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 12. Concurrency: the migration's `LOCK TABLE runs IN SHARE MODE` wins
//     first -- a concurrent begin_run (ROW EXCLUSIVE) must block for the
//     duration of the lock and can only proceed once it is released,
//     mirroring mig_07's insert-blocks proof but for the production
//     begin_run seam specifically.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_12_begin_run_blocks_while_migration_lock_is_held() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_12: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("lockwins");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_fresh(&pool)
        .await
        .expect("fresh chain through 0069 must apply cleanly");

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("lease-legacy-safety-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: ts(0),
            git_hash: "TEST".to_string(),
            config_hash: format!("cfg-{run_id}"),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm_run");

    let mut locker_tx = pool.begin().await.expect("begin locker tx");
    sqlx::query("LOCK TABLE runs IN SHARE MODE")
        .execute(&mut *locker_tx)
        .await
        .expect("acquire SHARE lock, simulating migration 0069's in-flight window");

    let begin_pool = pool.clone();
    let racer = tokio::spawn(async move { mqk_db::begin_run(&begin_pool, run_id).await });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !racer.is_finished(),
        "begin_run must block while the migration-equivalent SHARE lock is held"
    );

    locker_tx.rollback().await.expect("release locker lock");
    racer
        .await
        .expect("racer task must complete once the lock is released")
        .expect("begin_run must succeed once the lock is released");

    let (status,): (String,) = sqlx::query_as("SELECT status FROM runs WHERE run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("post-unblock status check");
    assert_eq!(status, "RUNNING");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 13. Concurrency: while a transaction holds the row lock this migration
//    takes on an existing legacy lease row (simulating the migration's own
//    in-flight decision window), a concurrent attempt to insert a new run
//    blocks rather than proceeding -- proving `LOCK TABLE runs IN SHARE
//    MODE` genuinely fences run-creation against the migration's
//    check-then-delete, so no concurrent run-start can race it into an
//    unbound or dual-authority state.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_07_concurrent_run_insert_blocks_while_migration_lock_is_held() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_07: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("race");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_fresh(&pool)
        .await
        .expect("fresh chain through 0069 must apply cleanly");

    let mut locker_tx = pool.begin().await.expect("begin locker tx");
    sqlx::query("LOCK TABLE runs IN SHARE MODE")
        .execute(&mut *locker_tx)
        .await
        .expect("acquire SHARE lock, simulating migration 0069's in-flight window");

    let insert_pool = pool.clone();
    let racer_run_id = Uuid::new_v4();
    let racer = tokio::spawn(async move {
        insert_run_with_status(&insert_pool, racer_run_id, "CREATED", None).await;
    });

    // The racer's INSERT needs ROW EXCLUSIVE on `runs`, which conflicts with
    // the held SHARE lock -- it must not have completed yet.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !racer.is_finished(),
        "a concurrent run insert must block while the migration-equivalent SHARE lock is held"
    );

    locker_tx.rollback().await.expect("release locker lock");
    racer
        .await
        .expect("racer task must complete once the lock is released");

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = $1)")
        .bind(racer_run_id)
        .fetch_one(&pool)
        .await
        .expect("post-unblock existence check");
    assert!(exists, "the racer's insert must have proceeded after the lock was released");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}
