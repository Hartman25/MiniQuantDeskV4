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
