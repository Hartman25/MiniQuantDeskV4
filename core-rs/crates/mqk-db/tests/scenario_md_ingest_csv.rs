// PATCH B: CSV -> md_bars ingestion + md_quality_reports persistence scenario test.
//
// DB-backed test, skipped if MQK_DATABASE_URL is not set.
// This test does NOT require any external market data providers.

use anyhow::Result;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_csv_persists_bars_and_quality_report() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
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
            ingest_id,
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

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01F: CSV ingest must stamp truthful provider metadata instead of
// falling through to the metadata-less helper's "unknown" default.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_csv_stamps_source_as_provider_id_not_unknown() -> Result<()> {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("MQK_DATABASE_URL required for this test");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let symbol = "CRYPTO01F_PROVENANCE_ONE";
    let timeframe = "1D";
    let source_label = "local_crypto_manual_01f";

    // Clean slate.
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(&pool)
        .await?;

    let dir = tempfile::tempdir()?;
    let csv_path = dir.path().join("bars.csv");
    let csv = format!(
        "symbol,timeframe,end_ts,open,high,low,close,volume,is_complete\n\
         {symbol},1D,1767312000,42000.00,43000.00,41000.00,42500.00,1234567,true\n"
    );
    std::fs::write(&csv_path, csv)?;

    let ingest_id = Uuid::new_v4();

    let res = mqk_db::ingest_csv_to_md_bars(
        &pool,
        mqk_db::IngestCsvArgs {
            path: csv_path.clone(),
            timeframe: timeframe.to_string(),
            source: source_label.to_string(),
            ingest_id,
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_inserted, 1);
    // Quality report source must remain exactly what the operator supplied
    // (existing behavior, unchanged by this patch).
    assert_eq!(res.report.source, source_label);

    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        r#"
        select provider_id, provider_source, ingest_mode
        from md_bars
        where symbol = $1 and timeframe = $2
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .fetch_one(&pool)
    .await?;

    assert_eq!(
        row.0, source_label,
        "provider_id must be the operator --source label, not 'unknown'"
    );
    assert_eq!(row.1.as_deref(), Some(source_label));
    assert_eq!(row.2.as_deref(), Some("csv_import"));

    // Re-run the same ingest (idempotent upsert): still one row, provider
    // metadata unchanged, no duplicate insert.
    let res2 = mqk_db::ingest_csv_to_md_bars(
        &pool,
        mqk_db::IngestCsvArgs {
            path: csv_path,
            timeframe: timeframe.to_string(),
            source: source_label.to_string(),
            ingest_id,
        },
    )
    .await?;
    assert_eq!(res2.report.coverage.rows_inserted, 0);
    assert_eq!(res2.report.coverage.rows_updated, 1);

    let (cnt,): (i64,) =
        sqlx::query_as("select count(*)::bigint from md_bars where symbol = $1 and timeframe = $2")
            .bind(symbol)
            .bind(timeframe)
            .fetch_one(&pool)
            .await?;
    assert_eq!(cnt, 1, "re-ingest must not duplicate rows");

    // Cleanup.
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(&pool)
        .await?;
    sqlx::query("delete from md_quality_reports where ingest_id = $1")
        .bind(ingest_id)
        .execute(&pool)
        .await?;

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_csv_blank_source_fails_closed_to_unknown_provider_id() -> Result<()> {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("MQK_DATABASE_URL required for this test");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let symbol = "CRYPTO01F_PROVENANCE_BLANK";
    let timeframe = "1D";

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(&pool)
        .await?;

    let dir = tempfile::tempdir()?;
    let csv_path = dir.path().join("bars.csv");
    let csv = format!(
        "symbol,timeframe,end_ts,open,high,low,close,volume,is_complete\n\
         {symbol},1D,1767312000,10,12,9,11,100,true\n"
    );
    std::fs::write(&csv_path, csv)?;

    let ingest_id = Uuid::new_v4();

    mqk_db::ingest_csv_to_md_bars(
        &pool,
        mqk_db::IngestCsvArgs {
            path: csv_path,
            timeframe: timeframe.to_string(),
            source: "   ".to_string(), // blank/whitespace-only source
            ingest_id,
        },
    )
    .await?;

    let (provider_id,): (String,) =
        sqlx::query_as("select provider_id from md_bars where symbol = $1 and timeframe = $2")
            .bind(symbol)
            .bind(timeframe)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        provider_id, "unknown",
        "blank --source must still fail closed to 'unknown', not an empty string"
    );

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(&pool)
        .await?;
    sqlx::query("delete from md_quality_reports where ingest_id = $1")
        .bind(ingest_id)
        .execute(&pool)
        .await?;

    Ok(())
}
