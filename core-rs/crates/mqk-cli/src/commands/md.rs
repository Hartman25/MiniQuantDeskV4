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

/// Maximum calendar days per provider fetch window for DAILY bars.
///
/// 8 × 365 = 2 920 days ≈ 8 years.  Keeps TwelveData request sizes manageable
/// for deep-history backfills (e.g. 1993-01-01 → today spans ~33 years → ~5 chunks).
const CHUNK_DAYS_1D: i64 = 8 * 365;

/// Maximum calendar days per provider fetch window for 1-minute bars.
///
/// TwelveData caps responses at ~5 000 bars. At 390 bars/trading-day and
/// ~0.71 trading-days/calendar-day, 14 calendar days ≈ 10 trading days ≈ 3 900 bars.
/// Stays safely under the cap with room for half-days and pre-market gaps.
const CHUNK_DAYS_1M: i64 = 14;

/// Maximum calendar days per provider fetch window for 5-minute bars.
///
/// 5-minute bars: 78 bars/trading-day.  63 calendar days ≈ 45 trading days ≈ 3 510 bars.
/// Stays safely under the TwelveData ~5 000-bar cap.
const CHUNK_DAYS_5M: i64 = 63;

// ---------------------------------------------------------------------------
// Date-range chunking helper
// ---------------------------------------------------------------------------

/// Split the inclusive date range `[start, end]` into fixed-size windows of
/// at most `chunk_days` calendar days each.
///
/// Returns a `Vec<(chunk_start, chunk_end)>` where every pair is inclusive and
/// the windows together cover the full range without gaps or overlap.
///
/// Properties:
/// - Deterministic: no wall-clock use, no randomness.
/// - `chunks.first().0 == start` and `chunks.last().1 == end`.
/// - Consecutive chunks satisfy `prev_end + 1 day == next_start`.
/// - Returns an empty `Vec` when `start > end` or `chunk_days <= 0`.
pub fn chunk_date_range(
    start: NaiveDate,
    end: NaiveDate,
    chunk_days: i64,
) -> Vec<(NaiveDate, NaiveDate)> {
    let mut chunks = Vec::new();
    if start > end || chunk_days <= 0 {
        return chunks;
    }
    let mut cur = start;
    loop {
        let candidate = cur + Duration::days(chunk_days - 1);
        let chunk_end = if candidate > end { end } else { candidate };
        chunks.push((cur, chunk_end));
        if chunk_end >= end {
            break;
        }
        cur = chunk_end + Duration::days(1);
    }
    chunks
}

// ---------------------------------------------------------------------------
// CSV backup helper
// ---------------------------------------------------------------------------

/// Write normalized provider bars to a deterministic CSV artifact.
///
/// Columns: `symbol,timeframe,end_ts,open,high,low,close,volume,is_complete`.
/// Rows are written in the order of the input slice, which must already be sorted
/// by `(symbol ASC, end_ts ASC)` by the caller.
///
/// Called before DB ingest so the CSV captures what was fetched regardless of
/// whether the subsequent ingest succeeds.
fn write_provider_bars_csv(path: &Path, bars: &[mqk_db::md::ProviderBar]) -> Result<()> {
    let mut content =
        String::from("symbol,timeframe,end_ts,open,high,low,close,volume,is_complete\n");
    for b in bars {
        content.push_str(&format!(
            "{},{},{},{},{},{},{},{},{}\n",
            b.symbol,
            b.timeframe,
            b.end_ts,
            b.open,
            b.high,
            b.low,
            b.close,
            b.volume,
            if b.is_complete { "true" } else { "false" },
        ));
    }
    fs::write(path, &content)
        .with_context(|| format!("write_provider_bars_csv failed: {}", path.display()))
}

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
// PATCH C — Provider ingestion (with chunked fetching + CSV backup artifacts)
// ---------------------------------------------------------------------------

/// Execute `mqk md ingest-provider`: fetch bars from a named provider and
/// ingest into `md_bars`.
///
/// For long date ranges the fetch is automatically chunked into fixed-size windows:
/// - 1D bars: 8-year windows (2 920 calendar days)
/// - 1m bars: 14-day windows (~10 trading days, ≤3 900 bars, under TwelveData cap)
/// - 5m bars: 63-day windows (~45 trading days, ≤3 510 bars, under TwelveData cap)
///
/// After all chunks are collected, bars are sorted ascending by `(symbol, end_ts)`
/// so that the DB ingest layer's per-symbol monotonicity check is never tripped by
/// cross-chunk ordering.
///
/// A `provider_bars.csv` backup artifact is written before DB ingest so the raw
/// normalized fetch is preserved independently of DB state.
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

    // D1-5: deterministic UUIDv5 from (source, timeframe, symbols, date range).
    let ingest_id = Uuid::new_v5(
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
    );

    let provider = mqk_md::TwelveDataHistoricalProvider::new(api_key);

    // Determine chunk window size based on timeframe to stay under TwelveData's
    // per-request bar cap (~5 000 bars) on deep-history backfills.
    let chunk_days = match tf {
        mqk_md::Timeframe::D1 => CHUNK_DAYS_1D,
        mqk_md::Timeframe::M5 => CHUNK_DAYS_5M,
        mqk_md::Timeframe::M1 => CHUNK_DAYS_1M,
    };
    let chunks = chunk_date_range(start_d, end_d, chunk_days);

    // Fetch each chunk sequentially. Prices are normalized provider-side in
    // fetch_bars (normalize_price_str: truncate to ≤6 decimals, trim trailing zeros).
    let mut all_raw: Vec<mqk_md::ProviderBar> = Vec::new();
    for (chunk_start, chunk_end) in &chunks {
        let req = mqk_md::FetchBarsRequest {
            symbols: syms.clone(),
            timeframe: tf,
            start: *chunk_start,
            end: *chunk_end,
        };
        let chunk_bars = provider.fetch_bars(req).await.with_context(|| {
            format!("provider fetch failed for chunk {chunk_start}..{chunk_end}")
        })?;
        all_raw.extend(chunk_bars);
    }

    // Sort all collected bars by (symbol ASC, end_ts ASC) to ensure the DB ingest
    // layer's per-symbol monotonicity check is satisfied across chunk boundaries.
    all_raw.sort_by(|a, b| {
        a.symbol
            .cmp(&b.symbol)
            .then_with(|| a.end_ts.cmp(&b.end_ts))
    });

    // Convert mqk_md::ProviderBar → mqk_db::md::ProviderBar.
    // Prices are already normalized by fetch_bars; no additional transformation needed.
    let bars: Vec<mqk_db::md::ProviderBar> = all_raw
        .iter()
        .map(|b| mqk_db::md::ProviderBar {
            symbol: b.symbol.clone(),
            timeframe: b.timeframe.clone(),
            end_ts: b.end_ts,
            open: b.open.clone(),
            high: b.high.clone(),
            low: b.low.clone(),
            close: b.close.clone(),
            volume: b.volume,
            is_complete: b.is_complete,
        })
        .collect();

    // Create artifact directory early (ingest_id is deterministic, available now).
    let out_dir = Path::new("../exports/md_ingest").join(ingest_id.to_string());
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    // Write CSV backup before DB ingest: captures the normalized fetch regardless
    // of DB ingest outcome.
    let csv_path = out_dir.join("provider_bars.csv");
    write_provider_bars_csv(&csv_path, &bars)?;

    // DB ingest.
    let pool = mqk_db::connect_from_env().await?;
    let res = mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: source_lc.clone(),
            timeframe: tf.as_str().to_string(),
            ingest_id,
            bars,
        },
    )
    .await?;

    // Write quality report artifact.
    let report_path = out_dir.join("data_quality.json");
    let report_json = serde_json::to_string_pretty(&res.report).context("serialize report")?;
    fs::write(&report_path, report_json)
        .with_context(|| format!("write {} failed", report_path.display()))?;

    // Operator output.
    println!("ingest_id={}", res.ingest_id);
    println!("source={}", source_lc);
    println!("timeframe={}", tf.as_str());
    println!("symbols={}", syms.join(","));
    println!("chunks={}", chunks.len());
    println!(
        "rows_read={} rows_ok={} rejected={} inserted={} updated={}",
        res.report.coverage.rows_read,
        res.report.coverage.rows_ok,
        res.report.coverage.rows_rejected,
        res.report.coverage.rows_inserted,
        res.report.coverage.rows_updated
    );
    println!("csv_artifact={}", csv_path.display());
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // chunk_date_range
    // -----------------------------------------------------------------------

    #[test]
    fn chunk_single_window_when_range_fits_in_one_chunk() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        let chunks = chunk_date_range(start, end, 30);
        assert_eq!(
            chunks.len(),
            1,
            "short range must produce exactly one chunk"
        );
        assert_eq!(chunks[0].0, start);
        assert_eq!(chunks[0].1, end);
    }

    #[test]
    fn chunk_splits_at_exact_multiple() {
        // 10-day range with chunk_days=5 → exactly 2 chunks: [1-5], [6-10]
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        let chunks = chunk_date_range(start, end, 5);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].0, start);
        assert_eq!(chunks[0].1, NaiveDate::from_ymd_opt(2024, 1, 5).unwrap());
        assert_eq!(chunks[1].0, NaiveDate::from_ymd_opt(2024, 1, 6).unwrap());
        assert_eq!(chunks[1].1, end);
    }

    #[test]
    fn chunk_no_gaps_full_range_covered() {
        // Deep-history range that produces multiple 1D chunks.
        // Verify: first chunk starts at start, last chunk ends at end,
        // and each consecutive pair has no gap (prev_end + 1 == next_start).
        let start = NaiveDate::from_ymd_opt(1993, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 3, 17).unwrap();
        let chunks = chunk_date_range(start, end, CHUNK_DAYS_1D);

        assert!(!chunks.is_empty());
        assert_eq!(
            chunks.first().unwrap().0,
            start,
            "first chunk must start at start"
        );
        assert_eq!(chunks.last().unwrap().1, end, "last chunk must end at end");

        for window in chunks.windows(2) {
            let (_, prev_end) = window[0];
            let (next_start, _) = window[1];
            assert_eq!(
                prev_end + Duration::days(1),
                next_start,
                "gap between chunks: {} and {}",
                prev_end,
                next_start
            );
        }
    }

    #[test]
    fn chunk_last_chunk_does_not_exceed_end() {
        // Range of 5 days, chunk of 3 → [1-3], [4-5]
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();
        let chunks = chunk_date_range(start, end, 3);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[1].1, end,
            "last chunk end must equal requested end, not exceed it"
        );
    }

    #[test]
    fn chunk_returns_empty_when_start_after_end() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let chunks = chunk_date_range(start, end, 30);
        assert!(chunks.is_empty(), "start > end must produce no chunks");
    }

    // -----------------------------------------------------------------------
    // write_provider_bars_csv
    // -----------------------------------------------------------------------

    #[test]
    fn write_provider_bars_csv_header_and_rows_deterministic() {
        // Write two bars to a temp file and verify header + row content exactly.
        let path =
            std::env::temp_dir().join(format!("mqk_test_provider_bars_{}.csv", std::process::id()));

        let bars = vec![
            mqk_db::md::ProviderBar {
                symbol: "SPY".to_string(),
                timeframe: "1D".to_string(),
                end_ts: 1_708_041_600,
                open: "100.5".to_string(),
                high: "101".to_string(),
                low: "99.5".to_string(),
                close: "100.75".to_string(),
                volume: 1000,
                is_complete: true,
            },
            mqk_db::md::ProviderBar {
                symbol: "SPY".to_string(),
                timeframe: "1D".to_string(),
                end_ts: 1_708_128_000,
                open: "100.75".to_string(),
                high: "102".to_string(),
                low: "100".to_string(),
                close: "101.5".to_string(),
                volume: 1200,
                is_complete: false,
            },
        ];

        write_provider_bars_csv(&path, &bars).expect("write_provider_bars_csv must succeed");

        let content = fs::read_to_string(&path).expect("read written CSV");
        let _ = fs::remove_file(&path); // cleanup; ignore error if already gone

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines[0], "symbol,timeframe,end_ts,open,high,low,close,volume,is_complete",
            "header must match canonical column order"
        );
        assert_eq!(lines.len(), 3, "1 header + 2 data rows");
        assert_eq!(
            lines[1],
            "SPY,1D,1708041600,100.5,101,99.5,100.75,1000,true"
        );
        assert_eq!(
            lines[2],
            "SPY,1D,1708128000,100.75,102,100,101.5,1200,false"
        );
    }
}
