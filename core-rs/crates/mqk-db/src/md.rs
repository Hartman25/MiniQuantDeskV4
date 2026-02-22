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
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IngestCsvArgs {
    pub path: PathBuf,
    /// Expected timeframe (e.g. "1D"). Rows with a different timeframe are rejected.
    pub timeframe: String,
    /// Source label for report (e.g. "csv").
    pub source: String,
    /// Optional caller-provided ingest_id for idempotent retries.
    pub ingest_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageTotals {
    pub rows_read: u64,
    pub rows_ok: u64,
    pub rows_rejected: u64,
    pub rows_inserted: u64,
    pub rows_updated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupStats {
    pub duplicates_in_batch: u64,
    pub out_of_order: u64,
    pub ohlc_sanity_violations: u64,
    pub negative_or_invalid_volume: u64,
    pub incomplete_bars: u64,
    pub gaps_detected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdQualityReport {
    pub ingest_id: Uuid,
    pub created_at_utc: String,
    pub source: String,
    pub coverage: CoverageTotals,
    /// per (symbol,timeframe)
    pub per_symbol_timeframe: BTreeMap<String, GroupStats>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IngestResult {
    pub ingest_id: Uuid,
    pub report: MdQualityReport,
}

#[derive(Debug, Clone)]
struct GroupState {
    stats: GroupStats,
    seen_end_ts: HashSet<i64>,
    last_end_ts_in_batch_order: Option<i64>,
    ok_end_ts: Vec<i64>,
}

impl GroupState {
    fn new() -> Self {
        Self {
            stats: GroupStats::default(),
            seen_end_ts: HashSet::new(),
            last_end_ts_in_batch_order: None,
            ok_end_ts: Vec::new(),
        }
    }
}

/// Ingest a CSV file into md_bars (canonical table) and persist a Data Quality Gate v1 report.
///
/// CSV format (headers case-insensitive; order can vary):
/// symbol,timeframe,end_ts,open,high,low,close,volume,is_complete
///
/// - end_ts is epoch seconds UTC
/// - open/high/low/close are decimal strings
/// - volume is integer (must be >= 0; invalid/negative rows are rejected)
/// - is_complete accepts true/false/1/0/yes/no (case-insensitive)
pub async fn ingest_csv_to_md_bars(pool: &PgPool, args: IngestCsvArgs) -> Result<IngestResult> {
    let ingest_id = args.ingest_id.unwrap_or_else(Uuid::new_v4);
    let created_at = Utc::now();

    let file = File::open(&args.path)
        .with_context(|| format!("open csv failed: {}", args.path.display()))?;

    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(file);

    let headers = rdr.headers().context("csv must have a header row")?.clone();

    let col = HeaderMap::from_headers(&headers)?;

    // Totals
    let mut totals = CoverageTotals {
        rows_read: 0,
        rows_ok: 0,
        rows_rejected: 0,
        rows_inserted: 0,
        rows_updated: 0,
    };

    // Per-group state
    let mut groups: HashMap<(String, String), GroupState> = HashMap::new();

    for rec in rdr.records() {
        let rec = rec.context("csv read record failed")?;
        totals.rows_read += 1;

        let symbol = col.get(&rec, "symbol")?;
        let timeframe = col.get(&rec, "timeframe")?;
        let end_ts_s = col.get(&rec, "end_ts")?;
        let open_s = col.get(&rec, "open")?;
        let high_s = col.get(&rec, "high")?;
        let low_s = col.get(&rec, "low")?;
        let close_s = col.get(&rec, "close")?;
        let volume_s = col.get(&rec, "volume")?;
        let is_complete_s = col.get(&rec, "is_complete")?;

        if !timeframe.eq_ignore_ascii_case(&args.timeframe) {
            totals.rows_rejected += 1;
            continue;
        }

        let key = (symbol.to_string(), args.timeframe.clone());
        let st = groups.entry(key.clone()).or_insert_with(GroupState::new);

        // Parse end_ts
        let end_ts: i64 = match end_ts_s.parse::<i64>() {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };

        // duplicates-in-batch detection by (symbol,timeframe,end_ts)
        if !st.seen_end_ts.insert(end_ts) {
            st.stats.duplicates_in_batch += 1;
            totals.rows_rejected += 1;
            continue;
        }

        // out-of-order detection in batch order (within symbol,timeframe)
        if let Some(prev) = st.last_end_ts_in_batch_order {
            if end_ts < prev {
                st.stats.out_of_order += 1;
                totals.rows_rejected += 1;
                st.last_end_ts_in_batch_order = Some(end_ts);
                continue;
            }
        }
        st.last_end_ts_in_batch_order = Some(end_ts);

        // Parse prices
        let open_m = match price_to_micros(open_s) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let high_m = match price_to_micros(high_s) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let low_m = match price_to_micros(low_s) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let close_m = match price_to_micros(close_s) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };

        // Parse volume (>= 0)
        let volume: i64 = match volume_s.parse::<i64>() {
            Ok(v) if v >= 0 => v,
            _ => {
                st.stats.negative_or_invalid_volume += 1;
                totals.rows_rejected += 1;
                continue;
            }
        };

        let is_complete = match parse_bool(is_complete_s) {
            Some(v) => v,
            None => {
                totals.rows_rejected += 1;
                continue;
            }
        };

        if !is_complete {
            st.stats.incomplete_bars += 1;
        }

        if !ohlc_sane(open_m, high_m, low_m, close_m) {
            st.stats.ohlc_sanity_violations += 1;
            totals.rows_rejected += 1;
            continue;
        }

        // Upsert into md_bars (canonical-only).
        let (inserted,): (bool,) = sqlx::query_as(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts,
              open_micros, high_micros, low_micros, close_micros,
              volume, is_complete
            )
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (symbol,timeframe,end_ts) do update
              set open_micros  = excluded.open_micros,
                  high_micros  = excluded.high_micros,
                  low_micros   = excluded.low_micros,
                  close_micros = excluded.close_micros,
                  volume       = excluded.volume,
                  is_complete  = excluded.is_complete,
                  ingested_at  = now()
            returning (xmax = 0) as inserted
            "#,
        )
        .bind(symbol)
        .bind(&args.timeframe)
        .bind(end_ts)
        .bind(open_m)
        .bind(high_m)
        .bind(low_m)
        .bind(close_m)
        .bind(volume)
        .bind(is_complete)
        .fetch_one(pool)
        .await
        .context("md_bars upsert failed")?;

        totals.rows_ok += 1;
        if inserted {
            totals.rows_inserted += 1;
        } else {
            totals.rows_updated += 1;
        }

        st.ok_end_ts.push(end_ts);
    }

    // Compute gap detection for 1D (weekday-only; holidays TODO).
    let tf_is_1d = args.timeframe.eq_ignore_ascii_case("1D");
    if tf_is_1d {
        for ((_sym, _tf), st) in groups.iter_mut() {
            let mut end_ts_unique: Vec<i64> = st.ok_end_ts.clone();
            end_ts_unique.sort_unstable();
            end_ts_unique.dedup();
            st.stats.gaps_detected = weekday_only_gaps_1d(&end_ts_unique);
        }
    }

    // Build report
    let mut per = BTreeMap::new();
    for ((sym, tf), st) in groups.into_iter() {
        let k = format!("{sym}|{tf}");
        per.insert(k, st.stats);
    }

    let report = MdQualityReport {
        ingest_id,
        created_at_utc: created_at.to_rfc3339(),
        source: args.source.clone(),
        coverage: totals.clone(),
        per_symbol_timeframe: per,
        notes: vec![
            "Gap detection for timeframe=1D ignores weekends only; US market holidays are TODO."
                .to_string(),
        ],
    };

    // Persist report JSON to md_quality_reports keyed by ingest_id (idempotent).
    let stats_json = serde_json::to_value(&report).context("serialize report failed")?;
    sqlx::query(
        r#"
        insert into md_quality_reports (ingest_id, stats_json)
        values ($1, $2)
        on conflict (ingest_id) do update
          set stats_json = excluded.stats_json
        "#,
    )
    .bind(ingest_id)
    .bind(stats_json)
    .execute(pool)
    .await
    .context("insert md_quality_reports failed")?;

    Ok(IngestResult { ingest_id, report })
}

// -------------------------------
// PATCH C — Provider ingestion
// -------------------------------

#[derive(Debug, Clone)]
pub struct ProviderBar {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: i64,
    pub is_complete: bool,
}

#[derive(Debug, Clone)]
pub struct IngestProviderBarsArgs {
    pub source: String,
    pub timeframe: String,
    pub ingest_id: Option<Uuid>,
    pub bars: Vec<ProviderBar>,
}

/// Ingest provider bars (already fetched) into canonical md_bars and persist a quality report.
/// No networking here (tests stay offline).
pub async fn ingest_provider_bars_to_md_bars(
    pool: &PgPool,
    args: IngestProviderBarsArgs,
) -> Result<IngestResult> {
    let ingest_id = args.ingest_id.unwrap_or_else(Uuid::new_v4);
    let created_at = Utc::now();

    let mut totals = CoverageTotals {
        rows_read: 0,
        rows_ok: 0,
        rows_rejected: 0,
        rows_inserted: 0,
        rows_updated: 0,
    };

    let mut groups: HashMap<(String, String), GroupState> = HashMap::new();

    for b in args.bars.into_iter() {
        totals.rows_read += 1;

        if !b.timeframe.eq_ignore_ascii_case(&args.timeframe) {
            totals.rows_rejected += 1;
            continue;
        }

        let key = (b.symbol.clone(), args.timeframe.clone());
        let st = groups.entry(key.clone()).or_insert_with(GroupState::new);

        let end_ts = b.end_ts;

        if !st.seen_end_ts.insert(end_ts) {
            st.stats.duplicates_in_batch += 1;
            totals.rows_rejected += 1;
            continue;
        }

        if let Some(prev) = st.last_end_ts_in_batch_order {
            if end_ts < prev {
                st.stats.out_of_order += 1;
                totals.rows_rejected += 1;
                st.last_end_ts_in_batch_order = Some(end_ts);
                continue;
            }
        }
        st.last_end_ts_in_batch_order = Some(end_ts);

        let open_m = match price_to_micros(&b.open) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let high_m = match price_to_micros(&b.high) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let low_m = match price_to_micros(&b.low) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };
        let close_m = match price_to_micros(&b.close) {
            Ok(v) => v,
            Err(_) => {
                totals.rows_rejected += 1;
                continue;
            }
        };

        if b.volume < 0 {
            st.stats.negative_or_invalid_volume += 1;
            totals.rows_rejected += 1;
            continue;
        }

        if !b.is_complete {
            st.stats.incomplete_bars += 1;
        }

        if !ohlc_sane(open_m, high_m, low_m, close_m) {
            st.stats.ohlc_sanity_violations += 1;
            totals.rows_rejected += 1;
            continue;
        }

        let (inserted,): (bool,) = sqlx::query_as(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts,
              open_micros, high_micros, low_micros, close_micros,
              volume, is_complete
            )
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (symbol,timeframe,end_ts) do update
              set open_micros  = excluded.open_micros,
                  high_micros  = excluded.high_micros,
                  low_micros   = excluded.low_micros,
                  close_micros = excluded.close_micros,
                  volume       = excluded.volume,
                  is_complete  = excluded.is_complete,
                  ingested_at  = now()
            returning (xmax = 0) as inserted
            "#,
        )
        .bind(&b.symbol)
        .bind(&args.timeframe)
        .bind(end_ts)
        .bind(open_m)
        .bind(high_m)
        .bind(low_m)
        .bind(close_m)
        .bind(b.volume)
        .bind(b.is_complete)
        .fetch_one(pool)
        .await
        .context("md_bars upsert failed")?;

        totals.rows_ok += 1;
        if inserted {
            totals.rows_inserted += 1;
        } else {
            totals.rows_updated += 1;
        }

        st.ok_end_ts.push(end_ts);
    }

    let tf_is_1d = args.timeframe.eq_ignore_ascii_case("1D");
    if tf_is_1d {
        for ((_sym, _tf), st) in groups.iter_mut() {
            let mut end_ts_unique: Vec<i64> = st.ok_end_ts.clone();
            end_ts_unique.sort_unstable();
            end_ts_unique.dedup();
            st.stats.gaps_detected = weekday_only_gaps_1d(&end_ts_unique);
        }
    }

    let mut per = BTreeMap::new();
    for ((sym, tf), st) in groups.into_iter() {
        let k = format!("{sym}|{tf}");
        per.insert(k, st.stats);
    }

    let report = MdQualityReport {
        ingest_id,
        created_at_utc: created_at.to_rfc3339(),
        source: args.source.clone(),
        coverage: totals.clone(),
        per_symbol_timeframe: per,
        notes: vec![
            "Gap detection for timeframe=1D ignores weekends only; US market holidays are TODO."
                .to_string(),
        ],
    };

    let stats_json = serde_json::to_value(&report).context("serialize report failed")?;
    sqlx::query(
        r#"
        insert into md_quality_reports (ingest_id, stats_json)
        values ($1, $2)
        on conflict (ingest_id) do update
          set stats_json = excluded.stats_json
        "#,
    )
    .bind(ingest_id)
    .bind(stats_json)
    .execute(pool)
    .await
    .context("insert md_quality_reports failed")?;

    Ok(IngestResult { ingest_id, report })
}

#[derive(Debug, Clone)]
struct HeaderMap {
    idx: HashMap<String, usize>,
}

impl HeaderMap {
    fn from_headers(headers: &csv::StringRecord) -> Result<Self> {
        let mut idx = HashMap::new();
        for (i, h) in headers.iter().enumerate() {
            idx.insert(h.trim().to_ascii_lowercase(), i);
        }

        for req in [
            "symbol",
            "timeframe",
            "end_ts",
            "open",
            "high",
            "low",
            "close",
            "volume",
            "is_complete",
        ] {
            if !idx.contains_key(req) {
                return Err(anyhow!("csv missing required header: {req}"));
            }
        }

        Ok(Self { idx })
    }

    fn get<'a>(&self, rec: &'a csv::StringRecord, name: &str) -> Result<&'a str> {
        let i = *self
            .idx
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| anyhow!("missing header mapping: {name}"))?;
        rec.get(i).ok_or_else(|| anyhow!("missing field '{name}'"))
    }
}

/// Parse boolean values accepted by spec: true/false/1/0/yes/no (case-insensitive).
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Convert a decimal price string to micros (i64) deterministically (no floats).
///
/// Rules:
/// - Only ASCII digits and optional single '.' are allowed (leading '+' ok; '-' rejected).
/// - Up to 6 fractional digits supported; fewer are right-padded with zeros.
/// - More than 6 fractional digits is rejected (deterministic; avoids rounding ambiguity).
pub fn price_to_micros(s: &str) -> Result<i64> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err(anyhow!("empty price"));
    }
    if raw.starts_with('-') {
        return Err(anyhow!("negative price"));
    }

    let raw = raw.strip_prefix('+').unwrap_or(raw);

    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() > 2 {
        return Err(anyhow!("invalid decimal"));
    }

    let whole = parts[0];
    if whole.is_empty() {
        return Err(anyhow!("invalid whole part"));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("invalid whole digits"));
    }

    let whole_i: i64 = whole.parse().context("whole parse")?;
    let whole_m = whole_i
        .checked_mul(1_000_000)
        .ok_or_else(|| anyhow!("price overflow"))?;

    let frac_m = if parts.len() == 2 {
        let frac = parts[1];
        if frac.is_empty() {
            0i64
        } else {
            if !frac.chars().all(|c| c.is_ascii_digit()) {
                return Err(anyhow!("invalid fractional digits"));
            }
            if frac.len() > 6 {
                return Err(anyhow!("too many fractional digits"));
            }
            let mut frac_padded = frac.to_string();
            while frac_padded.len() < 6 {
                frac_padded.push('0');
            }
            let frac_i: i64 = frac_padded.parse().context("frac parse")?;
            frac_i
        }
    } else {
        0i64
    };

    whole_m
        .checked_add(frac_m)
        .ok_or_else(|| anyhow!("price overflow"))
}

/// OHLC sanity check per spec.
pub fn ohlc_sane(open_m: i64, high_m: i64, low_m: i64, close_m: i64) -> bool {
    if low_m > high_m {
        return false;
    }
    let max_oc = open_m.max(close_m);
    let min_oc = open_m.min(close_m);
    high_m >= max_oc && low_m <= min_oc
}

/// Count missing weekday-only days between consecutive dates for timeframe=1D.
/// Weekends are ignored. Holidays are explicitly TODO in notes.
fn weekday_only_gaps_1d(sorted_end_ts: &[i64]) -> u64 {
    if sorted_end_ts.len() < 2 {
        return 0;
    }

    let mut gaps: u64 = 0;
    let mut prev_date: Option<NaiveDate> = None;

    for &ts in sorted_end_ts {
        let Some(dt) = DateTime::<Utc>::from_timestamp(ts, 0) else {
            continue;
        };
        let d = dt.date_naive();

        if let Some(prev) = prev_date {
            let mut cur = prev.succ_opt();
            while let Some(day) = cur {
                if day >= d {
                    break;
                }
                if is_weekday(day.weekday()) {
                    gaps += 1;
                }
                cur = day.succ_opt();
            }
        }

        prev_date = Some(d);
    }

    gaps
}

fn is_weekday(w: Weekday) -> bool {
    !matches!(w, Weekday::Sat | Weekday::Sun)
}

#[cfg(test)]
mod tests {
    use super::*;

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
