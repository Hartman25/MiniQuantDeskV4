// core-rs/crates/mqk-db/src/lib.rs
//
// Thin crate root. All domain logic lives in the modules below.
// This file owns only:
//   - the TimeSource abstraction (shared by all modules)
//   - connection / migration helpers (crate-level infrastructure)
//   - re-exports that preserve the pre-refactor public API
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPoolOptions, PgPool};

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
pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("db migrate failed")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Disposable per-test database — true isolation for tests whose production
// query is inherently global/singleton (e.g. "the latest run for this
// engine", a bounded global feed window) and therefore cannot be isolated by
// per-fixture-ID cleanup against the shared `MQK_DATABASE_URL` database.
// FULL-AUDIT-FAIL-017.
// ---------------------------------------------------------------------------

/// Splits a Postgres URL into `(everything before the final path segment,
/// database name, optional query string)`. Pure string manipulation — every
/// URL used by this repo's test harness (`MQK_DATABASE_URL`) is a plain
/// `postgres://user:pass@host:port/dbname[?params]` with no path segments
/// beyond the database name.
fn split_db_url(url: &str) -> (String, String, Option<String>) {
    let (path_part, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (url.to_string(), None),
    };
    let idx = path_part
        .rfind('/')
        .expect("MQK_DATABASE_URL must contain a /<dbname> path segment");
    (
        path_part[..idx].to_string(),
        path_part[idx + 1..].to_string(),
        query,
    )
}

fn build_db_url(base: &str, db_name: &str, query: &Option<String>) -> String {
    match query {
        Some(q) => format!("{base}/{db_name}?{q}"),
        None => format!("{base}/{db_name}"),
    }
}

/// A disposable, migrated Postgres database created for exactly one test
/// (or one test's own private scope), on the same server as
/// `MQK_DATABASE_URL`. Never touches any other test's rows because nothing
/// else connects to it. Call [`DisposableTestDb::drop_database`] when done —
/// [`run_isolated`] does this automatically, including on test panic.
pub struct DisposableTestDb {
    pub pool: PgPool,
    db_name: String,
    admin_url: String,
}

impl DisposableTestDb {
    /// Drops the disposable database. Must be called after `self.pool` (and
    /// any clone of it) is done being used — this closes `self.pool` first.
    pub async fn drop_database(self) -> Result<()> {
        self.pool.close().await;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
            .context("connect to admin db for disposable-db teardown")?;
        sqlx::query(&format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        ))
        .execute(&admin_pool)
        .await
        .context("drop disposable test database")?;
        admin_pool.close().await;
        Ok(())
    }
}

/// Creates a fresh, migrated, uniquely-named database on the same Postgres
/// server as `MQK_DATABASE_URL` (port 5434 in this repo's test harness), and
/// returns a pool connected to it. `label` is sanitized to `[a-z0-9_]` and
/// truncated so the generated name stays within Postgres' 63-byte identifier
/// limit; the OS process ID plus a per-process monotonic counter guarantee
/// uniqueness across concurrent test runs (deterministic inputs only --
/// `Uuid::new_v4()` is disallowed in this crate's production src/ by
/// scripts/guards/check_unsafe_patterns.ps1/.sh, so this does not use it).
static DISPOSABLE_DB_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub async fn create_disposable_test_db(label: &str) -> Result<DisposableTestDb> {
    let base_url =
        std::env::var(ENV_DB_URL).with_context(|| format!("missing env var {ENV_DB_URL}"))?;
    let (base, _orig_db, query) = split_db_url(&base_url);

    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let sanitized: String = sanitized.chars().take(24).collect();
    let sequence = DISPOSABLE_DB_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let db_name = format!("mqk_disp_{sanitized}_{}_{sequence}", std::process::id());

    let admin_url = build_db_url(&base, "postgres", &query);
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .context("connect to admin db to create disposable test database")?;
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .context("create disposable test database")?;
    admin_pool.close().await;

    let target_url = build_db_url(&base, &db_name, &query);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&target_url)
        .await
        .context("connect to newly-created disposable test database")?;
    migrate(&pool)
        .await
        .context("migrate disposable test database")?;

    Ok(DisposableTestDb {
        pool,
        db_name,
        admin_url,
    })
}

/// Runs `test` against a fresh disposable database created just for this
/// call, and guarantees the database is dropped afterward — including when
/// `test` panics (an assertion failure), by running it inside `tokio::spawn`
/// and catching the join error before propagating the original panic.
/// This is the FULL-AUDIT-FAIL-017 replacement for global shared-DB deletion
/// helpers and `Utc::now()`-based "latest row" racing: each caller gets an
/// empty database, so there is no other test's row to collide with and no
/// global query to race.
pub async fn run_isolated<F, Fut>(label: &str, test: F)
where
    F: FnOnce(PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let disposable = create_disposable_test_db(label)
        .await
        .expect("create_disposable_test_db");
    let pool = disposable.pool.clone();
    let handle = tokio::spawn(async move { test(pool).await });
    let result = handle.await;
    disposable
        .drop_database()
        .await
        .expect("drop_database (disposable test db teardown)");
    if let Err(join_err) = result {
        match join_err.try_into_panic() {
            Ok(panic_payload) => std::panic::resume_unwind(panic_payload),
            Err(join_err) => {
                panic!("run_isolated: test task did not panic but failed to join: {join_err}")
            }
        }
    }
}

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
