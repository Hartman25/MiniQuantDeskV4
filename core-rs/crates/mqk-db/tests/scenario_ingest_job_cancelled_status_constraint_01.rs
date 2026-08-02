//! INGEST-JOB-CANCEL-STATUS-CONSTRAINT-REPAIR-01 (FULL-AUDIT-FAIL-012):
//! proves that migration 0061 repairs `sys_ingest_jobs_status_check` so the
//! DB vocabulary matches `IngestJobStatus::as_str()`/`from_str()` exactly,
//! including `'cancelled'`, on both a fresh chain and a 0060 -> 0061
//! upgrade, while every other legal status remains accepted and unknown
//! strings remain rejected.
//!
//! Every test here creates its own disposable database (never the shared
//! `mqk_test` database) and drops it when done, per patch instructions:
//! "Do not run DROP SCHEMA migration-bootstrap proof against shared
//! mqk_test. Use a disposable database and drop it afterward."
//!
//! Requires `MQK_DATABASE_URL` to point at an isolated test Postgres
//! (127.0.0.1:5434 in this repo's dev setup); tests are skipped (not
//! failed) when it is absent so this file is safe in environments without
//! a local Postgres.

use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Connection info derived from `MQK_DATABASE_URL`, split into the
/// maintenance (admin) database used to CREATE/DROP disposable databases
/// and the host/credentials shared by every disposable database URL.
struct AdminConn {
    admin_url: String,
    base_url_without_db: String,
}

fn parse_admin_conn() -> Option<AdminConn> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    // Expected shape: postgres://user:pass@host:port/dbname[?query]
    let (prefix_and_host, rest) = url.rsplit_once('/')?;
    let db_and_query = rest;
    let dbname_only = db_and_query.split('?').next().unwrap_or(db_and_query);
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
    // Terminate any lingering backends so DROP DATABASE cannot be blocked.
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
    format!(
        "mqk_ingest_cancel_repair_{}_{}",
        label,
        Uuid::new_v4().simple()
    )
}

/// Copies migration files with numeric version prefix <= `max_version` from
/// the crate's real migrations directory (flat, non-recursive — matching
/// how both the compile-time `sqlx::migrate!` macro and the runtime
/// `Migrator` resolve a directory source, and why the `hold/` subdirectory
/// is never picked up) into a fresh temp directory, byte-for-byte, so the
/// runtime `Migrator` built over it computes the exact same per-migration
/// checksums the embedded compile-time migrator would.
fn copy_migrations_up_to(max_version: i64) -> tempfile::TempDir {
    let migrations_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
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

/// Runs the real embedded migration chain (through the current HEAD
/// migration, 0061 included) against a freshly created disposable database.
async fn migrate_fresh(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    mqk_db::migrate(pool).await
}

async fn insert_job_with_status(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into sys_ingest_jobs (
            job_id, source, mode, status, timeframe, csv_path, registry_path,
            provider_registry_path, symbols_source, asset_class, dry_run,
            provider_api_calls_allowed, api_calls_made, symbols_count,
            symbols_completed, symbols_failed, planned_first_symbol,
            planned_last_symbol, rows_read, rows_inserted, rows_rejected,
            quality_report_path, error, provider_enabled,
            provider_verification_status, created_at_utc, started_at_utc,
            completed_at_utc, updated_at_utc
        ) values (
            $1, 'csv', null, $2, '1D', null, null,
            null, null, 'equity', true,
            false, 0, null,
            null, null, null,
            null, null, null, null,
            null, null, null,
            null, $3, null,
            null, $3
        )
        "#,
    )
    .bind(job_id)
    .bind(status)
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

// ---------------------------------------------------------------------------
// 1. Fresh full chain accepts 'cancelled'.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_01_fresh_chain_accepts_cancelled() {
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

    migrate_fresh(&pool).await.expect("fresh migration chain must apply cleanly");

    let job_id = Uuid::new_v4();
    insert_job_with_status(&pool, job_id, "cancelled")
        .await
        .expect("fresh chain must accept 'cancelled' status");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 2. 0060 -> 0061 upgrade accepts 'cancelled'.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_02_0060_to_0061_upgrade_accepts_cancelled() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_02: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("upgrade");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    // Bring the database to exactly the 0060 state using byte-identical
    // copies of migrations 0001..=0060 (same checksums the embedded
    // migrator would compute), then apply the real embedded chain, which
    // must see only 0061 as pending.
    let capped_dir = copy_migrations_up_to(60);
    let migrator = sqlx::migrate::Migrator::new(Path::new(capped_dir.path()))
        .await
        .expect("build capped migrator");
    migrator
        .run(&pool)
        .await
        .expect("migrate to 0060 must apply cleanly");

    // Sanity: 'cancelled' must still be rejected at 0060, proving the
    // upgrade below is the thing that changes behavior.
    let pre_upgrade_job_id = Uuid::new_v4();
    let pre_upgrade_result =
        insert_job_with_status(&pool, pre_upgrade_job_id, "cancelled").await;
    assert!(
        pre_upgrade_result.is_err(),
        "database at 0060 must still reject 'cancelled' before the upgrade"
    );

    migrate_fresh(&pool)
        .await
        .expect("0060 -> 0061 upgrade must apply cleanly");

    let job_id = Uuid::new_v4();
    insert_job_with_status(&pool, job_id, "cancelled")
        .await
        .expect("post-upgrade database must accept 'cancelled' status");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// ---------------------------------------------------------------------------
// 3. Every current legal Rust status is accepted; legal existing rows
//    (inserted before the constraint replacement) are preserved.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_03_every_rust_status_accepted_and_preserved_across_upgrade() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_03: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("vocab");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    // Bring to 0060 and insert one row per pre-existing legal status, to
    // prove the migration preserves rows that existed before it ran.
    let capped_dir = copy_migrations_up_to(60);
    let migrator = sqlx::migrate::Migrator::new(Path::new(capped_dir.path()))
        .await
        .expect("build capped migrator");
    migrator.run(&pool).await.expect("migrate to 0060");

    let pre_existing_statuses = [
        "queued",
        "running",
        "completed",
        "dry_run_completed",
        "failed",
        "refused",
        "partial",
    ];
    let mut pre_existing_ids = Vec::new();
    for status in pre_existing_statuses {
        let job_id = Uuid::new_v4();
        insert_job_with_status(&pool, job_id, status)
            .await
            .unwrap_or_else(|e| panic!("insert legal pre-existing status '{status}' failed: {e}"));
        pre_existing_ids.push((job_id, status));
    }

    migrate_fresh(&pool).await.expect("upgrade to 0061");

    // All previously-inserted rows must still read back with their original
    // status, untouched by the constraint replacement.
    for (job_id, status) in &pre_existing_ids {
        let (row_status,): (String,) =
            sqlx::query_as("select status from sys_ingest_jobs where job_id = $1")
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .expect("row must survive migration");
        assert_eq!(&row_status, status, "existing row status must be preserved");
    }

    // Every current IngestJobStatus::as_str() value, including 'cancelled',
    // must be insertable after the upgrade.
    let all_current_statuses = [
        mqk_daemon_status_str_queued(),
        "running",
        "completed",
        "dry_run_completed",
        "failed",
        "refused",
        "partial",
        "cancelled",
    ];
    for status in all_current_statuses {
        let job_id = Uuid::new_v4();
        insert_job_with_status(&pool, job_id, status)
            .await
            .unwrap_or_else(|e| panic!("insert current legal status '{status}' failed: {e}"));
    }

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

// Kept as a tiny named function (rather than a bare literal) so the
// vocabulary list above reads as "the Rust vocabulary", not a duplicated
// magic string, without adding a cross-crate dependency for one constant.
fn mqk_daemon_status_str_queued() -> &'static str {
    "queued"
}

// ---------------------------------------------------------------------------
// 4. A bogus status is rejected.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn mig_04_bogus_status_rejected() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("mig_04: skipped; MQK_DATABASE_URL is not set");
        return;
    };
    let db_name = disposable_db_name("bogus");
    let db_url = create_disposable_database(&admin, &db_name)
        .await
        .expect("create disposable database");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect to disposable database");

    migrate_fresh(&pool).await.expect("fresh migration chain");

    let job_id = Uuid::new_v4();
    let result = insert_job_with_status(&pool, job_id, "not_a_real_status").await;
    assert!(
        result.is_err(),
        "an unknown status string must be rejected by the CHECK constraint"
    );

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}
