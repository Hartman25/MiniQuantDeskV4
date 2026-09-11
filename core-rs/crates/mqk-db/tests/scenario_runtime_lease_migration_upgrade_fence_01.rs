//! M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01
//!
//! Proves the canonical `mqk_db::migrate` runner closes the chronology hole
//! where immutable historical migration 0069 could otherwise remove a legacy
//! lease before forward migration 0070 applies its stricter ARMED/RUNNING
//! quiescence rule.

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

async fn create_disposable_database(
    admin: &AdminConn,
    label: &str,
) -> anyhow::Result<(String, String)> {
    let db_name = format!("mqk_m113c_fence_{}_{}", label, Uuid::new_v4().simple());

    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin.admin_url)
        .await?;

    sqlx::query(&format!(r#"CREATE DATABASE "{db_name}""#))
        .execute(&admin_pool)
        .await?;

    admin_pool.close().await;

    Ok((
        db_name.clone(),
        format!("{}{}", admin.base_url_without_db, db_name),
    ))
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
        r#"SELECT pg_terminate_backend(pid)
             FROM pg_stat_activity
            WHERE datname = $1
              AND pid <> pg_backend_pid()"#,
    )
    .bind(db_name)
    .execute(&admin_pool)
    .await;

    let _ = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{db_name}""#))
        .execute(&admin_pool)
        .await;

    admin_pool.close().await;
}

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
            .expect("migration filename must be utf8");

        let version_str = file_name.split('_').next().unwrap_or("");
        let Ok(version) = version_str.parse::<i64>() else {
            continue;
        };

        if version <= max_version {
            std::fs::copy(&path, dest.path().join(file_name)).expect("copy migration file");
        }
    }

    dest
}

async fn migrate_to(pool: &sqlx::PgPool, max_version: i64) -> anyhow::Result<()> {
    let capped_dir = copy_migrations_up_to(max_version);
    let migrator = sqlx::migrate::Migrator::new(Path::new(capped_dir.path())).await?;
    migrator.run(pool).await?;
    Ok(())
}

fn ts(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .expect("valid timestamp")
}

async fn insert_run_with_status(
    pool: &sqlx::PgPool,
    status: &str,
    last_heartbeat_utc: Option<DateTime<Utc>>,
) -> Uuid {
    let run_id = Uuid::new_v4();

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("m113c-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: ts(0),
            git_hash: "TEST".to_string(),
            config_hash: format!("cfg-{run_id}"),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");

    sqlx::query(
        "UPDATE runs
            SET status = $2,
                last_heartbeat_utc = $3
          WHERE run_id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(last_heartbeat_utc)
    .execute(pool)
    .await
    .expect("set run fixture status");

    run_id
}

async fn seed_legacy_lease(pool: &sqlx::PgPool, has_run_id: bool) {
    let now = Utc::now();

    if has_run_id {
        sqlx::query(
            r#"
            INSERT INTO runtime_leader_lease
                (id, run_id, holder_id, epoch, lease_expires_at, updated_at)
            VALUES
                (1, NULL, 'legacy-holder', 1, $1, $2)
            "#,
        )
        .bind(now - Duration::hours(2))
        .bind(now - Duration::hours(3))
        .execute(pool)
        .await
        .expect("seed post-0068 legacy lease");
    } else {
        sqlx::query(
            r#"
            INSERT INTO runtime_leader_lease
                (id, holder_id, epoch, lease_expires_at, updated_at)
            VALUES
                (1, 'legacy-holder', 1, $1, $2)
            "#,
        )
        .bind(now - Duration::hours(2))
        .bind(now - Duration::hours(3))
        .execute(pool)
        .await
        .expect("seed pre-0068 legacy lease");
    }
}

async fn max_applied_version(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(version)
           FROM _sqlx_migrations
          WHERE success = true",
    )
    .fetch_one(pool)
    .await
    .expect("read max applied migration")
    .expect("at least one migration")
}

async fn lease_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM runtime_leader_lease")
        .fetch_one(pool)
        .await
        .expect("count runtime lease rows")
}

async fn fk_delete_action(pool: &sqlx::PgPool) -> String {
    sqlx::query_scalar(
        "SELECT confdeltype::text
           FROM pg_constraint
          WHERE conname = 'runtime_leader_lease_run_id_fkey'",
    )
    .fetch_one(pool)
    .await
    .expect("read runtime lease FK delete action")
}

async fn run_id_column_exists(pool: &sqlx::PgPool) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM information_schema.columns
              WHERE table_schema = ANY(current_schemas(false))
                AND table_name = 'runtime_leader_lease'
                AND column_name = 'run_id'
         )",
    )
    .fetch_one(pool)
    .await
    .expect("inspect run_id column")
}

fn assert_fence_error(err: anyhow::Error) {
    let detail = format!("{err:#}");
    assert!(
        detail.contains("M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01"),
        "unexpected migration error: {detail}"
    );
}

async fn make_0068_case(
    admin: &AdminConn,
    label: &str,
    status: &str,
    heartbeat: Option<DateTime<Utc>>,
) -> (String, sqlx::PgPool, Uuid) {
    let (db_name, db_url) = create_disposable_database(admin, label)
        .await
        .expect("create disposable DB");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&db_url)
        .await
        .expect("connect disposable DB");

    migrate_to(&pool, 68).await.expect("migrate through 0068");
    seed_legacy_lease(&pool, true).await;
    let run_id = insert_run_with_status(&pool, status, heartbeat).await;

    (db_name, pool, run_id)
}

async fn assert_blocked_at_0068(pool: &sqlx::PgPool) {
    let err = mqk_db::migrate(pool)
        .await
        .expect_err("historical-upgrade fence must refuse before 0069");

    assert_fence_error(err);
    assert_eq!(max_applied_version(pool).await, 68);
    assert_eq!(lease_count(pool).await, 1);
    assert_eq!(fk_delete_action(pool).await, "c");
}

#[tokio::test]
async fn fence01_0068_stale_running_blocks_before_0069() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence01 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, pool, _) = make_0068_case(
        &admin,
        "stale",
        "RUNNING",
        Some(Utc::now() - Duration::hours(4)),
    )
    .await;

    assert_blocked_at_0068(&pool).await;

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

#[tokio::test]
async fn fence02_0068_null_heartbeat_running_blocks_before_0069() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence02 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, pool, _) = make_0068_case(&admin, "nullhb", "RUNNING", None).await;

    assert_blocked_at_0068(&pool).await;

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

#[tokio::test]
async fn fence03_0068_armed_blocks_before_0069() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence03 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, pool, _) = make_0068_case(&admin, "armed", "ARMED", None).await;

    assert_blocked_at_0068(&pool).await;

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

#[tokio::test]
async fn fence04_resolved_halted_state_allows_0068_to_latest() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence04 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, pool, run_id) = make_0068_case(&admin, "resume", "RUNNING", None).await;

    assert_blocked_at_0068(&pool).await;

    sqlx::query("UPDATE runs SET status = 'HALTED' WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("resolve fixture authority to HALTED");

    mqk_db::migrate(&pool)
        .await
        .expect("HALTED state should allow 0069 and 0070 to apply");

    assert_eq!(max_applied_version(&pool).await, 70);
    assert_eq!(lease_count(&pool).await, 0);
    assert_eq!(fk_delete_action(&pool).await, "r");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

#[tokio::test]
async fn fence05_0067_legacy_schema_running_blocks_before_0068() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence05 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, db_url) = create_disposable_database(&admin, "pre68")
        .await
        .expect("create disposable DB");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&db_url)
        .await
        .expect("connect disposable DB");

    migrate_to(&pool, 67).await.expect("migrate through 0067");
    assert!(!run_id_column_exists(&pool).await);

    seed_legacy_lease(&pool, false).await;
    insert_run_with_status(&pool, "RUNNING", None).await;

    let err = mqk_db::migrate(&pool)
        .await
        .expect_err("pre-0068 legacy lease plus RUNNING authority must block");

    assert_fence_error(err);
    assert_eq!(max_applied_version(&pool).await, 67);
    assert!(!run_id_column_exists(&pool).await);
    assert_eq!(lease_count(&pool).await, 1);

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}

#[tokio::test]
async fn fence06_database_at_0069_without_legacy_lease_can_apply_0070() {
    let Some(admin) = parse_admin_conn() else {
        eprintln!("fence06 skipped; MQK_DATABASE_URL is not set");
        return;
    };

    let (db_name, db_url) = create_disposable_database(&admin, "at69")
        .await
        .expect("create disposable DB");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&db_url)
        .await
        .expect("connect disposable DB");

    migrate_to(&pool, 69).await.expect("migrate through 0069");
    assert_eq!(lease_count(&pool).await, 0);

    insert_run_with_status(&pool, "RUNNING", None).await;

    mqk_db::migrate(&pool)
        .await
        .expect("database already past 0069 must be allowed to apply 0070");

    assert_eq!(max_applied_version(&pool).await, 70);
    assert_eq!(lease_count(&pool).await, 0);
    assert_eq!(fk_delete_action(&pool).await, "r");

    pool.close().await;
    drop_disposable_database(&admin, &db_name).await;
}
