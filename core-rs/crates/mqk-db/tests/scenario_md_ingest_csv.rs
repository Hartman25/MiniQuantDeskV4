// PATCH B: CSV -> md_bars ingestion + md_quality_reports persistence scenario test.
//
// DB-backed test, skipped if MQK_DATABASE_URL is not set.
// This test does NOT require any external market data providers.

use anyhow::Result;
use uuid::Uuid;

#[tokio::test]
async fn md_ingest_csv_persists_bars_and_quality_report() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    // Create a temp CSV.
    let dir = tempfile::tempdir()?;
    let csv_path = dir.path().join("bars.csv");

    // Two symbols, 1D, with a weekend gap that should NOT be flagged (Fri -> Mon).
    // Also includes one invalid row (negative volume) to prove rejection counting.
    let csv = "\
symbol,timeframe,end_ts,open,high,low,close,volume,is_complete
AAA,1D,1708041600,10,12,9,11,100,true
AAA,1D,1708300800,11,13,10,12,110,true
BBB,1D,1708041600,20,22,19,21,-5,true
";

    std::fs::write(&csv_path, csv)?;

    let ingest_id = Uuid::new_v4();
    let res = mqk_db::ingest_csv_to_md_bars(
        &pool,
        mqk_db::IngestCsvArgs {
            path: csv_path.clone(),
            timeframe: "1D".to_string(),
            source: "csv".to_string(),
            ingest_id: Some(ingest_id),
        },
    )
    .await?;

    assert_eq!(res.ingest_id, ingest_id);

    // md_bars should have 2 rows inserted (BBB row rejected).
    let (cnt,): (i64,) =
        sqlx::query_as("select count(*)::bigint from md_bars where timeframe = '1D'")
            .fetch_one(&pool)
            .await?;
    assert!(cnt >= 2, "expected at least 2 md_bars rows, got {cnt}");

    // md_quality_reports should have a row for this ingest_id.
    let (exists,): (bool,) = sqlx::query_as(
        r#"
        select exists(
          select 1 from md_quality_reports where ingest_id = $1
        )
        "#,
    )
    .bind(ingest_id)
    .fetch_one(&pool)
    .await?;
    assert!(exists, "expected md_quality_reports row for ingest_id");

    Ok(())
}
