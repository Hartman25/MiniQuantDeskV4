// core-rs/crates/mqk-db/src/md.rs
//
// PATCH B: CSV ingestion -> canonical md_bars + Data Quality Gate v1 report + persistence.
// PATCH C: Provider ingestion (mock/provider -> canonical md_bars) with same report shape.
//
// Design notes:
// - Canonical-only: we do NOT store raw vendor payloads.
// - Deterministic conversion: prices are parsed as decimal strings -> micros (i64) with no floats.
// - Safety policy for violations: we REJECT rows with invalid volume, invalid decimals, duplicates in batch,
//   out-of-order bars, or OHLC sanity violations. Rejected rows are counted in the quality report
//   and are not inserted/updated in md_bars.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Backtest loader (DB -> canonical rows)
// ---------------------------------------------------------------------------

/// Canonical md_bars row used by deterministic backtest loaders.
///
/// This is intentionally minimal and mirrors the md_bars table schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdBarRowBt {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
}

/// Load canonical bars from the `md_bars` table for use in backtesting.
///
/// Determinism guarantees:
/// - Rows are returned in stable order: `(end_ts ASC, symbol ASC)`.
/// - No implicit time sources are used.
///
/// Notes:
/// - `symbols` empty => loads all symbols for the timeframe/time-range.
/// - Uses sqlx `query()` + binds (no macros).
pub async fn load_md_bars_for_backtest(
    pool: &PgPool,
    timeframe: &str,
    start_end_ts_inclusive: i64,
    end_end_ts_inclusive: i64,
    symbols: &[String],
) -> Result<Vec<MdBarRowBt>> {
    load_md_bars_for_backtest_symbols(
        pool,
        timeframe,
        start_end_ts_inclusive,
        end_end_ts_inclusive,
        symbols,
    )
    .await
}

/// Like `load_md_bars_for_backtest`, but restricts to a symbol allowlist (deterministic order).
pub async fn load_md_bars_for_backtest_symbols(
    pool: &PgPool,
    timeframe: &str,
    start_end_ts_inclusive: i64,
    end_end_ts_inclusive: i64,
    symbols: &[String],
) -> Result<Vec<MdBarRowBt>> {
    // If symbols filter is empty, we skip the ANY($4) predicate.
    let rows = if symbols.is_empty() {
        sqlx::query(
            r#"
                select
                  symbol,
                  timeframe,
                  end_ts,
                  open_micros,
                  high_micros,
                  low_micros,
                  close_micros,
                  volume,
                  is_complete
                from md_bars
                where timeframe = $1
                  and end_ts >= $2
                  and end_ts <= $3
                order by end_ts asc, symbol asc
                "#,
        )
        .bind(timeframe)
        .bind(start_end_ts_inclusive)
        .bind(end_end_ts_inclusive)
        .fetch_all(pool)
        .await
        .context("load_md_bars_for_backtest query failed")?
    } else {
        sqlx::query(
            r#"
                select
                  symbol,
                  timeframe,
                  end_ts,
                  open_micros,
                  high_micros,
                  low_micros,
                  close_micros,
                  volume,
                  is_complete
                from md_bars
                where timeframe = $1
                  and end_ts >= $2
                  and end_ts <= $3
                  and symbol = any($4)
                order by end_ts asc, symbol asc
                "#,
        )
        .bind(timeframe)
        .bind(start_end_ts_inclusive)
        .bind(end_end_ts_inclusive)
        .bind(symbols)
        .fetch_all(pool)
        .await
        .context("load_md_bars_for_backtest query failed")?
    };

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(MdBarRowBt {
            symbol: r.try_get::<String, _>("symbol").context("md_bars.symbol")?,
            timeframe: r
                .try_get::<String, _>("timeframe")
                .context("md_bars.timeframe")?,
            end_ts: r.try_get::<i64, _>("end_ts").context("md_bars.end_ts")?,
            open_micros: r
                .try_get::<i64, _>("open_micros")
                .context("md_bars.open_micros")?,
            high_micros: r
                .try_get::<i64, _>("high_micros")
                .context("md_bars.high_micros")?,
            low_micros: r
                .try_get::<i64, _>("low_micros")
                .context("md_bars.low_micros")?,
            close_micros: r
                .try_get::<i64, _>("close_micros")
                .context("md_bars.close_micros")?,
            volume: r.try_get::<i64, _>("volume").context("md_bars.volume")?,
            is_complete: r
                .try_get::<bool, _>("is_complete")
                .context("md_bars.is_complete")?,
        });
    }
    Ok(out)
}

// ===== PATCH B/C: Deterministic md_bars ingestion + quality reporting =====

#[derive(Debug, Clone)]
pub struct IngestCsvArgs {
    pub path: PathBuf,
    /// Expected timeframe (e.g. "1D"). Rows with a different timeframe are rejected.
    pub timeframe: String,
    /// Source label for report (e.g. "csv").
    pub source: String,
    /// Caller-provided ingest_id for idempotent retries.
    /// Derive deterministically (e.g. UUIDv5 from source + path + timeframe).
    pub ingest_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageTotals {
    pub rows_read: u64,
    pub rows_ok: u64,
    pub rows_rejected: u64,
    pub rows_inserted: u64,
    pub rows_updated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectCounts {
    pub bad_timeframe: u64,
    pub bad_symbol: u64,
    pub bad_date: u64,
    pub bad_price: u64,
    pub bad_volume: u64,
    pub ohlc_sanity: u64,
    pub duplicate_in_batch: u64,
    pub out_of_order_in_batch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolCoverageStats {
    pub first_end_ts: Option<i64>,
    pub last_end_ts: Option<i64>,
    pub bars_ok: u64,
    pub bars_rejected: u64,
    /// Count of missing weekdays between first and last (1D only).
    pub missing_weekdays_est: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageQualityReport {
    pub ingest_id: Uuid,
    pub source: String,
    pub timeframe: String,
    pub symbols: Vec<String>,
    pub totals: CoverageTotals,
    pub rejects: RejectCounts,
    /// Per-symbol coverage stats (deterministic ordering by symbol).
    pub per_symbol: BTreeMap<String, SymbolCoverageStats>,
}

/// Canonical md bar row used internally for ingestion before upsert.
/// Uses micros for prices for deterministic handling.
#[derive(Debug, Clone)]
pub struct MdBarIngestRow {
    symbol: String,
    timeframe: String,
    end_ts: i64,
    open_micros: i64,
    high_micros: i64,
    low_micros: i64,
    close_micros: i64,
    volume: i64,
    is_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderIngestArgs {
    pub timeframe: String,
    pub source: String,
    /// Caller-provided ingest_id for idempotent retries.
    /// Derive deterministically (e.g. UUIDv5 from source + timeframe).
    pub ingest_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderBar {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    /// Decimal string, e.g. "123.45" (no floats).
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: i64,
    pub is_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdBarProviderMetadata {
    pub provider_id: String,
    pub provider_source: Option<String>,
    pub provider_symbol: Option<String>,
    pub ingest_mode: Option<String>,
    pub provider_bar_id: Option<String>,
    pub provider_updated_at_utc: Option<DateTime<Utc>>,
}

impl MdBarProviderMetadata {
    pub fn unknown() -> Self {
        Self {
            provider_id: "unknown".to_string(),
            provider_source: None,
            provider_symbol: None,
            ingest_mode: None,
            provider_bar_id: None,
            provider_updated_at_utc: None,
        }
    }

    pub fn provider_id_or_unknown(provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        let provider_id = if provider_id.trim().is_empty() {
            "unknown".to_string()
        } else {
            provider_id
        };

        Self {
            provider_id,
            ..Self::unknown()
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestProviderBarsArgs {
    pub source: String,
    /// Expected timeframe (e.g. "1D"). Rows with a different timeframe are rejected.
    pub timeframe: String,
    /// Caller-provided ingest_id for idempotent retries.
    /// Derive deterministically (e.g. UUIDv5 from source + timeframe + symbols + date range).
    pub ingest_id: Uuid,
    pub bars: Vec<ProviderBar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdQualityGroupStats {
    pub bars_ok: u64,
    pub bars_rejected: u64,
    pub duplicates_in_batch: u64,
    pub out_of_order: u64,
    pub ohlc_sanity_violations: u64,
    pub negative_or_invalid_volume: u64,
    /// Weekday-only gap count (1D only).
    pub gaps_detected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdQualityReport {
    pub ingest_id: Uuid,
    pub source: String,
    pub timeframe: String,
    pub coverage: CoverageTotals,
    /// Key format: "{SYMBOL}|{TIMEFRAME}" (deterministic via BTreeMap).
    pub per_symbol_timeframe: BTreeMap<String, MdQualityGroupStats>,
}

#[derive(Debug, Clone)]
pub struct IngestResult {
    pub ingest_id: Uuid,
    pub report: MdQualityReport,
}

/// CSV -> md_bars ingestion. CSV must include headers:
/// symbol,timeframe,end_ts,open,high,low,close,volume,is_complete
fn count_missing_weekdays_between(prev_end_ts: i64, cur_end_ts: i64) -> u64 {
    if cur_end_ts <= prev_end_ts {
        return 0;
    }
    let prev_dt = DateTime::<Utc>::from_timestamp(prev_end_ts, 0);
    let cur_dt = DateTime::<Utc>::from_timestamp(cur_end_ts, 0);
    let (prev_dt, cur_dt) = match (prev_dt, cur_dt) {
        (Some(p), Some(c)) => (p, c),
        _ => return 0,
    };
    let mut d = prev_dt.date_naive().succ_opt();
    let end_date = cur_dt.date_naive();
    let mut gaps = 0_u64;
    while let Some(day) = d {
        if day >= end_date {
            break;
        }
        if is_weekday(day.weekday()) {
            gaps += 1;
        }
        d = day.succ_opt();
    }
    gaps
}

/// DB backup CSV row — columns exported by `mqk-db` backup tooling.
/// Prices are already in micros (i64); `is_complete` uses PostgreSQL `t`/`f` notation.
/// Extra columns (e.g. `ingested_at`) are ignored by the csv deserializer.
#[derive(Debug, Clone, Deserialize)]
struct DbBackupCsvRow {
    symbol: String,
    timeframe: String,
    end_ts: i64,
    open_micros: i64,
    high_micros: i64,
    low_micros: i64,
    close_micros: i64,
    volume: i64,
    #[serde(deserialize_with = "deserialize_pg_bool")]
    is_complete: bool,
    // ingested_at is intentionally absent — extra CSV columns are ignored
}

/// Deserialize PostgreSQL boolean notation: `t`/`f` as well as `true`/`false`/`1`/`0`/`yes`/`no`.
fn deserialize_pg_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    match s.trim().to_ascii_lowercase().as_str() {
        "t" | "true" | "1" | "yes" => Ok(true),
        "f" | "false" | "0" | "no" => Ok(false),
        other => Err(serde::de::Error::custom(format!(
            "unrecognized boolean value: '{other}'"
        ))),
    }
}

/// Convert i64 micros to a 6-decimal-place price string accepted by `price_to_micros`.
/// Round-trip is exact: micros_to_price_str(x) |> price_to_micros == x for all x >= 0.
fn micros_to_price_str(micros: i64) -> String {
    let int_part = micros / 1_000_000;
    let frac_part = micros % 1_000_000;
    format!("{int_part}.{frac_part:06}")
}

/// CSV -> md_bars ingestion.
///
/// Supports two CSV schemas detected from the header:
/// - **Provider artifact schema**: `symbol,timeframe,end_ts,open,high,low,close,volume,is_complete`
///   (decimal-string prices; `is_complete` as `true`/`false`)
/// - **DB backup schema**: `symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete[,ingested_at]`
///   (integer-micro prices; `is_complete` as PostgreSQL `t`/`f` or `true`/`false`)
///
/// Schema detection: header column `open_micros` present → backup schema; otherwise provider schema.
pub async fn ingest_csv_to_md_bars(pool: &PgPool, args: IngestCsvArgs) -> Result<IngestResult> {
    let csv = std::fs::read_to_string(&args.path)
        .with_context(|| format!("read ingest csv failed: {}", args.path.display()))?;

    // Detect schema from header line.
    let is_db_backup = csv
        .lines()
        .next()
        .map(|h| h.to_ascii_lowercase().contains("open_micros"))
        .unwrap_or(false);

    let bars: Vec<ProviderBar> = if is_db_backup {
        let mut rdr = csv::Reader::from_reader(csv.as_bytes());
        let mut result = Vec::new();
        for rec in rdr.deserialize() {
            let b: DbBackupCsvRow = rec.context("deserialize DbBackupCsvRow failed")?;
            result.push(ProviderBar {
                symbol: b.symbol,
                timeframe: b.timeframe,
                end_ts: b.end_ts,
                open: micros_to_price_str(b.open_micros),
                high: micros_to_price_str(b.high_micros),
                low: micros_to_price_str(b.low_micros),
                close: micros_to_price_str(b.close_micros),
                volume: b.volume,
                is_complete: b.is_complete,
            });
        }
        result
    } else {
        let mut rdr = csv::Reader::from_reader(csv.as_bytes());
        let mut result = Vec::new();
        for rec in rdr.deserialize() {
            let b: ProviderBar = rec.context("deserialize ProviderBar failed")?;
            result.push(b);
        }
        result
    };

    // CRYPTO-DATA-01F: stamp truthful provider metadata instead of falling through
    // to the metadata-less helper's "unknown" default. provider_id/provider_source
    // are the operator-supplied --source label (the only provenance the CLI is
    // given); ingest_mode records the true ingestion mechanism ("csv_import").
    // provider_symbol is left unset: it is a single value applied to every row in
    // the batch, and a CSV file is not guaranteed to be single-symbol.
    let provider_metadata = MdBarProviderMetadata {
        provider_source: Some(args.source.clone()),
        ingest_mode: Some("csv_import".to_string()),
        ..MdBarProviderMetadata::provider_id_or_unknown(args.source.clone())
    };

    ingest_provider_bars_to_md_bars_with_provider_metadata(
        pool,
        IngestProviderBarsArgs {
            source: args.source,
            timeframe: args.timeframe,
            ingest_id: args.ingest_id,
            bars,
        },
        provider_metadata,
    )
    .await
}

pub async fn ingest_provider_bars_to_md_bars(
    pool: &PgPool,
    args: IngestProviderBarsArgs,
) -> Result<IngestResult> {
    ingest_provider_bars_to_md_bars_inner(pool, args, MdBarProviderMetadata::unknown()).await
}

pub async fn ingest_provider_bars_to_md_bars_with_provider_metadata(
    pool: &PgPool,
    args: IngestProviderBarsArgs,
    provider_metadata: MdBarProviderMetadata,
) -> Result<IngestResult> {
    ingest_provider_bars_to_md_bars_inner(pool, args, provider_metadata).await
}

async fn ingest_provider_bars_to_md_bars_inner(
    pool: &PgPool,
    args: IngestProviderBarsArgs,
    provider_metadata: MdBarProviderMetadata,
) -> Result<IngestResult> {
    let ingest_id = args.ingest_id;
    let provider_id = if provider_metadata.provider_id.trim().is_empty() {
        "unknown".to_string()
    } else {
        provider_metadata.provider_id
    };

    let mut coverage = CoverageTotals {
        rows_read: 0,
        rows_ok: 0,
        rows_rejected: 0,
        rows_inserted: 0,
        rows_updated: 0,
    };

    // Deterministic stats map.
    let mut per: BTreeMap<String, MdQualityGroupStats> = BTreeMap::new();

    // Duplicate detection within the batch.
    let mut seen_keys: BTreeSet<(String, String, i64)> = BTreeSet::new();

    // Out-of-order and gap tracking per (symbol,timeframe) in *input order*.
    let mut last_end_ts_seen: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut last_ok_end_ts: BTreeMap<(String, String), i64> = BTreeMap::new();

    for b in args.bars {
        coverage.rows_read += 1;

        let group_key = format!("{}|{}", b.symbol, b.timeframe);
        let st = per.entry(group_key.clone()).or_insert(MdQualityGroupStats {
            bars_ok: 0,
            bars_rejected: 0,
            duplicates_in_batch: 0,
            out_of_order: 0,
            ohlc_sanity_violations: 0,
            negative_or_invalid_volume: 0,
            gaps_detected: 0,
        });

        // Timeframe gate.
        if b.timeframe != args.timeframe {
            st.bars_rejected += 1;
            coverage.rows_rejected += 1;
            continue;
        }

        // Duplicate gate.
        let dup_key = (b.symbol.clone(), b.timeframe.clone(), b.end_ts);
        if !seen_keys.insert(dup_key) {
            st.duplicates_in_batch += 1;
            st.bars_rejected += 1;
            coverage.rows_rejected += 1;
            continue;
        }

        // Out-of-order gate (input order per group).
        let k = (b.symbol.clone(), b.timeframe.clone());
        if let Some(prev) = last_end_ts_seen.get(&k) {
            if b.end_ts < *prev {
                st.out_of_order += 1;
                st.bars_rejected += 1;
                coverage.rows_rejected += 1;
                // Still update last_end_ts_seen so the rule is monotonic within the batch.
                last_end_ts_seen.insert(k, b.end_ts);
                continue;
            }
        }
        last_end_ts_seen.insert(k.clone(), b.end_ts);

        // Volume gate.
        if b.volume < 0 {
            st.negative_or_invalid_volume += 1;
            st.bars_rejected += 1;
            coverage.rows_rejected += 1;
            continue;
        }

        // Price parse + OHLC sanity.
        let open_micros = price_to_micros(&b.open).context("open parse")?;
        let high_micros = price_to_micros(&b.high).context("high parse")?;
        let low_micros = price_to_micros(&b.low).context("low parse")?;
        let close_micros = price_to_micros(&b.close).context("close parse")?;

        let mut ohlc_bad = false;
        if low_micros > high_micros {
            ohlc_bad = true;
        }
        if !(low_micros <= open_micros && open_micros <= high_micros) {
            ohlc_bad = true;
        }
        if !(low_micros <= close_micros && close_micros <= high_micros) {
            ohlc_bad = true;
        }
        if ohlc_bad {
            st.ohlc_sanity_violations += 1;
            st.bars_rejected += 1;
            coverage.rows_rejected += 1;
            continue;
        }

        // Gap detection for 1D (weekday-only) based on ACCEPTED bars in input order.
        if args.timeframe == "1D" {
            if let Some(prev_ok) = last_ok_end_ts.get(&k) {
                st.gaps_detected = st
                    .gaps_detected
                    .saturating_add(count_missing_weekdays_between(*prev_ok, b.end_ts));
            }
        }
        last_ok_end_ts.insert(k.clone(), b.end_ts);

        // Upsert canonical md_bars.
        // inserted = (xmax = 0) in Postgres (true on insert, false on update).
        let inserted: bool = sqlx::query_scalar(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts,
              open_micros, high_micros, low_micros, close_micros,
              volume, is_complete,
              provider_id, provider_source, provider_symbol, ingest_mode,
              provider_bar_id, provider_updated_at_utc
            ) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            on conflict (symbol, timeframe, end_ts) do update set
              open_micros = excluded.open_micros,
              high_micros = excluded.high_micros,
              low_micros = excluded.low_micros,
              close_micros = excluded.close_micros,
              volume = excluded.volume,
              is_complete = excluded.is_complete,
              provider_id = excluded.provider_id,
              provider_source = excluded.provider_source,
              provider_symbol = excluded.provider_symbol,
              ingest_mode = excluded.ingest_mode,
              provider_bar_id = excluded.provider_bar_id,
              provider_updated_at_utc = excluded.provider_updated_at_utc
            returning (xmax = 0)
            "#,
        )
        .bind(&b.symbol)
        .bind(&b.timeframe)
        .bind(b.end_ts)
        .bind(open_micros)
        .bind(high_micros)
        .bind(low_micros)
        .bind(close_micros)
        .bind(b.volume)
        .bind(b.is_complete)
        .bind(&provider_id)
        .bind(&provider_metadata.provider_source)
        .bind(&provider_metadata.provider_symbol)
        .bind(&provider_metadata.ingest_mode)
        .bind(&provider_metadata.provider_bar_id)
        .bind(provider_metadata.provider_updated_at_utc)
        .fetch_one(pool)
        .await
        .context("upsert md_bars failed")?;

        coverage.rows_ok += 1;
        st.bars_ok += 1;
        if inserted {
            coverage.rows_inserted += 1;
        } else {
            coverage.rows_updated += 1;
        }
    }

    // Each group should have bars_rejected consistent with totals.
    // (We only track per-group rejections we observe; coverage is authoritative.)
    // Persist a single quality report row per ingest_id (idempotent).
    let report = MdQualityReport {
        ingest_id,
        source: args.source,
        timeframe: args.timeframe,
        coverage,
        per_symbol_timeframe: per,
    };

    let stats_json =
        serde_json::to_value(&report).context("serialize MdQualityReport to json failed")?;

    sqlx::query(
        r#"
        insert into md_quality_reports (ingest_id, stats_json)
        values ($1, $2)
        on conflict (ingest_id) do update set
          stats_json = excluded.stats_json
        "#,
    )
    .bind(ingest_id)
    .bind(stats_json)
    .execute(pool)
    .await
    .context("persist md_quality_reports failed")?;

    Ok(IngestResult { ingest_id, report })
}
pub async fn ingest_md_bars_csv(
    pool: &PgPool,
    args: IngestCsvArgs,
) -> Result<CoverageQualityReport> {
    let ingest_id = args.ingest_id;

    let mut report = CoverageQualityReport {
        ingest_id,
        source: args.source.clone(),
        timeframe: args.timeframe.clone(),
        symbols: vec![],
        totals: CoverageTotals {
            rows_read: 0,
            rows_ok: 0,
            rows_rejected: 0,
            rows_inserted: 0,
            rows_updated: 0,
        },
        rejects: RejectCounts {
            bad_timeframe: 0,
            bad_symbol: 0,
            bad_date: 0,
            bad_price: 0,
            bad_volume: 0,
            ohlc_sanity: 0,
            duplicate_in_batch: 0,
            out_of_order_in_batch: 0,
        },
        per_symbol: BTreeMap::new(),
    };

    let file = File::open(&args.path)
        .with_context(|| format!("open csv path failed: {}", args.path.display()))?;
    let mut rdr = csv::Reader::from_reader(file);

    // Track duplicates in batch deterministically via HashSet of (symbol,timeframe,end_ts).
    let mut seen_keys: HashSet<(String, String, i64)> = HashSet::new();
    let mut per_symbol_last_ts: HashMap<String, i64> = HashMap::new();

    let mut ok_rows: Vec<MdBarIngestRow> = Vec::new();

    for rec in rdr.deserialize::<CsvMdBarRow>() {
        report.totals.rows_read += 1;

        let row = match rec {
            Ok(r) => r,
            Err(_) => {
                // Treat parse failures as bad_date for simplicity (deterministic bucket).
                report.rejects.bad_date += 1;
                report.totals.rows_rejected += 1;
                continue;
            }
        };

        // Timeframe match gate
        if row.timeframe != args.timeframe {
            report.rejects.bad_timeframe += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &row.symbol);
            continue;
        }

        // Symbol non-empty
        if row.symbol.trim().is_empty() {
            report.rejects.bad_symbol += 1;
            report.totals.rows_rejected += 1;
            continue;
        }

        // end_ts must be positive-ish (allow 0? reject <=0 for safety)
        if row.end_ts <= 0 {
            report.rejects.bad_date += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &row.symbol);
            continue;
        }

        let open_micros = match price_to_micros(&row.open) {
            Ok(v) => v,
            Err(_) => {
                report.rejects.bad_price += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &row.symbol);
                continue;
            }
        };
        let high_micros = match price_to_micros(&row.high) {
            Ok(v) => v,
            Err(_) => {
                report.rejects.bad_price += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &row.symbol);
                continue;
            }
        };
        let low_micros = match price_to_micros(&row.low) {
            Ok(v) => v,
            Err(_) => {
                report.rejects.bad_price += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &row.symbol);
                continue;
            }
        };
        let close_micros = match price_to_micros(&row.close) {
            Ok(v) => v,
            Err(_) => {
                report.rejects.bad_price += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &row.symbol);
                continue;
            }
        };

        // volume must be >= 0
        if row.volume < 0 {
            report.rejects.bad_volume += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &row.symbol);
            continue;
        }

        // OHLC sanity
        if !ohlc_sane(open_micros, high_micros, low_micros, close_micros) {
            report.rejects.ohlc_sanity += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &row.symbol);
            continue;
        }

        // Duplicate in batch
        let key = (row.symbol.clone(), row.timeframe.clone(), row.end_ts);
        if !seen_keys.insert(key) {
            report.rejects.duplicate_in_batch += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &row.symbol);
            continue;
        }

        // Out of order in batch per symbol (must be non-decreasing end_ts as read)
        if let Some(prev) = per_symbol_last_ts.get(&row.symbol) {
            if row.end_ts < *prev {
                report.rejects.out_of_order_in_batch += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &row.symbol);
                continue;
            }
        }
        per_symbol_last_ts.insert(row.symbol.clone(), row.end_ts);

        // OK
        report.totals.rows_ok += 1;
        bump_symbol_ok(&mut report, &row.symbol, row.end_ts);

        ok_rows.push(MdBarIngestRow {
            symbol: row.symbol,
            timeframe: row.timeframe,
            end_ts: row.end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume: row.volume,
            is_complete: row.is_complete,
        });
    }

    // Populate symbols list deterministically from per_symbol map keys.
    report.symbols = report.per_symbol.keys().cloned().collect();

    // Upsert into md_bars.
    // Note: deterministic behavior by processing ok_rows in the order they were accepted.
    // (caller can pre-sort csv if needed; ordering does not affect final DB state due to PK).
    for r in ok_rows {
        let res = sqlx::query(
            r#"
            insert into md_bars
              (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete, ingest_id)
            values
              ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (symbol, timeframe, end_ts) do update set
              open_micros = excluded.open_micros,
              high_micros = excluded.high_micros,
              low_micros = excluded.low_micros,
              close_micros = excluded.close_micros,
              volume = excluded.volume,
              is_complete = excluded.is_complete,
              ingest_id = excluded.ingest_id
            "#,
        )
        .bind(&r.symbol)
        .bind(&r.timeframe)
        .bind(r.end_ts)
        .bind(r.open_micros)
        .bind(r.high_micros)
        .bind(r.low_micros)
        .bind(r.close_micros)
        .bind(r.volume)
        .bind(r.is_complete)
        .bind(ingest_id)
        .execute(pool)
        .await?;

        // Postgres returns rows_affected == 1 both for insert and update, so we cannot
        // reliably distinguish without additional tricks. We keep counts best-effort:
        // attempt to detect insert vs update by checking if the row existed prior would be extra IO.
        // For now: treat as inserted count (deterministic).
        let _ = res;
        report.totals.rows_inserted += 1;
    }

    // Compute missing weekday estimates for 1D only.
    if args.timeframe == "1D" {
        for (_sym, stats) in report.per_symbol.iter_mut() {
            if let (Some(first), Some(last)) = (stats.first_end_ts, stats.last_end_ts) {
                stats.missing_weekdays_est = Some(estimate_missing_weekdays(first, last));
            }
        }
    }

    Ok(report)
}

#[derive(Debug, Clone, Deserialize)]
struct CsvMdBarRow {
    symbol: String,
    timeframe: String,
    end_ts: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: i64,
    is_complete: bool,
}

// Provider ingestion: for PATCH C, we assume caller already normalized and validated.
// We still do basic gating similar to CSV path for deterministic safety.
pub async fn ingest_md_bars_provider(
    pool: &PgPool,
    args: ProviderIngestArgs,
    rows: Vec<MdBarIngestRow>,
) -> Result<CoverageQualityReport> {
    let ingest_id = args.ingest_id;

    let mut report = CoverageQualityReport {
        ingest_id,
        source: args.source.clone(),
        timeframe: args.timeframe.clone(),
        symbols: vec![],
        totals: CoverageTotals {
            rows_read: 0,
            rows_ok: 0,
            rows_rejected: 0,
            rows_inserted: 0,
            rows_updated: 0,
        },
        rejects: RejectCounts {
            bad_timeframe: 0,
            bad_symbol: 0,
            bad_date: 0,
            bad_price: 0,
            bad_volume: 0,
            ohlc_sanity: 0,
            duplicate_in_batch: 0,
            out_of_order_in_batch: 0,
        },
        per_symbol: BTreeMap::new(),
    };

    let mut seen_keys: HashSet<(String, String, i64)> = HashSet::new();
    let mut per_symbol_last_ts: HashMap<String, i64> = HashMap::new();

    let mut ok_rows: Vec<MdBarIngestRow> = Vec::new();

    for r in rows {
        report.totals.rows_read += 1;

        if r.timeframe != args.timeframe {
            report.rejects.bad_timeframe += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &r.symbol);
            continue;
        }
        if r.symbol.trim().is_empty() {
            report.rejects.bad_symbol += 1;
            report.totals.rows_rejected += 1;
            continue;
        }
        if r.end_ts <= 0 {
            report.rejects.bad_date += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &r.symbol);
            continue;
        }
        if r.volume < 0 {
            report.rejects.bad_volume += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &r.symbol);
            continue;
        }
        if !ohlc_sane(r.open_micros, r.high_micros, r.low_micros, r.close_micros) {
            report.rejects.ohlc_sanity += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &r.symbol);
            continue;
        }

        let key = (r.symbol.clone(), r.timeframe.clone(), r.end_ts);
        if !seen_keys.insert(key) {
            report.rejects.duplicate_in_batch += 1;
            report.totals.rows_rejected += 1;
            bump_symbol_reject(&mut report, &r.symbol);
            continue;
        }

        if let Some(prev) = per_symbol_last_ts.get(&r.symbol) {
            if r.end_ts < *prev {
                report.rejects.out_of_order_in_batch += 1;
                report.totals.rows_rejected += 1;
                bump_symbol_reject(&mut report, &r.symbol);
                continue;
            }
        }
        per_symbol_last_ts.insert(r.symbol.clone(), r.end_ts);

        report.totals.rows_ok += 1;
        bump_symbol_ok(&mut report, &r.symbol, r.end_ts);

        ok_rows.push(r);
    }

    report.symbols = report.per_symbol.keys().cloned().collect();

    for r in ok_rows {
        let res = sqlx::query(
            r#"
            insert into md_bars
              (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete, ingest_id)
            values
              ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (symbol, timeframe, end_ts) do update set
              open_micros = excluded.open_micros,
              high_micros = excluded.high_micros,
              low_micros = excluded.low_micros,
              close_micros = excluded.close_micros,
              volume = excluded.volume,
              is_complete = excluded.is_complete,
              ingest_id = excluded.ingest_id
            "#,
        )
        .bind(&r.symbol)
        .bind(&r.timeframe)
        .bind(r.end_ts)
        .bind(r.open_micros)
        .bind(r.high_micros)
        .bind(r.low_micros)
        .bind(r.close_micros)
        .bind(r.volume)
        .bind(r.is_complete)
        .bind(ingest_id)
        .execute(pool)
        .await?;

        let _ = res;
        report.totals.rows_inserted += 1;
    }

    if args.timeframe == "1D" {
        for (_sym, stats) in report.per_symbol.iter_mut() {
            if let (Some(first), Some(last)) = (stats.first_end_ts, stats.last_end_ts) {
                stats.missing_weekdays_est = Some(estimate_missing_weekdays(first, last));
            }
        }
    }

    Ok(report)
}

fn bump_symbol_ok(report: &mut CoverageQualityReport, symbol: &str, end_ts: i64) {
    let entry = report
        .per_symbol
        .entry(symbol.to_string())
        .or_insert(SymbolCoverageStats {
            first_end_ts: None,
            last_end_ts: None,
            bars_ok: 0,
            bars_rejected: 0,
            missing_weekdays_est: None,
        });

    entry.bars_ok += 1;
    entry.first_end_ts = Some(match entry.first_end_ts {
        Some(v) => v.min(end_ts),
        None => end_ts,
    });
    entry.last_end_ts = Some(match entry.last_end_ts {
        Some(v) => v.max(end_ts),
        None => end_ts,
    });
}

fn bump_symbol_reject(report: &mut CoverageQualityReport, symbol: &str) {
    if symbol.trim().is_empty() {
        return;
    }
    let entry = report
        .per_symbol
        .entry(symbol.to_string())
        .or_insert(SymbolCoverageStats {
            first_end_ts: None,
            last_end_ts: None,
            bars_ok: 0,
            bars_rejected: 0,
            missing_weekdays_est: None,
        });

    entry.bars_rejected += 1;
}

/// Parse a decimal string into integer micros deterministically.
/// Accepts optional + sign. Rejects negative. Rejects > 6 decimal places.
/// Rejects any trailing precision beyond micros to avoid rounding ambiguity.
fn price_to_micros(s: &str) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("empty price"));
    }
    let s = if let Some(rest) = s.strip_prefix('+') {
        rest
    } else {
        s
    };
    if s.starts_with('-') {
        return Err(anyhow!("negative price not allowed"));
    }

    let mut parts = s.split('.');
    let int_part = parts.next().unwrap_or("0");
    let frac_part = parts.next();

    if parts.next().is_some() {
        return Err(anyhow!("invalid decimal format"));
    }

    // Disallow non-digit chars
    if !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("invalid integer part"));
    }

    let int_val: i64 = int_part
        .parse::<i64>()
        .with_context(|| format!("parse int part failed: {}", int_part))?;

    let micros = match frac_part {
        None => 0_i64,
        Some(frac) => {
            if frac.is_empty() {
                0
            } else {
                if !frac.chars().all(|c| c.is_ascii_digit()) {
                    return Err(anyhow!("invalid fractional part"));
                }
                if frac.len() > 6 {
                    // Reject rounding ambiguity
                    return Err(anyhow!("too many decimals"));
                }
                let mut frac_str = frac.to_string();
                while frac_str.len() < 6 {
                    frac_str.push('0');
                }
                frac_str
                    .parse::<i64>()
                    .with_context(|| format!("parse frac part failed: {}", frac_str))?
            }
        }
    };

    int_val
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(micros))
        .ok_or_else(|| anyhow!("price overflow"))
}

fn ohlc_sane(open: i64, high: i64, low: i64, close: i64) -> bool {
    if low > high {
        return false;
    }
    // high must be >= open, close
    if high < open || high < close {
        return false;
    }
    // low must be <= open, close
    if low > open || low > close {
        return false;
    }
    true
}

/// Estimate number of missing weekdays between two unix timestamps (inclusive bounds)
/// by converting to dates and counting weekdays not present.
/// This is only a rough estimate: assumes one bar per weekday for 1D.
fn estimate_missing_weekdays(first_end_ts: i64, last_end_ts: i64) -> u64 {
    let first_dt = DateTime::<Utc>::from_timestamp(first_end_ts, 0);
    let last_dt = DateTime::<Utc>::from_timestamp(last_end_ts, 0);
    let (first_dt, last_dt) = match (first_dt, last_dt) {
        (Some(f), Some(l)) => (f, l),
        _ => return 0,
    };

    let first_date: NaiveDate = first_dt.date_naive();
    let last_date: NaiveDate = last_dt.date_naive();

    if last_date < first_date {
        return 0;
    }

    // Count weekdays between dates inclusive, then subtract observed count would require actual data.
    // As an estimate, return 0 for now if range too small.
    let mut count = 0_u64;
    let mut d = first_date;
    while d <= last_date {
        if is_weekday(d.weekday()) {
            count += 1;
        }
        d = d.succ_opt().unwrap_or(d);
        if d == last_date.succ_opt().unwrap_or(d) {
            break;
        }
    }

    // We don't know observed bars here; this is just number of weekdays in range.
    // The caller can interpret as potential bars expected.
    // For this module, we use a more precise estimate based on actual accepted end_ts values elsewhere.
    count.saturating_sub(1) // conservative: at least one bar exists
}

// Additional utility used by quality report when we have actual observed end_ts list.
#[allow(dead_code)]
fn compute_gaps_1d(dates: &[NaiveDate]) -> u64 {
    if dates.len() < 2 {
        return 0;
    }

    let mut gaps = 0_u64;
    let mut prev_date: Option<NaiveDate> = None;

    for d in dates {
        if let Some(prev) = prev_date {
            let mut cur = prev.succ_opt();
            while let Some(day) = cur {
                if day >= *d {
                    break;
                }
                if is_weekday(day.weekday()) {
                    gaps += 1;
                }
                cur = day.succ_opt();
            }
        }

        prev_date = Some(*d);
    }

    gaps
}

fn is_weekday(w: Weekday) -> bool {
    !matches!(w, Weekday::Sat | Weekday::Sun)
}

// ===== PATCH BT2: Deterministic md_bars READ API =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdBarRow {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchMdBarsArgs {
    pub timeframe: String,
    pub symbols: Vec<String>,
    pub start_end_ts: Option<i64>,
    pub end_end_ts: Option<i64>,
    pub require_complete: bool,
}

pub async fn fetch_md_bars(
    pool: &sqlx::PgPool,
    args: FetchMdBarsArgs,
) -> anyhow::Result<Vec<MdBarRow>> {
    if args.symbols.is_empty() {
        return Err(anyhow!("symbols must be non-empty"));
    }

    if let (Some(start), Some(end)) = (args.start_end_ts, args.end_end_ts) {
        if start > end {
            return Err(anyhow!("start_end_ts must be <= end_end_ts"));
        }
    }

    let rows = sqlx::query(
        r#"
        select
            symbol,
            timeframe,
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete
        from md_bars
        where timeframe = $1
          and symbol = any($2)
          and ($3::bigint is null or end_ts >= $3)
          and ($4::bigint is null or end_ts <= $4)
          and (not $5 or is_complete = true)
        order by symbol asc, end_ts asc
        "#,
    )
    .bind(&args.timeframe)
    .bind(&args.symbols)
    .bind(args.start_end_ts)
    .bind(args.end_end_ts)
    .bind(args.require_complete)
    .fetch_all(pool)
    .await
    .context("fetch_md_bars query failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(MdBarRow {
            symbol: r.try_get::<String, _>("symbol")?,
            timeframe: r.try_get::<String, _>("timeframe")?,
            end_ts: r.try_get::<i64, _>("end_ts")?,
            open_micros: r.try_get::<i64, _>("open_micros")?,
            high_micros: r.try_get::<i64, _>("high_micros")?,
            low_micros: r.try_get::<i64, _>("low_micros")?,
            close_micros: r.try_get::<i64, _>("close_micros")?,
            volume: r.try_get::<i64, _>("volume")?,
            is_complete: r.try_get::<bool, _>("is_complete")?,
        });
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-RESULTS-01: md_bars coverage summary (read-only)
// ---------------------------------------------------------------------------

/// One row of the coverage summary: one (symbol, timeframe) group.
#[derive(Debug, Clone)]
pub struct MdCoverageRow {
    pub symbol: String,
    pub timeframe: String,
    /// Total bar count for this (symbol, timeframe) pair.
    pub bars: i64,
    pub min_end_ts: i64,
    pub max_end_ts: i64,
    /// RFC3339-formatted `max(ingested_at)` for this group. `None` if somehow null.
    pub latest_ingested_at: Option<String>,
}

/// Read-only aggregate query: count/min/max per (symbol, timeframe) group.
///
/// `timeframe = None`  — return all timeframes.
/// `timeframe = Some(tf)` — restrict to that timeframe only.
///
/// Ordered deterministically: `symbol ASC, timeframe ASC`.
/// No inserts, updates, or deletes.
pub async fn fetch_md_bars_coverage(
    pool: &PgPool,
    timeframe: Option<&str>,
) -> Result<Vec<MdCoverageRow>> {
    let rows = sqlx::query(
        r#"
        select
          symbol,
          timeframe,
          count(*) as bars,
          min(end_ts) as min_end_ts,
          max(end_ts) as max_end_ts,
          max(ingested_at) as latest_ingested_at
        from md_bars
        where ($1::text is null or timeframe = $1)
        group by symbol, timeframe
        order by symbol asc, timeframe asc
        "#,
    )
    .bind(timeframe)
    .fetch_all(pool)
    .await
    .context("fetch_md_bars_coverage query failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let latest: Option<DateTime<Utc>> = r
            .try_get::<Option<DateTime<Utc>>, _>("latest_ingested_at")
            .context("md_bars.latest_ingested_at")?;
        out.push(MdCoverageRow {
            symbol: r.try_get::<String, _>("symbol").context("md_bars.symbol")?,
            timeframe: r
                .try_get::<String, _>("timeframe")
                .context("md_bars.timeframe")?,
            bars: r.try_get::<i64, _>("bars").context("md_bars.bars")?,
            min_end_ts: r
                .try_get::<i64, _>("min_end_ts")
                .context("md_bars.min_end_ts")?,
            max_end_ts: r
                .try_get::<i64, _>("max_end_ts")
                .context("md_bars.max_end_ts")?,
            latest_ingested_at: latest.map(|t| t.to_rfc3339()),
        });
    }
    Ok(out)
}

/// AUTON-SIGNAL-CONTEXT-01: Load the N most recent *complete* bars for a given
/// (symbol, timeframe) pair, ordered oldest-to-newest (ascending `end_ts`).
///
/// Used to build a `RecentBarsWindow` for the autonomous strategy context.
/// Caller must check cardinality — an empty result means no completed bars
/// exist for this symbol/timeframe (surface `NO_MARKET_DATA`); fewer than
/// the strategy's LOOKBACK bars means `INSUFFICIENT_LOOKBACK`.
///
/// Read-only query: no inserts, updates, or deletes.
pub async fn fetch_recent_completed_bars_for_strategy(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    limit: i64,
) -> Result<Vec<MdBarRow>> {
    let rows = sqlx::query(
        r#"
        select
            symbol,
            timeframe,
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete
        from md_bars
        where symbol = $1
          and timeframe = $2
          and is_complete = true
        order by end_ts desc
        limit $3
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_completed_bars_for_strategy query failed")?;

    let mut out: Vec<MdBarRow> = rows
        .iter()
        .map(|r| -> Result<MdBarRow> {
            Ok(MdBarRow {
                symbol: r.try_get::<String, _>("symbol")?,
                timeframe: r.try_get::<String, _>("timeframe")?,
                end_ts: r.try_get::<i64, _>("end_ts")?,
                open_micros: r.try_get::<i64, _>("open_micros")?,
                high_micros: r.try_get::<i64, _>("high_micros")?,
                low_micros: r.try_get::<i64, _>("low_micros")?,
                close_micros: r.try_get::<i64, _>("close_micros")?,
                volume: r.try_get::<i64, _>("volume")?,
                is_complete: r.try_get::<bool, _>("is_complete")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // Reverse to oldest-first so callers can build a window without reversing again.
    out.reverse();
    Ok(out)
}

/// Return the maximum `end_ts` stored in `md_bars` for a given (symbol, timeframe) pair.
///
/// Returns `None` when no bars exist yet — the caller must require a `--full-start` date
/// before issuing an incremental fetch.  Returns `Some(ts)` when at least one bar is present;
/// the caller subtracts an overlap window before requesting new bars from the provider.
///
/// Uses a single `max(end_ts)` aggregate query with explicit NULL handling via sqlx scalar.
pub async fn latest_stored_bar_end_ts(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
) -> Result<Option<i64>> {
    let val: Option<i64> =
        sqlx::query_scalar("select max(end_ts) from md_bars where symbol = $1 and timeframe = $2")
            .bind(symbol)
            .bind(timeframe)
            .fetch_one(pool)
            .await
            .with_context(|| {
                format!("latest_stored_bar_end_ts query failed for {symbol}/{timeframe}")
            })?;
    Ok(val)
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01AB-AC: content-diff read helper + comparison for provider sync
// ---------------------------------------------------------------------------

/// One existing `md_bars` row, read for the purpose of content-diff
/// comparison against a candidate provider bar before deciding to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingMdBarForProviderSync {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
    pub provider_id: Option<String>,
    pub provider_source: Option<String>,
    pub provider_symbol: Option<String>,
    pub ingest_mode: Option<String>,
}

/// Read existing `md_bars` rows for exactly the given `(symbol, timeframe,
/// end_ts)` keys, keyed by `end_ts` for O(1) lookup by the caller's
/// candidate list.
///
/// Read-only. No migration, no schema change. Never touches
/// broker/order/OMS tables and never queries a broader range than the exact
/// candidate keys supplied. Returns an empty map (no query issued) when
/// `end_ts_keys` is empty.
pub async fn fetch_md_bars_for_provider_sync_keys(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts_keys: &[i64],
) -> Result<BTreeMap<i64, ExistingMdBarForProviderSync>> {
    if end_ts_keys.is_empty() {
        return Ok(BTreeMap::new());
    }

    let rows = sqlx::query(
        r#"
        select
          symbol, timeframe, end_ts,
          open_micros, high_micros, low_micros, close_micros,
          volume, is_complete,
          provider_id, provider_source, provider_symbol, ingest_mode
        from md_bars
        where symbol = $1
          and timeframe = $2
          and end_ts = any($3)
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts_keys)
    .fetch_all(pool)
    .await
    .context("fetch_md_bars_for_provider_sync_keys query failed")?;

    let mut out = BTreeMap::new();
    for r in rows {
        let end_ts: i64 = r.try_get("end_ts").context("md_bars.end_ts")?;
        out.insert(
            end_ts,
            ExistingMdBarForProviderSync {
                symbol: r.try_get("symbol").context("md_bars.symbol")?,
                timeframe: r.try_get("timeframe").context("md_bars.timeframe")?,
                end_ts,
                open_micros: r.try_get("open_micros").context("md_bars.open_micros")?,
                high_micros: r.try_get("high_micros").context("md_bars.high_micros")?,
                low_micros: r.try_get("low_micros").context("md_bars.low_micros")?,
                close_micros: r.try_get("close_micros").context("md_bars.close_micros")?,
                volume: r.try_get("volume").context("md_bars.volume")?,
                is_complete: r.try_get("is_complete").context("md_bars.is_complete")?,
                provider_id: r.try_get("provider_id").context("md_bars.provider_id")?,
                provider_source: r
                    .try_get("provider_source")
                    .context("md_bars.provider_source")?,
                provider_symbol: r
                    .try_get("provider_symbol")
                    .context("md_bars.provider_symbol")?,
                ingest_mode: r.try_get("ingest_mode").context("md_bars.ingest_mode")?,
            },
        );
    }
    Ok(out)
}

/// Compare a candidate `ProviderBar` plus its intended write-time
/// `MdBarProviderMetadata` against an existing `md_bars` row for the same
/// `(symbol, timeframe, end_ts)`.
///
/// Compares OHLCV, `is_complete`, and provider provenance (`provider_id`,
/// `provider_source`, `provider_symbol`, `ingest_mode`). Deliberately does
/// not compare `provider_bar_id` / `provider_updated_at_utc`: Kraken never
/// supplies either, and every write path here always leaves them `None`, so
/// comparing them would only ever compare `None == None` and add no signal.
///
/// Returns `Ok(true)` when content is identical (safe to skip the write).
pub fn provider_bar_matches_existing(
    candidate: &ProviderBar,
    metadata: &MdBarProviderMetadata,
    existing: &ExistingMdBarForProviderSync,
) -> Result<bool> {
    let open_micros = price_to_micros(&candidate.open).context("content-diff open parse")?;
    let high_micros = price_to_micros(&candidate.high).context("content-diff high parse")?;
    let low_micros = price_to_micros(&candidate.low).context("content-diff low parse")?;
    let close_micros = price_to_micros(&candidate.close).context("content-diff close parse")?;

    Ok(existing.open_micros == open_micros
        && existing.high_micros == high_micros
        && existing.low_micros == low_micros
        && existing.close_micros == close_micros
        && existing.volume == candidate.volume
        && existing.is_complete == candidate.is_complete
        && existing.provider_id.as_deref() == Some(metadata.provider_id.as_str())
        && existing.provider_source.as_deref() == metadata.provider_source.as_deref()
        && existing.provider_symbol.as_deref() == metadata.provider_symbol.as_deref()
        && existing.ingest_mode.as_deref() == metadata.ingest_mode.as_deref())
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED Phase B: bounded read with
// provenance, for the strict daily readiness evaluator.
// ---------------------------------------------------------------------------

/// One `md_bars` row with full provenance, for strict readiness continuity
/// checking. Superset of [`MdBarRow`]'s OHLCV fields plus every provenance
/// column §11 of the readiness contract requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdBarRowWithProvenance {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
    pub provider_id: String,
    pub provider_source: Option<String>,
    pub provider_symbol: Option<String>,
    pub ingest_mode: Option<String>,
    pub provider_updated_at_utc: Option<DateTime<Utc>>,
    pub ingested_at: DateTime<Utc>,
}

/// Read the most recent `limit` `md_bars` rows (any completeness) for a
/// `(symbol, timeframe)` pair, with full provenance, ordered oldest-to-newest
/// (ascending `end_ts`).
///
/// Bounded: `limit` must be the caller's `required_history_bars + 2` (or an
/// equally bounded, explicitly justified amount) — this function does not
/// enforce a maximum itself, callers own the bound. Read-only: no inserts,
/// updates, or deletes.
pub async fn fetch_bounded_bars_with_provenance(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    limit: i64,
) -> Result<Vec<MdBarRowWithProvenance>> {
    let rows = sqlx::query(
        r#"
        select
            symbol,
            timeframe,
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete,
            provider_id,
            provider_source,
            provider_symbol,
            ingest_mode,
            provider_updated_at_utc,
            ingested_at
        from md_bars
        where symbol = $1
          and timeframe = $2
        order by end_ts desc
        limit $3
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_bounded_bars_with_provenance query failed")?;

    let mut out: Vec<MdBarRowWithProvenance> = rows
        .iter()
        .map(|r| -> Result<MdBarRowWithProvenance> {
            Ok(MdBarRowWithProvenance {
                symbol: r.try_get("symbol").context("md_bars.symbol")?,
                timeframe: r.try_get("timeframe").context("md_bars.timeframe")?,
                end_ts: r.try_get("end_ts").context("md_bars.end_ts")?,
                open_micros: r.try_get("open_micros").context("md_bars.open_micros")?,
                high_micros: r.try_get("high_micros").context("md_bars.high_micros")?,
                low_micros: r.try_get("low_micros").context("md_bars.low_micros")?,
                close_micros: r.try_get("close_micros").context("md_bars.close_micros")?,
                volume: r.try_get("volume").context("md_bars.volume")?,
                is_complete: r.try_get("is_complete").context("md_bars.is_complete")?,
                provider_id: r.try_get("provider_id").context("md_bars.provider_id")?,
                provider_source: r
                    .try_get("provider_source")
                    .context("md_bars.provider_source")?,
                provider_symbol: r
                    .try_get("provider_symbol")
                    .context("md_bars.provider_symbol")?,
                ingest_mode: r.try_get("ingest_mode").context("md_bars.ingest_mode")?,
                provider_updated_at_utc: r
                    .try_get("provider_updated_at_utc")
                    .context("md_bars.provider_updated_at_utc")?,
                ingested_at: r.try_get("ingested_at").context("md_bars.ingested_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // Reverse to oldest-first so callers get a natural ascending window
    // without re-sorting (mirrors fetch_recent_completed_bars_for_strategy).
    out.reverse();
    Ok(out)
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-POLL-REEVALUATE-AND-PROVIDER-CLOSURE-01
// REPAIR 3: exact expected-bar lookup, so the autonomous completed-bar driver
// can avoid a provider call whenever the exact expected canonical bar
// already exists in `md_bars` — a single-row equality query, not a window
// scan of `fetch_bounded_bars_with_provenance`.
// ---------------------------------------------------------------------------

/// Read the single `md_bars` row for `(symbol, timeframe, end_ts)`, with full
/// provenance, or `None` if no such row exists. Read-only: no inserts,
/// updates, or deletes. Unlike [`fetch_bounded_bars_with_provenance`], this
/// is an exact-identity lookup, not a bounded recent-window scan — callers
/// that need to know whether one specific canonical bar identity is already
/// present (rather than the shape of a recent window) should use this
/// function.
pub async fn fetch_exact_bar_with_provenance(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
) -> Result<Option<MdBarRowWithProvenance>> {
    let row = sqlx::query(
        r#"
        select
            symbol,
            timeframe,
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete,
            provider_id,
            provider_source,
            provider_symbol,
            ingest_mode,
            provider_updated_at_utc,
            ingested_at
        from md_bars
        where symbol = $1
          and timeframe = $2
          and end_ts = $3
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .fetch_optional(pool)
    .await
    .context("fetch_exact_bar_with_provenance query failed")?;

    row.map(|r| -> Result<MdBarRowWithProvenance> {
        Ok(MdBarRowWithProvenance {
            symbol: r.try_get("symbol").context("md_bars.symbol")?,
            timeframe: r.try_get("timeframe").context("md_bars.timeframe")?,
            end_ts: r.try_get("end_ts").context("md_bars.end_ts")?,
            open_micros: r.try_get("open_micros").context("md_bars.open_micros")?,
            high_micros: r.try_get("high_micros").context("md_bars.high_micros")?,
            low_micros: r.try_get("low_micros").context("md_bars.low_micros")?,
            close_micros: r.try_get("close_micros").context("md_bars.close_micros")?,
            volume: r.try_get("volume").context("md_bars.volume")?,
            is_complete: r.try_get("is_complete").context("md_bars.is_complete")?,
            provider_id: r.try_get("provider_id").context("md_bars.provider_id")?,
            provider_source: r
                .try_get("provider_source")
                .context("md_bars.provider_source")?,
            provider_symbol: r
                .try_get("provider_symbol")
                .context("md_bars.provider_symbol")?,
            ingest_mode: r.try_get("ingest_mode").context("md_bars.ingest_mode")?,
            provider_updated_at_utc: r
                .try_get("provider_updated_at_utc")
                .context("md_bars.provider_updated_at_utc")?,
            ingested_at: r.try_get("ingested_at").context("md_bars.ingested_at")?,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_candidate() -> ProviderBar {
        ProviderBar {
            symbol: "BTC/USD".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 1_783_209_600,
            open: "62539.0".to_string(),
            high: "63443.4".to_string(),
            low: "62290.9".to_string(),
            close: "63085.8".to_string(),
            volume: 131_715_941_434,
            is_complete: true,
        }
    }

    fn sample_metadata() -> MdBarProviderMetadata {
        MdBarProviderMetadata {
            provider_id: "kraken".to_string(),
            provider_source: Some("kraken".to_string()),
            provider_symbol: Some("XXBTZUSD".to_string()),
            ingest_mode: Some("provider_sync".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        }
    }

    fn sample_existing_matching() -> ExistingMdBarForProviderSync {
        ExistingMdBarForProviderSync {
            symbol: "BTC/USD".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 1_783_209_600,
            open_micros: 62_539_000_000,
            high_micros: 63_443_400_000,
            low_micros: 62_290_900_000,
            close_micros: 63_085_800_000,
            volume: 131_715_941_434,
            is_complete: true,
            provider_id: Some("kraken".to_string()),
            provider_source: Some("kraken".to_string()),
            provider_symbol: Some("XXBTZUSD".to_string()),
            ingest_mode: Some("provider_sync".to_string()),
        }
    }

    #[test]
    fn provider_bar_matches_existing_true_when_identical() {
        assert!(provider_bar_matches_existing(
            &sample_candidate(),
            &sample_metadata(),
            &sample_existing_matching()
        )
        .unwrap());
    }

    #[test]
    fn provider_bar_matches_existing_false_when_close_differs() {
        let mut existing = sample_existing_matching();
        existing.close_micros = 60_000_000_000;
        assert!(
            !provider_bar_matches_existing(&sample_candidate(), &sample_metadata(), &existing)
                .unwrap()
        );
    }

    #[test]
    fn provider_bar_matches_existing_false_when_volume_differs() {
        let mut existing = sample_existing_matching();
        existing.volume = 1_000_000_000;
        assert!(
            !provider_bar_matches_existing(&sample_candidate(), &sample_metadata(), &existing)
                .unwrap()
        );
    }

    #[test]
    fn provider_bar_matches_existing_false_when_provider_symbol_differs() {
        let mut existing = sample_existing_matching();
        existing.provider_symbol = Some("seed_stale".to_string());
        assert!(
            !provider_bar_matches_existing(&sample_candidate(), &sample_metadata(), &existing)
                .unwrap()
        );
    }

    #[test]
    fn provider_bar_matches_existing_false_when_ingest_mode_differs() {
        let mut existing = sample_existing_matching();
        existing.ingest_mode = Some("provider_ingest".to_string());
        assert!(
            !provider_bar_matches_existing(&sample_candidate(), &sample_metadata(), &existing)
                .unwrap()
        );
    }

    #[test]
    fn md_bar_provider_metadata_unknown_defaults_to_unknown_id() {
        let metadata = MdBarProviderMetadata::unknown();

        assert_eq!(metadata.provider_id, "unknown");
        assert_eq!(metadata.provider_source, None);
        assert_eq!(metadata.provider_symbol, None);
        assert_eq!(metadata.ingest_mode, None);
        assert_eq!(metadata.provider_bar_id, None);
        assert_eq!(metadata.provider_updated_at_utc, None);
    }

    #[test]
    fn md_bar_provider_metadata_blank_provider_id_fails_closed_to_unknown() {
        let metadata = MdBarProviderMetadata::provider_id_or_unknown("   ");

        assert_eq!(metadata.provider_id, "unknown");
    }

    // --- micros_to_price_str round-trip ---

    #[test]
    fn micros_to_price_str_round_trip() {
        for micros in [0_i64, 531250, 100_500_000, 1_000_000, 999_999_999_999] {
            let s = micros_to_price_str(micros);
            let back = price_to_micros(&s)
                .unwrap_or_else(|e| panic!("round-trip failed for {micros}: {e}"));
            assert_eq!(back, micros, "round-trip mismatch for {micros} → '{s}'");
        }
    }

    #[test]
    fn micros_to_price_str_format() {
        assert_eq!(micros_to_price_str(531250), "0.531250");
        assert_eq!(micros_to_price_str(100_500_000), "100.500000");
        assert_eq!(micros_to_price_str(0), "0.000000");
        assert_eq!(micros_to_price_str(1_000_000), "1.000000");
    }

    // --- deserialize_pg_bool (via DbBackupCsvRow CSV parse) ---

    #[test]
    fn pg_bool_csv_parses_t_and_f() {
        // DB backup format uses t/f from PostgreSQL COPY export.
        let csv = "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete,ingested_at\n\
                   AAPL,1D,726105600,531250,535713,515625,520088,129136000,t,2026-04-19 12:21:42Z\n\
                   AAPL,1D,726192000,517857,529017,511161,529017,186256000,f,2026-04-19 12:21:42Z\n";
        let mut rdr = csv::Reader::from_reader(csv.as_bytes());
        let rows: Vec<DbBackupCsvRow> = rdr.deserialize().map(|r| r.expect("parse row")).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_complete, "t → true");
        assert!(!rows[1].is_complete, "f → false");
    }

    #[test]
    fn pg_bool_csv_parses_true_and_false() {
        let csv = "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n\
                   SPY,1D,1708041600,100500000,101000000,99500000,100750000,1000,true\n\
                   SPY,1D,1708128000,100500000,101000000,99500000,100750000,1000,false\n";
        let mut rdr = csv::Reader::from_reader(csv.as_bytes());
        let rows: Vec<DbBackupCsvRow> = rdr.deserialize().map(|r| r.expect("parse row")).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_complete);
        assert!(!rows[1].is_complete);
    }

    #[test]
    fn price_to_micros_basic() {
        assert_eq!(price_to_micros("0").unwrap(), 0);
        assert_eq!(price_to_micros("1").unwrap(), 1_000_000);
        assert_eq!(price_to_micros("1.0").unwrap(), 1_000_000);
        assert_eq!(price_to_micros("1.23").unwrap(), 1_230_000);
        assert_eq!(price_to_micros("001.2300").unwrap(), 1_230_000);
        assert_eq!(price_to_micros("+5.000001").unwrap(), 5_000_001);
    }

    #[test]
    fn price_to_micros_rejects_rounding_ambiguity() {
        assert!(price_to_micros("1.0000000").is_err());
        assert!(price_to_micros("1.1234567").is_err());
    }

    #[test]
    fn ohlc_sane_rules() {
        let o = 10_000_000;
        let c = 11_000_000;
        let h = 12_000_000;
        let l = 9_000_000;
        assert!(ohlc_sane(o, h, l, c));

        // High too low
        assert!(!ohlc_sane(o, 10_500_000, l, c));

        // Low too high
        assert!(!ohlc_sane(o, h, 10_500_000, c));

        // Low > High
        assert!(!ohlc_sane(o, l, h, c));
    }
}
