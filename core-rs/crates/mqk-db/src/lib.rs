// core-rs/crates/mqk-db/src/lib.rs
//
// Thin crate root. All domain logic lives in the modules below.
// This file owns only:
//   - the TimeSource abstraction (shared by all modules)
//   - connection / migration helpers (crate-level infrastructure)
//   - re-exports that preserve the pre-refactor public API
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{
    migrate::Migrate as SqlxMigrate, postgres::PgPoolOptions, Connection, PgConnection, PgPool,
};

pub const ENV_DB_URL: &str = "MQK_DATABASE_URL";

pub mod runtime_lease;

// ---------------------------------------------------------------------------
// TimeSource — injectable clock abstraction (FC-5)
// ---------------------------------------------------------------------------

/// Abstraction over a UTC clock, injected wherever enforcement or
/// state-transition logic needs a timestamp.
///
/// This crate must remain deterministic: it must not read the wall clock.
/// Production code should provide a `TimeSource` implementation at the
/// runtime/daemon layer and inject it into db calls.
pub trait TimeSource: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
}

/// Deterministic `TimeSource` for tests and scenario replay.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock {
    now: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }
}

impl TimeSource for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now
    }
}

// -----------------------------
// Backtest Market Data (Patch A/B/C)
// -----------------------------
// PATCH B/C: expose md module + re-export ingest/report types at crate root
pub mod md;

pub use md::{
    fetch_md_bars, fetch_recent_completed_bars_for_strategy, latest_stored_bar_end_ts,
    CoverageTotals, FetchMdBarsArgs, IngestCsvArgs, IngestProviderBarsArgs, IngestResult,
    MdBarProviderMetadata, MdBarRow, MdQualityReport, ProviderBar,
};

pub use md::{
    ingest_csv_to_md_bars, ingest_provider_bars_to_md_bars,
    ingest_provider_bars_to_md_bars_with_provider_metadata,
};

// ---------------------------------------------------------------------------
// Domain modules
// ---------------------------------------------------------------------------

pub mod account_equity_baseline;
pub mod alert_acks;
pub mod arm_state;
pub mod audit;
pub mod autonomous_daily_operation;
pub mod broker_baseline;
pub mod dynamic_selection_evidence;
pub mod fill_quality;
pub mod flow;
pub mod inbox;
pub mod incidents;
pub mod order_lifecycle;
pub mod orders;
pub mod paper_portfolio;
pub mod reconcile_state;
pub mod restart_intent;
pub mod runs;
pub mod runtime_opportunity_allocation;
pub mod runtime_strategy_conflict;
pub mod strategy;
pub mod strategy_promotion;

// Re-export all public items to preserve pre-refactor public API.
// Callers continue to use `mqk_db::insert_run`, `mqk_db::RunStatus`, etc.
pub use account_equity_baseline::*;
pub use alert_acks::*;
pub use arm_state::*;
pub use audit::*;
pub use autonomous_daily_operation::*;
pub use broker_baseline::*;
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 1: durable
// dynamic-selection plan evidence store (never portfolio/P&L/order truth).
pub use dynamic_selection_evidence::*;
pub use fill_quality::*;
pub use flow::{fetch_execution_flow, ExecutionFlowRow, FlowQuery};
pub use inbox::*;
pub use incidents::*;
pub use order_lifecycle::*;
pub use orders::*;
pub use paper_portfolio::*;
pub use reconcile_state::*;
pub use restart_intent::*;
pub use runs::*;
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase G: durable allocation-cycle
// evidence store (never portfolio/P&L/order truth).
pub use runtime_opportunity_allocation::*;
// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase C: durable conflict-resolution
// evidence store (never portfolio/P&L/order truth).
pub use runtime_strategy_conflict::*;
pub use strategy::*;
pub use strategy_promotion::*;

// ---------------------------------------------------------------------------
// Connection / migration / status — crate-level infrastructure
// ---------------------------------------------------------------------------

/// Connect to Postgres using MQK_DATABASE_URL.
pub async fn connect_from_env() -> Result<PgPool> {
    let url = std::env::var(ENV_DB_URL).with_context(|| format!("missing env var {ENV_DB_URL}"))?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .context("failed to connect to Postgres")?;

    Ok(pool)
}

/// Test helper used by integration tests:
/// - Connect using MQK_DATABASE_URL
/// - Ensure migrations are applied
pub async fn testkit_db_pool() -> Result<PgPool> {
    let pool = connect_from_env().await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Run embedded SQLx migrations.
///
/// M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01:
/// migration 0069 is immutable deployed history. Its original reconciliation
/// predicate can consider a RUNNING row with a stale/NULL heartbeat quiescent,
/// while migration 0070 intentionally tightens that rule to treat every
/// ARMED/RUNNING row as authority. Because SQLx executes 0069 before 0070, a
/// database still below 0069 needs a runner-level fence so 0069 cannot delete
/// an ambiguous legacy lease before 0070 gets a chance to enforce the stricter
/// rule.
///
/// The fence acquires SQLx's normal Postgres migration advisory lock first,
/// then (only while 0069 is pending and the relevant tables already exist)
/// holds transaction-scoped table locks in production lock order: `runs`
/// before `runtime_leader_lease`. This serializes run/lease mutation across
/// the preflight and the complete pending migration chain. The embedded
/// migrator then runs with its own locking disabled because this connection
/// already owns SQLx's advisory migration lock.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    let mut conn = pool
        .acquire()
        .await
        .context("db migrate: acquire dedicated connection failed")?;

    SqlxMigrate::lock(&mut *conn)
        .await
        .context("db migrate: acquire SQLx advisory migration lock failed")?;

    let migration_result = migrate_while_sqlx_locked(&mut conn).await;
    let unlock_result = SqlxMigrate::unlock(&mut *conn).await;

    match (migration_result, unlock_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(unlock_err)) => {
            let _ = conn.close().await;
            Err(anyhow::Error::new(unlock_err)
                .context("db migrate: release SQLx advisory migration lock failed"))
        }
        (Err(err), Err(unlock_err)) => {
            let _ = conn.close().await;
            Err(err.context(format!(
                "db migrate also failed to release SQLx advisory migration lock: {unlock_err}"
            )))
        }
    }
}

async fn run_embedded_migrations_without_lock(conn: &mut PgConnection) -> Result<()> {
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_locking(false);
    // `Migrator::run` is generic over `Acquire` and makes callers that await
    // mqk_db::migrate inside tokio::spawn fail the all-targets Send/HRTB
    // compile gate ("implementation of Acquire is not general enough").
    // We already hold a concrete PgConnection plus SQLx's advisory migration
    // lock here, so use SQLx's direct-connection path deliberately.
    migrator
        .run_direct(conn)
        .await
        .context("db migrate failed")?;
    Ok(())
}

async fn migrate_while_sqlx_locked(conn: &mut PgConnection) -> Result<()> {
    let migration_table_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await
            .context("db migrate: inspect migration ledger presence failed")?;

    if !migration_table_exists {
        return run_embedded_migrations_without_lock(conn).await;
    }

    let migration_69_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM _sqlx_migrations
              WHERE version = 69
                AND success = true
         )",
    )
    .fetch_one(&mut *conn)
    .await
    .context("db migrate: inspect migration 0069 status failed")?;

    if migration_69_applied {
        return run_embedded_migrations_without_lock(conn).await;
    }

    let historical_tables_exist: bool = sqlx::query_scalar(
        "SELECT to_regclass('runs') IS NOT NULL
             AND to_regclass('runtime_leader_lease') IS NOT NULL",
    )
    .fetch_one(&mut *conn)
    .await
    .context("db migrate: inspect historical lease tables failed")?;

    if !historical_tables_exist {
        return run_embedded_migrations_without_lock(conn).await;
    }

    let mut tx = conn
        .begin()
        .await
        .context("db migrate: begin historical-upgrade fence transaction failed")?;

    // Production run lifecycle mutates `runs` before lease authority. Keep the
    // same order here. SHARE ROW EXCLUSIVE conflicts with ordinary DML and
    // with another fenced migrator, so no authority state can change between
    // this preflight and commit of the pending migration chain.
    sqlx::query("LOCK TABLE runs IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .context("db migrate: lock runs for historical-upgrade fence failed")?;

    sqlx::query("LOCK TABLE runtime_leader_lease IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .context("db migrate: lock runtime_leader_lease for historical-upgrade fence failed")?;

    let lease_has_run_id: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM information_schema.columns
              WHERE table_schema = ANY(current_schemas(false))
                AND table_name = 'runtime_leader_lease'
                AND column_name = 'run_id'
         )",
    )
    .fetch_one(&mut *tx)
    .await
    .context("db migrate: inspect runtime_leader_lease.run_id presence failed")?;

    let legacy_lease_exists: bool = if lease_has_run_id {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1
                   FROM runtime_leader_lease
                  WHERE id = 1
                    AND run_id IS NULL
             )",
        )
        .fetch_one(&mut *tx)
        .await
        .context("db migrate: inspect legacy unbound lease failed")?
    } else {
        // Before migration 0068 every extant singleton lease row is
        // necessarily legacy/unbound because the run_id column does not yet
        // exist.
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1
                   FROM runtime_leader_lease
                  WHERE id = 1
             )",
        )
        .fetch_one(&mut *tx)
        .await
        .context("db migrate: inspect pre-0068 legacy lease failed")?
    };

    if legacy_lease_exists {
        let active_authority_count: i64 = sqlx::query_scalar(
            "SELECT count(*)
               FROM runs
              WHERE status IN ('ARMED', 'RUNNING')",
        )
        .fetch_one(&mut *tx)
        .await
        .context("db migrate: inspect active run authority failed")?;

        if active_authority_count > 0 {
            tx.rollback()
                .await
                .context("db migrate: rollback historical-upgrade refusal failed")?;

            anyhow::bail!(
                "M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01:                  refusing to apply pending migration 0069 while a legacy/unbound                  runtime_leader_lease row exists and {active_authority_count} run(s)                  report ARMED or RUNNING. Heartbeat freshness is not authority evidence.                  Resolve active run lifecycle authority before retrying migration."
            );
        }
    }

    let migration_result = run_embedded_migrations_without_lock(&mut tx).await;

    if let Err(err) = migration_result {
        let _ = tx.rollback().await;
        return Err(err);
    }

    tx.commit()
        .await
        .context("db migrate: commit historical-upgrade fence transaction failed")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Disposable per-test database — true isolation for tests whose production
// query is inherently global/singleton (e.g. "the latest run for this
// engine", a bounded global feed window) and therefore cannot be isolated by
// per-fixture-ID cleanup against the shared `MQK_DATABASE_URL` database.
// FULL-AUDIT-FAIL-017.
//
// This is test-only infrastructure: it lives in `test_support`, gated so it
// compiles into mqk-db's own unit tests automatically (`cfg(test)`) and into
// an external crate's tests only when that crate's own `[dev-dependencies]`
// (never `[dependencies]`) enables the `testkit` feature on this crate. The
// default production `mqk-db` library API exposes none of this — see
// scripts/guards/check_disposable_db_not_in_production.sh.
// ---------------------------------------------------------------------------
#[cfg(any(test, feature = "testkit"))]
pub mod test_support;

#[cfg(any(test, feature = "testkit"))]
pub use test_support::{
    create_disposable_test_db, create_disposable_test_db_with_hooks, run_isolated, CleanupOutcome,
    DisposableDbError, DisposableDbTestHooks, DisposableTestDb, SetupCompletion, TestBarrier,
    TestObservations,
};

/// Simple status query (connectivity + schema presence).
pub async fn status(pool: &PgPool) -> Result<DbStatus> {
    let (one,): (i32,) = sqlx::query_as::<_, (i32,)>("select 1")
        .fetch_one(pool)
        .await
        .context("status connectivity query failed")?;
    let ok = one == 1;

    let (exists,): (bool,) = sqlx::query_as::<_, (bool,)>(
        r#"
        select exists (
            select 1
            from information_schema.tables
            where table_schema='public' and table_name='runs'
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .context("status table-exists query failed")?;

    Ok(DbStatus {
        ok,
        has_runs_table: exists,
    })
}

#[derive(Debug, Clone)]
pub struct DbStatus {
    pub ok: bool,
    pub has_runs_table: bool,
}
