//! Market-data ingestion command handlers.
//!
//! Covers `mqk md ingest-csv` and `mqk md ingest-provider`.
//! These are the data-pipeline paths used for backtesting workflows.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::fs;
use std::path::{Path, PathBuf};

/// Env-var name for the TwelveData API key (PATCH C).
const ENV_TWELVEDATA_API_KEY: &str = "TWELVEDATA_API_KEY";

// ---------------------------------------------------------------------------
// PATCH B — CSV ingestion
// ---------------------------------------------------------------------------

/// Execute `mqk md ingest-csv`: parse a CSV file and ingest into `md_bars`.
pub async fn md_ingest_csv(path: String, timeframe: String, source: String) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;

    let res = mqk_db::md::ingest_csv_to_md_bars(
        &pool,
        mqk_db::md::IngestCsvArgs {
            path: PathBuf::from(&path),
            timeframe: timeframe.clone(),
            source: source.clone(),
            ingest_id: None,
        },
    )
    .await
    .with_context(|| format!("ingest-csv failed for {}", path))?;

    let out_dir = Path::new("../exports")
        .join("md_ingest")
        .join(res.ingest_id.to_string());
    fs::create_dir_all(&out_dir).context("create md_ingest export dir failed")?;

    let out_path = out_dir.join("data_quality.json");
    let json = serde_json::to_string_pretty(&res.report).context("serialize report json failed")?;
    fs::write(&out_path, json)
        .with_context(|| format!("write report failed: {}", out_path.display()))?;

    println!("md_ingest_ok=true ingest_id={}", res.ingest_id);
    println!(
        "coverage rows_read={} rows_ok={} rows_rejected={} rows_inserted={} rows_updated={}",
        res.report.coverage.rows_read,
        res.report.coverage.rows_ok,
        res.report.coverage.rows_rejected,
        res.report.coverage.rows_inserted,
        res.report.coverage.rows_updated
    );
    println!("report_path={}", out_path.display());
    println!(
        "sql=select ingest_id, created_at, stats_json from md_quality_reports where ingest_id='{}';",
        res.ingest_id
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// PATCH C — Provider ingestion
// ---------------------------------------------------------------------------

/// Execute `mqk md ingest-provider`: fetch bars from a named provider and
/// ingest into `md_bars`.
pub async fn md_ingest_provider(
    source: String,
    symbols: String,
    timeframe: String,
    start: String,
    end: String,
) -> Result<()> {
    use mqk_md::HistoricalProvider;

    let source_lc = source.trim().to_ascii_lowercase();
    if source_lc != "twelvedata" {
        anyhow::bail!("unsupported --source '{}'. supported: twelvedata", source);
    }

    let syms: Vec<String> = symbols
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if syms.is_empty() {
        anyhow::bail!("--symbols must contain at least one symbol");
    }

    let tf = mqk_md::Timeframe::parse(&timeframe)?;
    let start_d = NaiveDate::parse_from_str(start.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid --start date: {}", start))?;
    let end_d = NaiveDate::parse_from_str(end.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid --end date: {}", end))?;
    if end_d < start_d {
        anyhow::bail!("--end must be >= --start");
    }

    let api_key = std::env::var(ENV_TWELVEDATA_API_KEY)
        .with_context(|| format!("missing env var {ENV_TWELVEDATA_API_KEY}"))?;

    let provider = mqk_md::TwelveDataHistoricalProvider::new(api_key);

    let req = mqk_md::FetchBarsRequest {
        symbols: syms.clone(),
        timeframe: tf,
        start: start_d,
        end: end_d,
    };

    let raw = provider.fetch_bars(req).await?;

    let bars: Vec<mqk_db::md::ProviderBar> = raw
        .into_iter()
        .map(|b| mqk_db::md::ProviderBar {
            symbol: b.symbol,
            timeframe: b.timeframe,
            end_ts: b.end_ts,
            open: b.open,
            high: b.high,
            low: b.low,
            close: b.close,
            volume: b.volume,
            is_complete: b.is_complete,
        })
        .collect();

    let pool = mqk_db::connect_from_env().await?;

    let res = mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: source_lc.clone(),
            timeframe: tf.as_str().to_string(),
            ingest_id: None,
            bars,
        },
    )
    .await?;

    let out_dir = Path::new("../exports/md_ingest").join(res.ingest_id.to_string());
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    let report_path = out_dir.join("data_quality.json");
    let report_json = serde_json::to_string_pretty(&res.report).context("serialize report")?;
    fs::write(&report_path, report_json)
        .with_context(|| format!("write {} failed", report_path.display()))?;

    println!("ingest_id={}", res.ingest_id);
    println!("source={}", source_lc);
    println!("timeframe={}", tf.as_str());
    println!("symbols={}", syms.join(","));
    println!(
        "rows_read={} rows_ok={} rejected={} inserted={} updated={}",
        res.report.coverage.rows_read,
        res.report.coverage.rows_ok,
        res.report.coverage.rows_rejected,
        res.report.coverage.rows_inserted,
        res.report.coverage.rows_updated
    );
    println!("artifact={}", report_path.display());
    println!(
        "sql=select ingest_id, created_at, stats_json from md_quality_reports where ingest_id='{}';",
        res.ingest_id
    );

    Ok(())
}
