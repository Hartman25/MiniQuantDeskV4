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

/// Maximum calendar days per provider fetch window for 1-hour bars.
///
/// Hourly bars: ~7 bars/trading-day (regular NYSE session, including the final
/// partial hour). 365 calendar days ≈ 259 trading days ≈ 1 813 bars.
/// Stays safely under the TwelveData/Alpaca ~5 000-bar cap with room to spare.
const CHUNK_DAYS_1H: i64 = 365;

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

fn fmt_opt_i64(v: Option<i64>) -> String {
    v.map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn fmt_opt_bool(v: Option<bool>) -> String {
    v.map(|b| b.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn print_completed_bar_filter_report(source: &str, report: &mqk_md::CompletedBarFilterReport) {
    println!(
        "completed_bar_filter source={} timeframe={} now_ts={} timeframe_secs={} completed_cutoff_ts={} rows_in={} rows_kept={} dropped_incomplete_flag={} dropped_current={} latest_completed_end_ts={}",
        source,
        report.timeframe,
        report.now_ts,
        report.timeframe_secs,
        fmt_opt_i64(report.completed_cutoff_ts),
        report.rows_in,
        report.rows_kept,
        report.rows_dropped_incomplete_flag,
        report.rows_dropped_current,
        fmt_opt_i64(report.latest_completed_end_ts),
    );
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
// Symbol resolution helper
// ---------------------------------------------------------------------------

/// Resolve the symbol list from exactly one of `--symbols` or `--symbols-from-registry`.
///
/// Rules:
/// - Providing both is an error (mutually exclusive).
/// - Providing neither is an error.
/// - `--symbols`: comma-separated list; must contain at least one non-empty token.
/// - `--symbols-from-registry`: load the registry JSON, validate it, and return
///   all enabled equity `provider_symbol` values in deterministic alphabetical order.
pub fn resolve_symbols(
    symbols: Option<&str>,
    symbols_from_registry: Option<&std::path::Path>,
) -> Result<Vec<String>> {
    match (symbols, symbols_from_registry) {
        (Some(_), Some(_)) => {
            anyhow::bail!(
                "--symbols and --symbols-from-registry are mutually exclusive; provide only one"
            )
        }
        (None, None) => {
            anyhow::bail!("one of --symbols or --symbols-from-registry is required")
        }
        (Some(csv), None) => {
            let syms: Vec<String> = csv
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            if syms.is_empty() {
                anyhow::bail!("--symbols must contain at least one symbol");
            }
            Ok(syms)
        }
        (None, Some(path)) => {
            let instruments = mqk_md::instrument_registry::load_instrument_registry(path)
                .with_context(|| format!("failed to load registry: {}", path.display()))?;
            mqk_md::instrument_registry::validate_registry(&instruments)
                .with_context(|| format!("registry validation failed: {}", path.display()))?;
            let syms = mqk_md::instrument_registry::enabled_equity_symbols(&instruments);
            if syms.is_empty() {
                anyhow::bail!(
                    "registry at {} contains no enabled equity instruments",
                    path.display()
                );
            }
            Ok(syms)
        }
    }
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
/// A `provider_bars.csv` backup artifact is written before DB ingest so the
/// completed, normalized provider rows selected for ingest are preserved
/// independently of DB state.
pub async fn md_ingest_provider(
    source: String,
    symbols: Option<String>,
    symbols_from_registry: Option<PathBuf>,
    timeframe: String,
    start: String,
    end: String,
) -> Result<()> {
    let source_lc = source.trim().to_ascii_lowercase();
    if source_lc != "twelvedata" && source_lc != "alpaca" {
        anyhow::bail!(
            "unsupported --source '{}'. supported: twelvedata, alpaca",
            source
        );
    }

    let syms = resolve_symbols(symbols.as_deref(), symbols_from_registry.as_deref())?;

    let tf = mqk_md::Timeframe::parse(&timeframe)?;
    let start_d = NaiveDate::parse_from_str(start.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid --start date: {}", start))?;
    let end_d = NaiveDate::parse_from_str(end.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid --end date: {}", end))?;
    if end_d < start_d {
        anyhow::bail!("--end must be >= --start");
    }

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

    let provider: Box<dyn mqk_md::HistoricalProvider> = if source_lc == "alpaca" {
        let (key, secret) = mqk_md::load_alpaca_paper_credentials()?;
        Box::new(mqk_md::AlpacaHistoricalProvider::new(key, secret))
    } else {
        let api_key = std::env::var(ENV_TWELVEDATA_API_KEY)
            .with_context(|| format!("missing env var {ENV_TWELVEDATA_API_KEY}"))?;
        Box::new(mqk_md::TwelveDataHistoricalProvider::new(api_key))
    };

    // Determine chunk window size based on timeframe to stay under TwelveData's
    // per-request bar cap (~5 000 bars) on deep-history backfills.
    let chunk_days = match tf {
        mqk_md::Timeframe::D1 => CHUNK_DAYS_1D,
        mqk_md::Timeframe::H1 => CHUNK_DAYS_1H,
        mqk_md::Timeframe::M15 => CHUNK_DAYS_5M,
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

    let (completed_raw, completion_report) =
        mqk_md::filter_completed_provider_bars(all_raw, tf, Utc::now().timestamp());
    print_completed_bar_filter_report(&source_lc, &completion_report);

    // Convert mqk_md::ProviderBar → mqk_db::md::ProviderBar.
    // Prices are already normalized by fetch_bars; no additional transformation needed.
    let bars: Vec<mqk_db::md::ProviderBar> = completed_raw
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

    // Write CSV backup before DB ingest: captures the completed filtered provider
    // rows regardless of DB ingest outcome.
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
    symbols: Option<String>,
    symbols_from_registry: Option<PathBuf>,
    timeframe: String,
    full_start: Option<String>,
    end: Option<String>,
    overlap_days: Option<u32>,
) -> Result<()> {
    let source_lc = source.trim().to_ascii_lowercase();
    if source_lc != "twelvedata" && source_lc != "alpaca" {
        anyhow::bail!(
            "unsupported --source '{}'. supported: twelvedata, alpaca",
            source
        );
    }

    let syms = resolve_symbols(symbols.as_deref(), symbols_from_registry.as_deref())?;

    let tf = mqk_md::Timeframe::parse(&timeframe)?;

    // Wall clock is acceptable here — this is an operator CLI command, not a deterministic path.
    let end_d = match &end {
        Some(s) => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .with_context(|| format!("invalid --end date: {}", s))?,
        None => Utc::now().date_naive(),
    };

    let default_overlap: u32 = match tf {
        mqk_md::Timeframe::D1 => 5,
        mqk_md::Timeframe::H1 => 2,
        mqk_md::Timeframe::M15 => 2,
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

    let provider: Box<dyn mqk_md::HistoricalProvider> = if source_lc == "alpaca" {
        let (key, secret) = mqk_md::load_alpaca_paper_credentials()?;
        Box::new(mqk_md::AlpacaHistoricalProvider::new(key, secret))
    } else {
        let api_key = std::env::var(ENV_TWELVEDATA_API_KEY)
            .with_context(|| format!("missing env var {ENV_TWELVEDATA_API_KEY}"))?;
        Box::new(mqk_md::TwelveDataHistoricalProvider::new(api_key))
    };

    // --- Fetch per-symbol, collect all bars into one batch ---
    let mut all_raw: Vec<mqk_md::ProviderBar> = Vec::new();
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
        all_raw.extend(raw);
    }

    all_raw.sort_by(|a, b| {
        a.symbol
            .cmp(&b.symbol)
            .then_with(|| a.end_ts.cmp(&b.end_ts))
    });

    let (completed_raw, completion_report) =
        mqk_md::filter_completed_provider_bars(all_raw, tf, Utc::now().timestamp());
    print_completed_bar_filter_report(&source_lc, &completion_report);

    let all_bars: Vec<mqk_db::md::ProviderBar> = completed_raw
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
// registry-v2-status — ASSET-CORE-01C read-only v1->v2 status probe
// ---------------------------------------------------------------------------

/// Execute `mqk md registry-v2-status`: load the v1 instrument registry at
/// `registry`, convert it to `InstrumentRegistryV2` in memory, validate it,
/// and print a concise status report.
///
/// Read-only: no DB connection, no provider or broker calls, no writes.
/// Returns `Err` (nonzero exit) when the v1 load or the v2 validation fails,
/// mirroring `resolve_symbols`'s fail-closed convention for malformed
/// registries. Never makes `InstrumentRegistryV2` a trading input — this is
/// a diagnostic probe only.
pub fn md_registry_v2_status(registry: PathBuf) -> Result<()> {
    let v1 = mqk_md::instrument_registry::load_instrument_registry(&registry)
        .with_context(|| format!("registry-v2-status: v1 load failed: {}", registry.display()))?;
    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    let validation = mqk_md::instrument_registry_v2::validate_registry_v2(&v2);

    let v1_count = v1.len();
    let v2_count = v2.instruments.len();
    let etf_count = v2
        .instruments
        .iter()
        .filter(|i| i.instrument_kind.as_deref() == Some("etf"))
        .count();
    let enabled_count = v2.instruments.iter().filter(|i| i.enabled).count();
    let non_equity_count = v2
        .instruments
        .iter()
        .filter(|i| i.asset_class != "equity")
        .count();
    let enabled_non_equity_count = v2
        .instruments
        .iter()
        .filter(|i| i.enabled && i.asset_class != "equity")
        .count();

    println!("registry_path={}", registry.display());
    println!("schema_version={}", v2.schema_version);
    println!("v1_count={v1_count}");
    println!("v2_count={v2_count}");
    println!("etf_count={etf_count}");
    println!("enabled_count={enabled_count}");
    println!("non_equity_count={non_equity_count}");
    println!("enabled_non_equity_count={enabled_non_equity_count}");
    println!("production_cutover_enabled=false");
    println!("trading_uses_v2=false");

    match validation {
        Ok(()) => {
            println!("validation_passed=true");
            Ok(())
        }
        Err(e) => {
            println!("validation_passed=false");
            println!("validation_error={e}");
            Err(anyhow::anyhow!(
                "registry-v2-status: v2 validation failed: {e}"
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// coinlore-latest-mark — CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-
// BUNDLE-01-COMBINED, evidence contract standardized by
// CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED:
// read-only latest-mark evidence surface.
//
// Default mode reads a local fixture/response file (`--input-file`) --
// zero network calls. Network mode is opt-in only, gated behind the
// `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1` environment variable, and performs at
// most one HTTP GET. Neither mode connects to the DB, writes to `md_bars`,
// or touches any broker/runtime/risk/order path.
// ---------------------------------------------------------------------------

const ENV_ALLOW_COINLORE_NETWORK_SMOKE: &str = "MQK_ALLOW_COINLORE_NETWORK_SMOKE";

/// Schema version of the evidence JSON written by `--output-dir`. The daemon's
/// `GET /api/v1/market-data/latest-marks/status` route (01P) refuses to
/// surface evidence carrying any other value as `active`.
const COINLORE_LATEST_MARK_EVIDENCE_SCHEMA_VERSION: &str = "coinlore-latest-mark-v1";

/// Execute `mqk md coinlore-latest-mark`: resolve CoinLore aliases for
/// `--symbols` from the registry-v2 fixture at `--registry`, obtain a
/// CoinLore ticker response body (from `--input-file` by default, or via
/// exactly one live HTTP GET when `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1` and
/// no `--input-file` is given), parse it into `LatestMark`s, print them, and
/// optionally write a JSON evidence artifact to `--output-dir`.
///
/// `--provider-registry` (default `config/providers/providers.json`) is read
/// only to report `provider_enabled` in the evidence artifact/stdout; it is
/// never used to gate parsing or the network call itself (the CLI's own
/// `MQK_ALLOW_COINLORE_NETWORK_SMOKE` opt-in remains the sole network gate).
///
/// Read-only: never opens a DB connection, never writes `md_bars`, never
/// registers a scheduler, never touches a broker/runtime/risk/order path.
pub async fn md_coinlore_latest_mark(
    registry: PathBuf,
    symbols: String,
    input_file: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    provider_registry: PathBuf,
) -> Result<()> {
    let requested: Vec<String> = symbols
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if requested.is_empty() {
        anyhow::bail!("--symbols must contain at least one canonical symbol");
    }

    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(&registry)
        .with_context(|| format!("coinlore-latest-mark: registry load failed: {}", registry.display()))?;
    let all_aliases = mqk_md::coinlore_aliases_from_registry_v2(&registry_v2);

    let mut aliases = Vec::with_capacity(requested.len());
    for symbol in &requested {
        let alias = all_aliases
            .iter()
            .find(|a| &a.canonical_symbol == symbol)
            .cloned()
            .with_context(|| {
                format!("coinlore-latest-mark: no coinlore alias configured for symbol {symbol}")
            })?;
        aliases.push(alias);
    }

    let (body, network_call_made, mode) = match &input_file {
        Some(path) => {
            let content = fs::read_to_string(path)
                .with_context(|| format!("coinlore-latest-mark: input-file read failed: {}", path.display()))?;
            (content, false, "input_file")
        }
        None => {
            let allowed = std::env::var(ENV_ALLOW_COINLORE_NETWORK_SMOKE)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allowed {
                anyhow::bail!(
                    "coinlore-latest-mark: no --input-file provided and {ENV_ALLOW_COINLORE_NETWORK_SMOKE} is not set; \
                     refusing to make a network call by default"
                );
            }
            let coin_ids: Vec<String> = aliases.iter().map(|a| a.coinlore_id.clone()).collect();
            let body = mqk_md::fetch_coinlore_ticker_body(&coin_ids)
                .await
                .map_err(|e| anyhow::anyhow!("coinlore-latest-mark: network fetch failed: {e}"))?;
            (body, true, "network_smoke")
        }
    };

    let as_of_client_request_ts = Utc::now().timestamp();
    let marks = mqk_md::parse_coinlore_ticker_response(&body, &aliases, as_of_client_request_ts)
        .map_err(|e| anyhow::anyhow!("coinlore-latest-mark: parse failed: {e}"))?;

    // provider_enabled is reported for operator visibility only -- it does not
    // gate parsing or the network call above (see the doc comment on this fn).
    // A missing/unreadable/malformed provider registry fails closed to `false`
    // rather than panicking or defaulting to `true`.
    let provider_enabled = mqk_md::provider_registry::load_provider_registry(&provider_registry)
        .ok()
        .and_then(|providers| {
            providers
                .into_iter()
                .find(|p| p.provider_id == mqk_md::COINLORE_PROVIDER_ID)
                .map(|p| p.enabled)
        })
        .unwrap_or(false);

    println!("provider_id={}", mqk_md::COINLORE_PROVIDER_ID);
    println!("mode={mode}");
    println!("network_call_made={network_call_made}");
    println!("db_write=false");
    println!("md_bars_write=false");
    println!("completed_bar_claim=false");
    println!("provider_enabled={provider_enabled}");
    for mark in &marks {
        println!(
            "symbol={} provider_symbol={} provider_coin_id={} price_usd={} volume24_usd={} as_of_client_request_ts={} provider_ts={} truth_state={:?} kind={:?}",
            mark.canonical_symbol,
            mark.provider_symbol,
            mark.provider_coin_id.as_deref().unwrap_or("none"),
            mark.price_usd,
            mark.volume24_usd.as_deref().unwrap_or("none"),
            mark.as_of_client_request_ts,
            fmt_opt_i64(mark.provider_ts),
            mark.truth_state,
            mark.kind,
        );
    }

    if let Some(dir) = &output_dir {
        fs::create_dir_all(dir)
            .with_context(|| format!("coinlore-latest-mark: create output dir failed: {}", dir.display()))?;
        let produced_at_utc = chrono::DateTime::<Utc>::from_timestamp(as_of_client_request_ts, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let evidence = serde_json::json!({
            "schema_version": COINLORE_LATEST_MARK_EVIDENCE_SCHEMA_VERSION,
            "producer": "mqk-cli md coinlore-latest-mark",
            "produced_at_utc": produced_at_utc,
            "provider": mqk_md::COINLORE_PROVIDER_ID,
            "provider_id": mqk_md::COINLORE_PROVIDER_ID,
            "mode": mode,
            "network_call_made": network_call_made,
            "db_write": false,
            "md_bars_write": false,
            "completed_bar_claim": false,
            "provider_enabled": provider_enabled,
            "registry_path": registry.display().to_string(),
            "symbols_requested": requested,
            "truth_state": "active",
            "stale_or_missing": false,
            "marks": marks,
            "all_passed": true,
            "reason_code": "latest_mark_evidence_generated",
            "fail_reasons": Vec::<String>::new(),
        });
        let out_path = dir.join(format!(
            "coinlore_latest_mark_{}.json",
            as_of_client_request_ts
        ));
        fs::write(&out_path, serde_json::to_string_pretty(&evidence)?)
            .with_context(|| format!("coinlore-latest-mark: write evidence failed: {}", out_path.display()))?;
        println!("evidence_path={}", out_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// kraken-ohlc-dry-run — CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-
// BUNDLE-01-COMBINED: read-only Kraken OHLC parser evidence surface.
//
// Default mode reads a local fixture/response file (`--input-file`) --
// zero network calls, zero DB connection, zero `md_bars` write. Network mode
// is opt-in only, gated behind `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`, and
// performs at most one HTTP GET. Neither mode connects to the DB, writes to
// `md_bars`, or touches any broker/runtime/risk/order path.
// ---------------------------------------------------------------------------

const ENV_ALLOW_KRAKEN_NETWORK_SMOKE: &str = "MQK_ALLOW_KRAKEN_NETWORK_SMOKE";

/// Schema version of the evidence JSON written by `--output-dir`.
const KRAKEN_OHLC_DRY_RUN_EVIDENCE_SCHEMA_VERSION: &str = "kraken-ohlc-dry-run-v1";

/// Execute `mqk md kraken-ohlc-dry-run`: resolve the Kraken alias for
/// `--symbol` from the registry-v2 fixture at `--registry`, obtain a Kraken
/// `/0/public/OHLC` response body (from `--input-file` by default, or via
/// exactly one live HTTP GET when `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` and no
/// `--input-file` is given), parse it, print a summary, and optionally write
/// a JSON evidence artifact to `--output-dir`.
///
/// Read-only: never opens a DB connection, never writes `md_bars`, never
/// registers a scheduler, never touches a broker/runtime/risk/order path.
/// Only `--timeframe 1D` is supported by the current adapter.
pub async fn md_kraken_ohlc_dry_run(
    registry: PathBuf,
    symbol: String,
    timeframe: String,
    input_file: Option<PathBuf>,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    let tf = mqk_md::Timeframe::parse(&timeframe)?;
    if tf != mqk_md::Timeframe::D1 {
        anyhow::bail!(
            "kraken-ohlc-dry-run: only --timeframe 1D is supported by this adapter, got {timeframe}"
        );
    }

    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(&registry)
        .with_context(|| format!("kraken-ohlc-dry-run: registry load failed: {}", registry.display()))?;
    let aliases = mqk_md::kraken_aliases_from_registry_v2(&registry_v2);
    let alias = aliases
        .iter()
        .find(|a| a.canonical_symbol == symbol)
        .cloned()
        .with_context(|| {
            format!("kraken-ohlc-dry-run: no kraken alias configured for symbol {symbol}")
        })?;

    let (body, network_call_made, mode) = match &input_file {
        Some(path) => {
            let content = fs::read_to_string(path).with_context(|| {
                format!("kraken-ohlc-dry-run: input-file read failed: {}", path.display())
            })?;
            (content, false, "input_file")
        }
        None => {
            let allowed = std::env::var(ENV_ALLOW_KRAKEN_NETWORK_SMOKE)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allowed {
                anyhow::bail!(
                    "kraken-ohlc-dry-run: no --input-file provided and {ENV_ALLOW_KRAKEN_NETWORK_SMOKE} is not set; \
                     refusing to make a network call by default"
                );
            }
            let body = mqk_md::fetch_kraken_ohlc_body(&alias.kraken_pair)
                .await
                .map_err(|e| anyhow::anyhow!("kraken-ohlc-dry-run: network fetch failed: {e}"))?;
            (body, true, "network_smoke")
        }
    };

    let interval_seconds = tf.duration_secs();
    let parsed = mqk_md::parse_kraken_ohlc_response(&body, &alias.kraken_pair, interval_seconds)
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-dry-run: parse failed: {e}"))?;

    let completed = parsed
        .completed_provider_bars(&symbol, tf.as_str())
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-dry-run: bar conversion failed: {e}"))?;
    let bars_excluded_forming = parsed.forming_bars().len();
    let forming_candle_excluded = bars_excluded_forming > 0;

    let latest_completed = completed.iter().max_by_key(|b| b.end_ts);
    let latest_completed_end_ts = latest_completed.map(|b| b.end_ts);
    let latest_completed_start_ts = latest_completed.map(|b| b.end_ts - interval_seconds);

    println!("provider_id={}", mqk_md::KRAKEN_PROVIDER_ID);
    println!("mode={mode}");
    println!("network_call_made={network_call_made}");
    println!("db_write=false");
    println!("md_bars_write=false");
    println!("completed_bar_claim={}", !completed.is_empty());
    println!("forming_candle_excluded={forming_candle_excluded}");
    println!(
        "volume_semantics=kraken_base_asset_volume_scaled_by_{}_not_whole_coins_not_usd",
        mqk_md::KRAKEN_VOLUME_SCALE
    );
    println!("volume_scale={}", mqk_md::KRAKEN_VOLUME_SCALE);
    println!("symbol={symbol}");
    println!("bars_completed={}", completed.len());
    println!("bars_excluded_forming={bars_excluded_forming}");
    println!("latest_completed_start_ts={}", fmt_opt_i64(latest_completed_start_ts));
    println!("latest_completed_end_ts={}", fmt_opt_i64(latest_completed_end_ts));
    for bar in &completed {
        println!(
            "end_ts={} open={} high={} low={} close={} volume_scaled={} is_complete={}",
            bar.end_ts, bar.open, bar.high, bar.low, bar.close, bar.volume, bar.is_complete
        );
    }

    if let Some(dir) = &output_dir {
        fs::create_dir_all(dir).with_context(|| {
            format!("kraken-ohlc-dry-run: create output dir failed: {}", dir.display())
        })?;
        let produced_at_ts = Utc::now().timestamp();
        let produced_at_utc = chrono::DateTime::<Utc>::from_timestamp(produced_at_ts, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let evidence = serde_json::json!({
            "schema_version": KRAKEN_OHLC_DRY_RUN_EVIDENCE_SCHEMA_VERSION,
            "producer": "mqk-cli md kraken-ohlc-dry-run",
            "produced_at_utc": produced_at_utc,
            "provider": mqk_md::KRAKEN_PROVIDER_ID,
            "mode": mode,
            "network_call_made": network_call_made,
            "db_write": false,
            "md_bars_write": false,
            "completed_bar_claim": !completed.is_empty(),
            "forming_candle_excluded": forming_candle_excluded,
            "volume_semantics": "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
            "volume_scale": mqk_md::KRAKEN_VOLUME_SCALE,
            "symbols_requested": [symbol.clone()],
            "bars_completed": completed.len(),
            "bars_excluded_forming": bars_excluded_forming,
            "latest_completed_start_ts": latest_completed_start_ts,
            "latest_completed_end_ts": latest_completed_end_ts,
            "all_passed": true,
            "reason_code": "kraken_ohlc_dry_run_evidence_generated",
            "fail_reasons": Vec::<String>::new(),
        });
        let out_path = dir.join(format!("kraken_ohlc_dry_run_{produced_at_ts}.json"));
        fs::write(&out_path, serde_json::to_string_pretty(&evidence)?).with_context(|| {
            format!("kraken-ohlc-dry-run: write evidence failed: {}", out_path.display())
        })?;
        println!("evidence_path={}", out_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// kraken-ohlc-ingest — CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-
// BUNDLE-01-COMBINED: fixture-first Kraken provider-ingest path into
// `md_bars`.
//
// A Kraken-specific command (Option B), not a change to the generic
// `ingest-provider`/`sync-provider` commands: those are tightly coupled to
// TwelveData/Alpaca's date-range-chunked `HistoricalProvider::fetch_bars`
// semantics and have no fixture/input-file mode. This command mirrors
// `kraken-ohlc-dry-run`'s parse path but adds the canonical `md_bars` DB
// write, stamped with truthful Kraken provider metadata.
//
// Default mode reads a local fixture/response file (`--input-file`) -- zero
// network calls. Network mode is opt-in only, gated behind
// `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`, and performs at most one HTTP GET. The
// forming (not-yet-committed) candle is never converted or written to
// `md_bars` -- only `KrakenPairOhlc::completed_provider_bars` output reaches
// the DB write path.
// ---------------------------------------------------------------------------

/// Schema version of the evidence JSON written by `--output-dir`.
const KRAKEN_OHLC_INGEST_EVIDENCE_SCHEMA_VERSION: &str = "kraken-ohlc-ingest-v1";

/// Execute `mqk md kraken-ohlc-ingest`: resolve the Kraken alias for
/// `--symbol` from the registry-v2 fixture at `--registry`, obtain a Kraken
/// `/0/public/OHLC` response body (from `--input-file` by default, or via
/// exactly one live HTTP GET when `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` and no
/// `--input-file` is given), parse it, and ingest only the completed rows
/// into `md_bars` via
/// [`mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata`]
/// with truthful `provider_id="kraken"` / `provider_source="kraken"`
/// metadata. Only `--timeframe 1D` is supported by the current adapter.
///
/// Never touches broker/runtime/risk/order/strategy code and never
/// registers a scheduler -- this is a single explicit operator invocation.
pub async fn md_kraken_ohlc_ingest(
    registry: PathBuf,
    symbol: String,
    timeframe: String,
    input_file: Option<PathBuf>,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    let tf = mqk_md::Timeframe::parse(&timeframe)?;
    if tf != mqk_md::Timeframe::D1 {
        anyhow::bail!(
            "kraken-ohlc-ingest: only --timeframe 1D is supported by this adapter, got {timeframe}"
        );
    }

    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(&registry)
        .with_context(|| format!("kraken-ohlc-ingest: registry load failed: {}", registry.display()))?;
    let aliases = mqk_md::kraken_aliases_from_registry_v2(&registry_v2);
    let alias = aliases
        .iter()
        .find(|a| a.canonical_symbol == symbol)
        .cloned()
        .with_context(|| {
            format!("kraken-ohlc-ingest: no kraken alias configured for symbol {symbol}")
        })?;

    let (body, network_call_made, mode) = match &input_file {
        Some(path) => {
            let content = fs::read_to_string(path).with_context(|| {
                format!("kraken-ohlc-ingest: input-file read failed: {}", path.display())
            })?;
            (content, false, "input_file")
        }
        None => {
            let allowed = std::env::var(ENV_ALLOW_KRAKEN_NETWORK_SMOKE)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allowed {
                anyhow::bail!(
                    "kraken-ohlc-ingest: no --input-file provided and {ENV_ALLOW_KRAKEN_NETWORK_SMOKE} is not set; \
                     refusing to make a network call by default (kraken_requires_input_file_or_network_opt_in)"
                );
            }
            let body = mqk_md::fetch_kraken_ohlc_body(&alias.kraken_pair)
                .await
                .map_err(|e| anyhow::anyhow!("kraken-ohlc-ingest: network fetch failed: {e}"))?;
            (body, true, "network_smoke")
        }
    };

    let interval_seconds = tf.duration_secs();
    let parsed = mqk_md::parse_kraken_ohlc_response(&body, &alias.kraken_pair, interval_seconds)
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-ingest: parse failed: {e}"))?;

    let completed = parsed
        .completed_provider_bars(&symbol, tf.as_str())
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-ingest: bar conversion failed: {e}"))?;
    let bars_excluded_forming = parsed.forming_bars().len();
    let forming_candle_excluded = bars_excluded_forming > 0;

    let latest_completed = completed.iter().max_by_key(|b| b.end_ts);
    let latest_completed_end_ts = latest_completed.map(|b| b.end_ts);
    let latest_completed_start_ts = latest_completed.map(|b| b.end_ts - interval_seconds);

    // mqk_md::ProviderBar -> mqk_db::md::ProviderBar: same field shape used
    // by md_ingest_provider's own conversion above.
    let db_bars: Vec<mqk_db::md::ProviderBar> = completed
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

    // Deterministic ingest_id from (symbol, timeframe, kraken pair) -- stable
    // across re-runs against the same registry/symbol so retries stay
    // idempotent, mirroring md_ingest_csv/md_ingest_provider's UUIDv5
    // convention (D1-5: no random UUID in src/).
    let ingest_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-ingest.kraken.v1|{}|{}|{}",
            symbol,
            tf.as_str(),
            alias.kraken_pair
        )
        .as_bytes(),
    );

    let provider_metadata = mqk_db::md::MdBarProviderMetadata {
        provider_id: mqk_md::KRAKEN_PROVIDER_ID.to_string(),
        provider_source: Some(mqk_md::KRAKEN_PROVIDER_ID.to_string()),
        provider_symbol: Some(parsed.provider_pair_key.clone()),
        ingest_mode: Some("provider_ingest".to_string()),
        provider_bar_id: None,
        provider_updated_at_utc: None,
    };

    let pool = mqk_db::connect_from_env().await?;
    let res = mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: mqk_md::KRAKEN_PROVIDER_ID.to_string(),
            timeframe: tf.as_str().to_string(),
            ingest_id,
            bars: db_bars,
        },
        provider_metadata,
    )
    .await?;

    println!("provider_id={}", mqk_md::KRAKEN_PROVIDER_ID);
    println!("provider_source={}", mqk_md::KRAKEN_PROVIDER_ID);
    println!("provider_symbol={}", parsed.provider_pair_key);
    println!("ingest_mode=provider_ingest");
    println!("mode={mode}");
    println!("network_call_made={network_call_made}");
    println!("db_write=true");
    println!("md_bars_write=true");
    println!("forming_candle_excluded={forming_candle_excluded}");
    println!(
        "volume_semantics=kraken_base_asset_volume_scaled_by_{}_not_whole_coins_not_usd",
        mqk_md::KRAKEN_VOLUME_SCALE
    );
    println!("volume_scale={}", mqk_md::KRAKEN_VOLUME_SCALE);
    println!("symbol={symbol}");
    println!("ingest_id={}", res.ingest_id);
    println!("bars_completed={}", completed.len());
    println!("bars_excluded_forming={bars_excluded_forming}");
    println!("latest_completed_start_ts={}", fmt_opt_i64(latest_completed_start_ts));
    println!("latest_completed_end_ts={}", fmt_opt_i64(latest_completed_end_ts));
    println!(
        "rows_read={} rows_ok={} rejected={} inserted={} updated={}",
        res.report.coverage.rows_read,
        res.report.coverage.rows_ok,
        res.report.coverage.rows_rejected,
        res.report.coverage.rows_inserted,
        res.report.coverage.rows_updated
    );

    if let Some(dir) = &output_dir {
        fs::create_dir_all(dir).with_context(|| {
            format!("kraken-ohlc-ingest: create output dir failed: {}", dir.display())
        })?;
        let produced_at_ts = Utc::now().timestamp();
        let produced_at_utc = chrono::DateTime::<Utc>::from_timestamp(produced_at_ts, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let evidence = serde_json::json!({
            "schema_version": KRAKEN_OHLC_INGEST_EVIDENCE_SCHEMA_VERSION,
            "producer": "mqk-cli md kraken-ohlc-ingest",
            "produced_at_utc": produced_at_utc,
            "provider": mqk_md::KRAKEN_PROVIDER_ID,
            "mode": mode,
            "network_call_made": network_call_made,
            "db_write": true,
            "md_bars_write": true,
            "provider_id": mqk_md::KRAKEN_PROVIDER_ID,
            "provider_source": mqk_md::KRAKEN_PROVIDER_ID,
            "provider_symbol": parsed.provider_pair_key,
            "ingest_mode": "provider_ingest",
            "symbols_requested": [symbol.clone()],
            "bars_completed": completed.len(),
            "bars_excluded_forming": bars_excluded_forming,
            "latest_completed_start_ts": latest_completed_start_ts,
            "latest_completed_end_ts": latest_completed_end_ts,
            "volume_semantics": "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
            "volume_scale": mqk_md::KRAKEN_VOLUME_SCALE,
            "rows_inserted": res.report.coverage.rows_inserted,
            "rows_updated": res.report.coverage.rows_updated,
            "all_passed": true,
            "reason_code": "kraken_ingest_evidence_generated",
            "fail_reasons": Vec::<String>::new(),
        });
        let out_path = dir.join(format!("kraken_ohlc_ingest_{produced_at_ts}.json"));
        fs::write(&out_path, serde_json::to_string_pretty(&evidence)?).with_context(|| {
            format!("kraken-ohlc-ingest: write evidence failed: {}", out_path.display())
        })?;
        println!("evidence_path={}", out_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// kraken-ohlc-sync — CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-
// COMBINED: true content-diff-aware sync, continuing from the presence-based
// proof landed by 01Z-AA.
//
// This is a separate, additive command from `kraken-ohlc-ingest` -- not a
// change to it. It shares the same fail-closed fixture/network gate, the
// same parser, and the same canonical `md_bars` write helper
// (`ingest_provider_bars_to_md_bars_with_provider_metadata`, unchanged), but
// stamps rows with `ingest_mode="provider_sync"` (distinct from
// `kraken-ohlc-ingest`'s `"provider_ingest"`) so DB rows/quality-report
// history can be traced back to which command wrote them.
//
// 01Z-AA honestly documented that classification was by `end_ts` presence
// relative to a high-water mark only, because the write helper always
// performs an unconditional `ON CONFLICT DO UPDATE` -- it could not tell
// "existing and unchanged" from "existing and changed". This patch closes
// that gap with a minimal read-only helper,
// `mqk_db::md::fetch_md_bars_for_provider_sync_keys`, that reads existing
// `md_bars` rows for the exact candidate `end_ts` keys, plus a pure
// comparison helper, `mqk_db::md::provider_bar_matches_existing`, that
// compares OHLCV + `is_complete` + provider provenance
// (`provider_id`/`provider_source`/`provider_symbol`/`ingest_mode`).
//
// Default policy now classifies every completed (non-forming) bar as:
// - missing/new: no existing row for that `end_ts` -> always written.
// - changed: existing row present but content/provenance differs -> written
//   unless `--no-update-existing` is passed, in which case it is counted as
//   `rows_changed_skipped_due_to_no_update_existing` and never sent to the
//   write helper.
// - unchanged: existing row present and content/provenance is identical ->
//   never sent to the write helper, counted as `rows_skipped_unchanged`.
// If no candidate bar requires a write, the upsert helper is not called at
// all (`md_bars_write=false`).
// ---------------------------------------------------------------------------

/// Schema version of the evidence JSON written by `--output-dir`.
const KRAKEN_OHLC_SYNC_EVIDENCE_SCHEMA_VERSION: &str = "kraken-ohlc-sync-v2";

/// Execute `mqk md kraken-ohlc-sync`: resolve the Kraken alias for
/// `--symbol`, obtain a Kraken `/0/public/OHLC` response body (fixture-first,
/// same fail-closed gate as `kraken-ohlc-ingest`), parse it, read existing
/// `md_bars` rows for the exact candidate `end_ts` keys, classify each
/// completed (non-forming) bar as missing/changed/unchanged by comparing
/// OHLCV + `is_complete` + provider provenance, then upsert only the
/// missing and (unless `--no-update-existing`) changed bars into `md_bars`
/// via the existing provider-metadata-aware helper, stamped
/// `ingest_mode="provider_sync"`.
///
/// Only `--timeframe 1D` is supported. Never touches
/// broker/runtime/risk/order/strategy code and never registers a scheduler
/// -- this is a single explicit operator invocation, not recurring sync.
#[allow(clippy::too_many_arguments)]
pub async fn md_kraken_ohlc_sync(
    registry: PathBuf,
    symbol: String,
    timeframe: String,
    input_file: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    no_update_existing: bool,
) -> Result<()> {
    let tf = mqk_md::Timeframe::parse(&timeframe)?;
    if tf != mqk_md::Timeframe::D1 {
        anyhow::bail!(
            "kraken-ohlc-sync: only --timeframe 1D is supported by this adapter, got {timeframe}"
        );
    }

    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(&registry)
        .with_context(|| format!("kraken-ohlc-sync: registry load failed: {}", registry.display()))?;
    let aliases = mqk_md::kraken_aliases_from_registry_v2(&registry_v2);
    let alias = aliases
        .iter()
        .find(|a| a.canonical_symbol == symbol)
        .cloned()
        .with_context(|| {
            format!("kraken-ohlc-sync: no kraken alias configured for symbol {symbol}")
        })?;

    // Fail-closed gate: refuses before any file/network/DB access when
    // neither --input-file nor the network opt-in env var is present.
    let (body, network_call_made, mode) = match &input_file {
        Some(path) => {
            let content = fs::read_to_string(path).with_context(|| {
                format!("kraken-ohlc-sync: input-file read failed: {}", path.display())
            })?;
            (content, false, "input_file")
        }
        None => {
            let allowed = std::env::var(ENV_ALLOW_KRAKEN_NETWORK_SMOKE)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allowed {
                anyhow::bail!(
                    "kraken-ohlc-sync: no --input-file provided and {ENV_ALLOW_KRAKEN_NETWORK_SMOKE} is not set; \
                     refusing to make a network call by default (kraken_sync_requires_input_file_or_network_opt_in)"
                );
            }
            let body = mqk_md::fetch_kraken_ohlc_body(&alias.kraken_pair)
                .await
                .map_err(|e| anyhow::anyhow!("kraken-ohlc-sync: network fetch failed: {e}"))?;
            (body, true, "network_smoke")
        }
    };

    let interval_seconds = tf.duration_secs();
    let parsed = mqk_md::parse_kraken_ohlc_response(&body, &alias.kraken_pair, interval_seconds)
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-sync: parse failed: {e}"))?;

    let completed = parsed
        .completed_provider_bars(&symbol, tf.as_str())
        .map_err(|e| anyhow::anyhow!("kraken-ohlc-sync: bar conversion failed: {e}"))?;
    let bars_excluded_forming = parsed.forming_bars().len();
    let forming_candle_excluded = bars_excluded_forming > 0;

    let latest_completed = completed.iter().max_by_key(|b| b.end_ts);
    let latest_completed_end_ts = latest_completed.map(|b| b.end_ts);
    let latest_completed_start_ts = latest_completed.map(|b| b.end_ts - interval_seconds);

    // DB connection only happens after the parse/gate above has passed.
    let pool = mqk_db::connect_from_env().await?;

    // Descriptive/back-compat evidence field only -- no longer used for
    // classification (see the content-diff read below).
    let latest_existing_end_ts_before =
        mqk_db::md::latest_stored_bar_end_ts(&pool, &symbol, tf.as_str()).await?;

    // CRYPTO-DATA-01AB: true content-diff classification. Reads existing
    // rows for exactly the candidate end_ts keys -- never a broader scan --
    // then compares OHLCV + is_complete + provider provenance per row.
    let candidate_keys: Vec<i64> = completed.iter().map(|b| b.end_ts).collect();
    let existing_by_end_ts = mqk_db::md::fetch_md_bars_for_provider_sync_keys(
        &pool,
        &symbol,
        tf.as_str(),
        &candidate_keys,
    )
    .await?;

    let provider_metadata = mqk_db::md::MdBarProviderMetadata {
        provider_id: mqk_md::KRAKEN_PROVIDER_ID.to_string(),
        provider_source: Some(mqk_md::KRAKEN_PROVIDER_ID.to_string()),
        provider_symbol: Some(parsed.provider_pair_key.clone()),
        ingest_mode: Some("provider_sync".to_string()),
        provider_bar_id: None,
        provider_updated_at_utc: None,
    };

    let mut bars_missing_new: u64 = 0;
    let mut rows_changed: u64 = 0;
    let mut rows_skipped_unchanged: u64 = 0;
    let mut rows_changed_skipped_due_to_no_update_existing: u64 = 0;
    let mut db_bars: Vec<mqk_db::md::ProviderBar> = Vec::new();

    for b in &completed {
        let db_bar = mqk_db::md::ProviderBar {
            symbol: b.symbol.clone(),
            timeframe: b.timeframe.clone(),
            end_ts: b.end_ts,
            open: b.open.clone(),
            high: b.high.clone(),
            low: b.low.clone(),
            close: b.close.clone(),
            volume: b.volume,
            is_complete: b.is_complete,
        };

        match existing_by_end_ts.get(&b.end_ts) {
            None => {
                bars_missing_new += 1;
                db_bars.push(db_bar);
            }
            Some(existing) => {
                let unchanged = mqk_db::md::provider_bar_matches_existing(
                    &db_bar,
                    &provider_metadata,
                    existing,
                )?;
                if unchanged {
                    rows_skipped_unchanged += 1;
                } else {
                    rows_changed += 1;
                    if no_update_existing {
                        rows_changed_skipped_due_to_no_update_existing += 1;
                    } else {
                        db_bars.push(db_bar);
                    }
                }
            }
        }
    }

    let bars_existing_candidate = rows_changed + rows_skipped_unchanged;
    let rows_skipped_if_known = (completed.len() as u64) - (db_bars.len() as u64);
    let md_bars_write = !db_bars.is_empty();
    let sync_policy = "content_diff_skip_unchanged_update_changed_insert_missing";

    // Deterministic ingest_id, distinct namespace from kraken-ohlc-ingest's
    // so this command's md_quality_reports history never collides with it.
    let ingest_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-ingest.kraken.sync.v1|{}|{}|{}",
            symbol,
            tf.as_str(),
            alias.kraken_pair
        )
        .as_bytes(),
    );

    // If no bar requires a write, the upsert helper is not called at all.
    let (final_ingest_id, rows_inserted, rows_updated) = if md_bars_write {
        let res = mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
            &pool,
            mqk_db::md::IngestProviderBarsArgs {
                source: mqk_md::KRAKEN_PROVIDER_ID.to_string(),
                timeframe: tf.as_str().to_string(),
                ingest_id,
                bars: db_bars,
            },
            provider_metadata,
        )
        .await?;
        (
            res.ingest_id,
            res.report.coverage.rows_inserted,
            res.report.coverage.rows_updated,
        )
    } else {
        (ingest_id, 0, 0)
    };

    println!("provider_id={}", mqk_md::KRAKEN_PROVIDER_ID);
    println!("provider_source={}", mqk_md::KRAKEN_PROVIDER_ID);
    println!("provider_symbol={}", parsed.provider_pair_key);
    println!("ingest_mode=provider_sync");
    println!("mode={mode}");
    println!("network_call_made={network_call_made}");
    println!("db_write=true");
    println!("md_bars_write={md_bars_write}");
    println!("forming_candle_excluded={forming_candle_excluded}");
    println!(
        "volume_semantics=kraken_base_asset_volume_scaled_by_{}_not_whole_coins_not_usd",
        mqk_md::KRAKEN_VOLUME_SCALE
    );
    println!("volume_scale={}", mqk_md::KRAKEN_VOLUME_SCALE);
    println!("symbol={symbol}");
    println!("ingest_id={final_ingest_id}");
    println!("sync_policy={sync_policy}");
    println!("no_update_existing={no_update_existing}");
    println!("latest_existing_end_ts_before={}", fmt_opt_i64(latest_existing_end_ts_before));
    println!("bars_completed={}", completed.len());
    println!("bars_excluded_forming={bars_excluded_forming}");
    println!("bars_considered_for_sync={}", completed.len());
    println!("bars_missing_new={bars_missing_new}");
    println!("bars_existing_candidate={bars_existing_candidate}");
    println!("rows_changed={rows_changed}");
    println!("rows_skipped_unchanged={rows_skipped_unchanged}");
    println!(
        "rows_changed_skipped_due_to_no_update_existing={rows_changed_skipped_due_to_no_update_existing}"
    );
    println!("rows_skipped_if_known={rows_skipped_if_known}");
    println!("latest_completed_start_ts={}", fmt_opt_i64(latest_completed_start_ts));
    println!("latest_completed_end_ts={}", fmt_opt_i64(latest_completed_end_ts));
    println!("inserted={rows_inserted} updated={rows_updated}");

    if let Some(dir) = &output_dir {
        fs::create_dir_all(dir).with_context(|| {
            format!("kraken-ohlc-sync: create output dir failed: {}", dir.display())
        })?;
        let produced_at_ts = Utc::now().timestamp();
        let produced_at_utc = chrono::DateTime::<Utc>::from_timestamp(produced_at_ts, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let evidence = serde_json::json!({
            "schema_version": KRAKEN_OHLC_SYNC_EVIDENCE_SCHEMA_VERSION,
            "producer": "mqk-cli md kraken-ohlc-sync",
            "produced_at_utc": produced_at_utc,
            "provider": mqk_md::KRAKEN_PROVIDER_ID,
            "mode": mode,
            "network_call_made": network_call_made,
            "db_write": true,
            "md_bars_write": md_bars_write,
            "provider_id": mqk_md::KRAKEN_PROVIDER_ID,
            "provider_source": mqk_md::KRAKEN_PROVIDER_ID,
            "provider_symbol": parsed.provider_pair_key,
            "ingest_mode": "provider_sync",
            "sync_policy": sync_policy,
            "no_update_existing": no_update_existing,
            "symbols_requested": [symbol.clone()],
            "bars_completed": completed.len(),
            "bars_excluded_forming": bars_excluded_forming,
            "bars_considered_for_sync": completed.len(),
            "bars_missing_new": bars_missing_new,
            "bars_existing_candidate": bars_existing_candidate,
            "rows_changed": rows_changed,
            "rows_skipped_unchanged": rows_skipped_unchanged,
            "rows_changed_skipped_due_to_no_update_existing": rows_changed_skipped_due_to_no_update_existing,
            "rows_inserted": rows_inserted,
            "rows_updated": rows_updated,
            "rows_skipped_if_known": rows_skipped_if_known,
            "latest_existing_end_ts_before": latest_existing_end_ts_before,
            "latest_completed_start_ts": latest_completed_start_ts,
            "latest_completed_end_ts": latest_completed_end_ts,
            "volume_semantics": "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
            "volume_scale": mqk_md::KRAKEN_VOLUME_SCALE,
            "all_passed": true,
            "reason_code": "kraken_sync_evidence_generated",
            "fail_reasons": Vec::<String>::new(),
        });
        let out_path = dir.join(format!("kraken_ohlc_sync_{produced_at_ts}.json"));
        fs::write(&out_path, serde_json::to_string_pretty(&evidence)?).with_context(|| {
            format!("kraken-ohlc-sync: write evidence failed: {}", out_path.display())
        })?;
        println!("evidence_path={}", out_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// crypto-registry-readiness — CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-
// READINESS-CLI-01: read-only operator readiness surface proving the
// current, unmodified registry-v2 fixture and providers config are ready
// for data-only Kraken OHLCV operations, without touching any config flag.
//
// Never opens a DB connection, never makes a provider/network call, never
// writes `md_bars`, never registers a scheduler, never mutates `--registry`
// or `--providers`. `enabled=false` on the crypto rows and on the provider
// is the expected, correct state for this phase -- not a failure. Per
// CRYPTO-REGISTRY-02's decision, `enabled=true` on the provider is treated
// as unsafe until a future, separate decision explicitly permits it.
// ---------------------------------------------------------------------------

/// Schema version of the evidence JSON written by `--output-dir`.
const CRYPTO_REGISTRY_READINESS_SCHEMA_VERSION: &str = "crypto-registry-readiness-v1";

/// Per-symbol readiness check result. `enabled`/`paper_trading_enabled`/
/// `live_trading_enabled` are `None` only when the symbol was not found in
/// the registry at all (distinct from `Some(false)`).
#[derive(Debug, Clone, serde::Serialize)]
struct CryptoRegistrySymbolCheck {
    symbol: String,
    found: bool,
    asset_class_ok: bool,
    kraken_pair: Option<String>,
    kraken_result_key: Option<String>,
    alias_ok: bool,
    enabled: Option<bool>,
    paper_trading_enabled: Option<bool>,
    live_trading_enabled: Option<bool>,
    trading_flags_safe: bool,
    passed: bool,
}

/// Execute `mqk md crypto-registry-readiness`: read `--providers` and
/// `--registry` (neither is ever mutated) and classify whether the current
/// configs are ready for data-only `--provider` (default `kraken`) OHLCV
/// operations for `--symbols` (default `BTC/USD,ETH/USD`). Never implies
/// trading, scheduler, or production-cutover readiness.
///
/// `provider_enabled=false` and per-symbol `enabled=false` are expected,
/// correct states here and classify as `data_ready_manual_only`, not a
/// failure. `paper_trading_enabled=true`, `live_trading_enabled=true`, or
/// `provider_enabled=true` (per `CRYPTO-REGISTRY-02`) each fail closed as
/// `unsafe_*`. A missing provider entry, missing symbol, or incomplete
/// Kraken alias (`kraken_pair`/`kraken_result_key`) also fails closed.
///
/// Read-only: never opens a DB connection, never calls a provider/network
/// endpoint, never writes `md_bars`, never registers a scheduler.
pub fn md_crypto_registry_readiness(
    registry: PathBuf,
    providers: PathBuf,
    provider_id: String,
    symbols: String,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    let requested: Vec<String> = symbols
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if requested.is_empty() {
        anyhow::bail!(
            "crypto-registry-readiness: --symbols must contain at least one canonical symbol"
        );
    }

    let mut fail_reasons: Vec<String> = Vec::new();
    let mut truth_state = "active";

    let provider_configs = mqk_md::provider_registry::load_provider_registry(&providers)
        .with_context(|| {
            format!(
                "crypto-registry-readiness: providers load failed: {}",
                providers.display()
            )
        })?;
    let provider_cfg = provider_configs.iter().find(|p| p.provider_id == provider_id);

    let (provider_enabled, api_key_required, provider_asset_classes, provider_implementation_status) =
        match provider_cfg {
            Some(cfg) => (
                cfg.enabled,
                cfg.api_key_required,
                cfg.asset_classes.clone(),
                cfg.implementation_status.clone(),
            ),
            None => {
                truth_state = "missing_provider";
                fail_reasons.push(format!(
                    "provider '{provider_id}' not found in {}",
                    providers.display()
                ));
                (false, false, Vec::new(), String::new())
            }
        };

    if provider_cfg.is_some() && provider_enabled {
        truth_state = "unsafe_provider_enabled";
        fail_reasons.push(format!(
            "provider '{provider_id}' is enabled=true; expected false pending an explicit, separate cutover decision (CRYPTO-REGISTRY-02)"
        ));
    }

    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(&registry)
        .with_context(|| {
            format!(
                "crypto-registry-readiness: registry load failed: {}",
                registry.display()
            )
        })?;

    let mut symbol_checks = Vec::with_capacity(requested.len());
    for symbol in &requested {
        let inst = registry_v2.instruments.iter().find(|i| &i.symbol == symbol);
        let check = match inst {
            None => {
                if truth_state == "active" {
                    truth_state = "missing_symbol";
                }
                fail_reasons.push(format!(
                    "symbol '{symbol}' not found in {}",
                    registry.display()
                ));
                CryptoRegistrySymbolCheck {
                    symbol: symbol.clone(),
                    found: false,
                    asset_class_ok: false,
                    kraken_pair: None,
                    kraken_result_key: None,
                    alias_ok: false,
                    enabled: None,
                    paper_trading_enabled: None,
                    live_trading_enabled: None,
                    trading_flags_safe: false,
                    passed: false,
                }
            }
            Some(inst) => {
                let asset_class_ok = inst.asset_class == "crypto";
                if !asset_class_ok {
                    if truth_state == "active" {
                        truth_state = "missing_symbol";
                    }
                    fail_reasons.push(format!(
                        "symbol '{symbol}' asset_class is '{}', expected 'crypto'",
                        inst.asset_class
                    ));
                }

                let kraken_pair = inst.provider_symbols.get("kraken_pair").cloned();
                let kraken_result_key = inst.provider_symbols.get("kraken_result_key").cloned();
                let alias_ok = kraken_pair.is_some() && kraken_result_key.is_some();
                if !alias_ok {
                    if truth_state == "active" {
                        truth_state = "missing_alias";
                    }
                    fail_reasons.push(format!(
                        "symbol '{symbol}' missing kraken_pair/kraken_result_key alias"
                    ));
                }

                let trading_flags_safe = !inst.paper_trading_enabled && !inst.live_trading_enabled;
                if inst.paper_trading_enabled {
                    truth_state = "unsafe_trading_enabled";
                    fail_reasons.push(format!("symbol '{symbol}' paper_trading_enabled=true"));
                }
                if inst.live_trading_enabled {
                    truth_state = "unsafe_trading_enabled";
                    fail_reasons.push(format!("symbol '{symbol}' live_trading_enabled=true"));
                }

                let passed = asset_class_ok && alias_ok && trading_flags_safe;
                CryptoRegistrySymbolCheck {
                    symbol: symbol.clone(),
                    found: true,
                    asset_class_ok,
                    kraken_pair,
                    kraken_result_key,
                    alias_ok,
                    enabled: Some(inst.enabled),
                    paper_trading_enabled: Some(inst.paper_trading_enabled),
                    live_trading_enabled: Some(inst.live_trading_enabled),
                    trading_flags_safe,
                    passed,
                }
            }
        };
        symbol_checks.push(check);
    }

    let all_symbols_disabled = symbol_checks.iter().all(|c| c.enabled == Some(false));
    let all_passed = truth_state == "active" && fail_reasons.is_empty();

    let data_readiness_state = if !all_passed {
        "blocked"
    } else if all_symbols_disabled {
        "data_ready_manual_only"
    } else {
        "production_default"
    };
    let trading_readiness_state = "disabled";
    let scheduler_readiness_state = "absent";

    let reason_code = if all_passed {
        "crypto_registry_readiness_data_ready_manual_only"
    } else {
        match truth_state {
            "missing_provider" => "provider_not_found",
            "missing_symbol" => "symbol_not_found_or_wrong_asset_class",
            "missing_alias" => "kraken_alias_incomplete",
            "unsafe_trading_enabled" => "trading_flag_unexpectedly_true",
            "unsafe_provider_enabled" => "provider_unexpectedly_enabled",
            _ => "readiness_check_failed",
        }
    };

    println!("schema_version={CRYPTO_REGISTRY_READINESS_SCHEMA_VERSION}");
    println!("provider={provider_id}");
    println!("registry_path={}", registry.display());
    println!("providers_path={}", providers.display());
    println!("truth_state={truth_state}");
    println!("data_readiness_state={data_readiness_state}");
    println!("trading_readiness_state={trading_readiness_state}");
    println!("scheduler_readiness_state={scheduler_readiness_state}");
    println!("provider_enabled={provider_enabled}");
    println!("api_key_required={api_key_required}");
    println!(
        "provider_asset_classes={}",
        provider_asset_classes.join(",")
    );
    println!("provider_implementation_status={provider_implementation_status}");
    println!("all_passed={all_passed}");
    println!("reason_code={reason_code}");
    println!("no_scheduler=true");
    println!("no_db_connection=true");
    println!("no_network_call=true");
    println!("no_trading_enabled=true");
    for check in &symbol_checks {
        println!(
            "symbol={} found={} asset_class_ok={} alias_ok={} enabled={} paper_trading_enabled={} live_trading_enabled={} passed={}",
            check.symbol,
            check.found,
            check.asset_class_ok,
            check.alias_ok,
            fmt_opt_bool(check.enabled),
            fmt_opt_bool(check.paper_trading_enabled),
            fmt_opt_bool(check.live_trading_enabled),
            check.passed,
        );
    }
    for reason in &fail_reasons {
        println!("fail_reason={reason}");
    }

    if let Some(dir) = &output_dir {
        fs::create_dir_all(dir).with_context(|| {
            format!(
                "crypto-registry-readiness: create output dir failed: {}",
                dir.display()
            )
        })?;
        let produced_at_ts = Utc::now().timestamp();
        let produced_at_utc = chrono::DateTime::<Utc>::from_timestamp(produced_at_ts, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let evidence = serde_json::json!({
            "schema_version": CRYPTO_REGISTRY_READINESS_SCHEMA_VERSION,
            "producer": "mqk-cli md crypto-registry-readiness",
            "produced_at_utc": produced_at_utc,
            "provider": provider_id,
            "registry_path": registry.display().to_string(),
            "providers_path": providers.display().to_string(),
            "symbols_requested": requested,
            "truth_state": truth_state,
            "data_readiness_state": data_readiness_state,
            "trading_readiness_state": trading_readiness_state,
            "scheduler_readiness_state": scheduler_readiness_state,
            "provider_enabled": provider_enabled,
            "api_key_required": api_key_required,
            "provider_asset_classes": provider_asset_classes,
            "provider_implementation_status": provider_implementation_status,
            "symbol_checks": symbol_checks,
            "all_passed": all_passed,
            "reason_code": reason_code,
            "fail_reasons": fail_reasons,
            "safety": {
                "no_scheduler": true,
                "no_db_connection": true,
                "no_network_call": true,
                "no_trading_enabled": true,
                "no_config_file_mutated": true,
            },
        });
        let out_path = dir.join(format!("crypto_registry_readiness_{produced_at_ts}.json"));
        fs::write(&out_path, serde_json::to_string_pretty(&evidence)?).with_context(|| {
            format!(
                "crypto-registry-readiness: write evidence failed: {}",
                out_path.display()
            )
        })?;
        println!("evidence_path={}", out_path.display());
    }

    if !all_passed {
        anyhow::bail!(
            "crypto-registry-readiness: {reason_code}: {}",
            fail_reasons.join("; ")
        );
    }

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

    // -----------------------------------------------------------------------
    // resolve_symbols — DATA-INGEST-SYNC-ALL-EQUITIES-01
    // -----------------------------------------------------------------------

    fn canonical_registry_path() -> std::path::PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir).join("../../../config/instruments/equities.json")
    }

    // RS-01: --symbols CSV resolves to the expected symbol list (no provider call).
    #[test]
    fn rs_01_symbols_csv_resolves_correctly() {
        let syms = resolve_symbols(Some("AAPL,SPY,MSFT"), None).unwrap();
        assert_eq!(syms, vec!["AAPL", "SPY", "MSFT"]);
    }

    // RS-02: --symbols trims whitespace around commas.
    #[test]
    fn rs_02_symbols_csv_trims_whitespace() {
        let syms = resolve_symbols(Some(" AAPL , SPY , MSFT "), None).unwrap();
        assert_eq!(syms, vec!["AAPL", "SPY", "MSFT"]);
    }

    // RS-03: --symbols-from-registry loads canonical registry and returns 88 symbols.
    #[test]
    fn rs_03_registry_resolves_88_enabled_equities() {
        let path = canonical_registry_path();
        let syms = resolve_symbols(None, Some(&path)).unwrap();
        assert_eq!(
            syms.len(),
            88,
            "registry must resolve exactly 88 enabled equity symbols"
        );
    }

    // RS-04: symbols from registry are in deterministic alphabetical order.
    #[test]
    fn rs_04_registry_symbols_are_sorted() {
        let path = canonical_registry_path();
        let syms = resolve_symbols(None, Some(&path)).unwrap();
        for window in syms.windows(2) {
            assert!(
                window[0] <= window[1],
                "symbols not sorted: {} before {}",
                window[0],
                window[1]
            );
        }
    }

    // RS-05: --symbols-from-registry includes known anchors (AAPL, SPY).
    #[test]
    fn rs_05_registry_contains_known_anchors() {
        let path = canonical_registry_path();
        let syms = resolve_symbols(None, Some(&path)).unwrap();
        assert!(syms.contains(&"AAPL".to_string()), "AAPL must be present");
        assert!(syms.contains(&"SPY".to_string()), "SPY must be present");
    }

    // RS-06: providing both --symbols and --symbols-from-registry is a conflict error.
    #[test]
    fn rs_06_both_symbols_and_registry_is_conflict_error() {
        let path = canonical_registry_path();
        let result = resolve_symbols(Some("AAPL"), Some(&path));
        assert!(result.is_err(), "conflict must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("mutually exclusive"),
            "error must mention 'mutually exclusive': {msg}"
        );
    }

    // RS-07: providing neither --symbols nor --symbols-from-registry is an error.
    #[test]
    fn rs_07_neither_symbols_nor_registry_is_error() {
        let result = resolve_symbols(None, None);
        assert!(result.is_err(), "missing both must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("required"),
            "error must mention 'required': {msg}"
        );
    }

    // RS-08: missing registry file returns a clear error (not a panic).
    #[test]
    fn rs_08_missing_registry_file_returns_clear_error() {
        let bad_path = std::path::Path::new("/nonexistent/path/equities.json");
        let result = resolve_symbols(None, Some(bad_path));
        assert!(result.is_err(), "missing registry must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failed to load registry"),
            "error must mention 'failed to load registry': {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // md_registry_v2_status — ASSET-CORE-01C
    // -----------------------------------------------------------------------
    //
    // No DB connection, no provider/broker calls: `md_registry_v2_status`'s
    // signature takes only a `PathBuf` and its body calls nothing but
    // `mqk_md::instrument_registry{,_v2}` pure functions and `println!`.

    // RV2-01: registry-v2-status succeeds on the real canonical registry.
    #[test]
    fn rv2_01_registry_v2_status_succeeds_on_canonical_registry() {
        let path = canonical_registry_path();
        let result = md_registry_v2_status(path);
        assert!(
            result.is_ok(),
            "canonical registry must pass v1 load + v2 validation: {:?}",
            result.err()
        );
    }

    // RV2-02: missing registry file returns a clear, non-panicking error.
    #[test]
    fn rv2_02_registry_v2_status_missing_file_returns_clear_error() {
        let bad_path = std::path::PathBuf::from("/nonexistent/path/equities.json");
        let result = md_registry_v2_status(bad_path);
        assert!(result.is_err(), "missing registry must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("v1 load failed"),
            "error must mention 'v1 load failed': {msg}"
        );
    }

    // RV2-03: a v1 file that parses but converts to an invalid v2 shape
    // (ETF tagged with no sector/category) fails closed with a nonzero-exit
    // error, not a silent success.
    #[test]
    fn rv2_03_registry_v2_status_fails_closed_on_invalid_v2_shape() {
        let bad = mqk_md::instrument_registry::TrackedInstrument {
            instrument_id: "equity:US:SPY".to_string(),
            symbol: "SPY".to_string(),
            asset_class: "equity".to_string(),
            provider: "twelvedata".to_string(),
            provider_symbol: "SPY".to_string(),
            venue: "NYSEARCA".to_string(),
            currency: "USD".to_string(),
            enabled: true,
            timeframes: vec!["1D".to_string()],
            notes: "fixture".to_string(),
            instrument_kind: Some("etf".to_string()),
            sector: None,
            category: None,
        };
        let json = serde_json::to_string(&vec![bad]).unwrap();
        let path = std::env::temp_dir().join(format!(
            "mqk_test_cli_registry_v2_status_{}.json",
            std::process::id()
        ));
        fs::write(&path, json).expect("write temp fixture");

        let result = md_registry_v2_status(path.clone());
        fs::remove_file(&path).ok();

        assert!(result.is_err(), "invalid v2 shape must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("v2 validation failed"),
            "error must mention 'v2 validation failed': {msg}"
        );
    }
}
