//! Market-data command handlers.
//!
//! Covers `mqk md ingest-csv`, `mqk md ingest-provider`, and `mqk md sync-provider`.
//! These handlers implement the market-data CLI surface and keep MD sync logic out of unrelated backtest modules.

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
            // D1-5: deterministic UUIDv5 from (source, path, timeframe); no random UUID in src/.
            ingest_id: Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!("mqk-md-ingest.csv.v1|{}|{}|{}", source, path, timeframe).as_bytes(),
            ),
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
            // D1-5: deterministic UUIDv5 from (source, timeframe, symbols, date range); no random UUID in src/.
            ingest_id: Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!(
                    "mqk-md-ingest.provider.v1|{}|{}|{}|{}|{}",
                    source_lc,
                    tf.as_str(),
                    syms.join(","),
                    start,
                    end
                )
                .as_bytes(),
            ),
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

// ---------------------------------------------------------------------------
// sync-provider — incremental historical market-data sync
// ---------------------------------------------------------------------------

/// Execute `mqk md sync-provider`: per-symbol incremental sync from a provider into `md_bars`.
///
/// For each symbol the latest stored `end_ts` is fetched from the DB.
/// - No bars exist → `--full-start` is required; the full date range is fetched.
/// - Bars exist → effective start = latest bar date − overlap window; only new bars are fetched.
///
/// All per-symbol bars are batched into a single `ingest_provider_bars_to_md_bars` call so the
/// quality report and DB upsert are atomic at the ingest level.
///
/// Wall clock (`Utc::now()`) is used only here, in this operator command, to default `--end`.
/// No wall-clock use is introduced in deterministic src/ paths.
pub async fn md_sync_provider(
    source: String,
    symbols: String,
    timeframe: String,
    full_start: Option<String>,
    end: Option<String>,
    overlap_days: Option<u32>,
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

    // Wall clock is acceptable here — this is an operator CLI command, not a deterministic path.
    let end_d = match &end {
        Some(s) => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .with_context(|| format!("invalid --end date: {}", s))?,
        None => Utc::now().date_naive(),
    };

    let default_overlap: u32 = match tf {
        mqk_md::Timeframe::D1 => 5,
        mqk_md::Timeframe::M5 => 2,
        mqk_md::Timeframe::M1 => 1,
    };
    let overlap = overlap_days.unwrap_or(default_overlap);

    let pool = mqk_db::connect_from_env().await?;

    // --- Per-symbol effective start detection ---
    let mut sym_start_pairs: Vec<(String, NaiveDate)> = Vec::new();
    for sym in &syms {
        let effective_start = match mqk_db::md::latest_stored_bar_end_ts(&pool, sym, tf.as_str())
            .await?
        {
            None => {
                // No bars exist: full_start is mandatory.
                let fs = full_start.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "no bars found for {}/{} — provide --full-start YYYY-MM-DD for initial backfill",
                            sym,
                            tf.as_str()
                        )
                    })?;
                NaiveDate::parse_from_str(fs.trim(), "%Y-%m-%d")
                    .with_context(|| format!("invalid --full-start date: {}", fs))?
            }
            Some(latest_end_ts) => {
                // Bars exist: start = latest bar date − overlap window.
                let latest_dt = chrono::DateTime::<Utc>::from_timestamp(latest_end_ts, 0)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "invalid end_ts {} stored for {}/{}",
                            latest_end_ts,
                            sym,
                            tf.as_str()
                        )
                    })?;
                latest_dt.date_naive() - Duration::days(i64::from(overlap))
            }
        };
        sym_start_pairs.push((sym.clone(), effective_start));
    }

    // --- Deterministic ingest_id from (source, tf, per-symbol start pairs, end_d) ---
    let pairs_str: Vec<String> = sym_start_pairs
        .iter()
        .map(|(s, d)| format!("{}:{}", s, d))
        .collect();
    let ingest_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-sync.provider.v1|{}|{}|{}|{}",
            source_lc,
            tf.as_str(),
            pairs_str.join("|"),
            end_d
        )
        .as_bytes(),
    );

    let api_key = std::env::var(ENV_TWELVEDATA_API_KEY)
        .with_context(|| format!("missing env var {ENV_TWELVEDATA_API_KEY}"))?;
    let provider = mqk_md::TwelveDataHistoricalProvider::new(api_key);

    // --- Fetch per-symbol, collect all bars into one batch ---
    let mut all_bars: Vec<mqk_db::md::ProviderBar> = Vec::new();
    for (sym, effective_start) in &sym_start_pairs {
        if effective_start > &end_d {
            // Symbol already up-to-date; overlap window extends past end_d.
            println!(
                "symbol={} status=already_current effective_start={}",
                sym, effective_start
            );
            continue;
        }
        let req = mqk_md::FetchBarsRequest {
            symbols: vec![sym.clone()],
            timeframe: tf,
            start: *effective_start,
            end: end_d,
        };
        let raw = provider.fetch_bars(req).await?;
        for b in raw {
            all_bars.push(mqk_db::md::ProviderBar {
                symbol: b.symbol,
                timeframe: b.timeframe,
                end_ts: b.end_ts,
                open: b.open,
                high: b.high,
                low: b.low,
                close: b.close,
                volume: b.volume,
                is_complete: b.is_complete,
            });
        }
    }

    // --- Ingest all bars in one atomic quality-report pass ---
    let res = mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: source_lc.clone(),
            timeframe: tf.as_str().to_string(),
            ingest_id,
            bars: all_bars,
        },
    )
    .await?;

    // --- Write quality report artifact ---
    let out_dir = Path::new("../exports/md_ingest").join(res.ingest_id.to_string());
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let report_path = out_dir.join("data_quality.json");
    let report_json = serde_json::to_string_pretty(&res.report).context("serialize report")?;
    fs::write(&report_path, report_json)
        .with_context(|| format!("write {} failed", report_path.display()))?;

    // --- Operator-visible output ---
    println!("mode=sync-provider");
    println!("ingest_id={}", res.ingest_id);
    println!("source={}", source_lc);
    println!("timeframe={}", tf.as_str());
    for (sym, effective_start) in &sym_start_pairs {
        println!("symbol={} effective_start={}", sym, effective_start);
    }
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
