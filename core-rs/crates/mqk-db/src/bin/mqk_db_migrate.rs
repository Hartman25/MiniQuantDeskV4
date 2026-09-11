//! Canonical production migration runner.
//!
//! M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01:
//! official operator scripts invoke this binary instead of calling raw
//! `sqlx migrate run`, ensuring every supported migration path uses
//! `mqk_db::migrate` and its historical 0069 upgrade fence.

#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let result = mqk_db::migrate(&pool).await;
    pool.close().await;
    result
}
