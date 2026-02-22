//! CSV ingestion helpers for market-data bars (mqk-md boundary).
//!
//! This module provides types and a parser that convert a CSV file (or in-memory
//! CSV text) into [`crate::provider::RawBar`] values.  It is the **read** side
//! only: it does **not** write to the database, run normalisation, or produce
//! quality reports.  Callers hand the resulting `Vec<RawBar>` to
//! `mqk_db::ingest_provider_bars_to_md_bars` (or equivalent) for persistence.
//!
//! ## CSV column contract (case-insensitive, order-independent)
//!
//! | Column        | Type / example      | Notes                              |
//! |---------------|---------------------|------------------------------------|
//! | `symbol`      | `AAPL`              |                                    |
//! | `timeframe`   | `1D`                | Must match caller-supplied filter  |
//! | `end_ts`      | `1708041600`        | UTC epoch seconds                  |
//! | `open`        | `182.34`            | Decimal string; no floats          |
//! | `high`        | `185.00`            | Decimal string                     |
//! | `low`         | `181.00`            | Decimal string                     |
//! | `close`       | `184.50`            | Decimal string                     |
//! | `volume`      | `1000000`           | Integer ≥ 0                        |
//! | `is_complete` | `true` / `1` / `yes` | See [`parse_is_complete`]         |

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

use crate::provider::RawBar;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced by CSV parsing in this module.
#[derive(Debug)]
pub enum CsvIngestError {
    /// An I/O or CSV-library error.
    Io(String),
    /// The header row is missing a required column.
    MissingHeader(String),
    /// A record field could not be parsed into the expected type.
    ParseField {
        row: usize,
        field: &'static str,
        raw: String,
    },
}

impl fmt::Display for CsvIngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CsvIngestError::Io(msg) => write!(f, "csv io error: {msg}"),
            CsvIngestError::MissingHeader(col) => {
                write!(f, "csv missing required header column: '{col}'")
            }
            CsvIngestError::ParseField { row, field, raw } => {
                write!(
                    f,
                    "csv row {row}: cannot parse field '{field}' from value '{raw}'"
                )
            }
        }
    }
}

impl std::error::Error for CsvIngestError {}

// ---------------------------------------------------------------------------
// Parsed row (pre-filter)
// ---------------------------------------------------------------------------

/// A single row as decoded from CSV, before any quality gate.
///
/// Prices remain as `String` so the caller (normaliser / DB layer) can apply
/// the canonical micro-conversion with no floating-point involvement.
#[derive(Debug, Clone)]
pub struct CsvRow {
    pub symbol: String,
    pub timeframe: String,
    /// UTC epoch seconds.
    pub end_ts: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    /// Volume must be ≥ 0; caller enforces the sign constraint.
    pub volume: i64,
    pub is_complete: bool,
}

impl From<CsvRow> for RawBar {
    fn from(r: CsvRow) -> Self {
        RawBar {
            symbol: r.symbol,
            timeframe: r.timeframe,
            end_ts: r.end_ts,
            open: r.open,
            high: r.high,
            low: r.low,
            close: r.close,
            volume: r.volume,
            is_complete: r.is_complete,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a CSV file at `path` and return all rows that match `timeframe_filter`
/// as [`RawBar`] values.
///
/// Rows with a non-matching timeframe, unparseable `end_ts`, or unparseable
/// `volume` are skipped (treated as rejected by the caller's quality gate).
/// Only structural / header errors are returned as `Err`.
///
/// Caller is responsible for OHLC sanity, duplicate detection, and DB
/// persistence (via `mqk_db::ingest_provider_bars_to_md_bars`).
pub fn parse_csv_file(
    path: &Path,
    timeframe_filter: &str,
) -> Result<Vec<RawBar>, CsvIngestError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| CsvIngestError::Io(format!("open '{}': {e}", path.display())))?;

    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|e| CsvIngestError::Io(format!("read '{}': {e}", path.display())))?;

    parse_csv_str(&buf, timeframe_filter)
}

/// Parse CSV from a string slice (useful for tests without touching the
/// filesystem).
///
/// See [`parse_csv_file`] for the full contract.
pub fn parse_csv_str(
    src: &str,
    timeframe_filter: &str,
) -> Result<Vec<RawBar>, CsvIngestError> {
    let mut lines = src.lines();

    // --- Header ---
    let header_line = match lines.next() {
        Some(l) => l,
        None => return Ok(Vec::new()),
    };

    let col_idx = build_col_index(header_line)?;

    // --- Data rows ---
    let mut out = Vec::new();
    let mut row_num: usize = 1; // 1-based, header = 0

    for line in lines {
        row_num += 1;

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Minimal CSV field split: comma-separated, no quoting (sufficient for OHLCV).
        let fields: Vec<&str> = line.splitn(col_idx.len() + 1, ',').collect();

        let get = |name: &'static str| -> Result<&str, CsvIngestError> {
            let i = *col_idx
                .get(&name.to_ascii_lowercase())
                .ok_or_else(|| CsvIngestError::MissingHeader(name.to_string()))?;
            fields
                .get(i)
                .copied()
                .map(str::trim)
                .ok_or_else(|| CsvIngestError::ParseField {
                    row: row_num,
                    field: name,
                    raw: String::new(),
                })
        };

        let timeframe = get("timeframe")?;
        if !timeframe.eq_ignore_ascii_case(timeframe_filter) {
            // Wrong timeframe — skip silently (counted as rejected by caller).
            continue;
        }

        let symbol = get("symbol")?.to_string();
        let timeframe = timeframe.to_string();

        let end_ts_s = get("end_ts")?;
        let end_ts: i64 = match end_ts_s.parse() {
            Ok(v) => v,
            Err(_) => {
                // Unparseable timestamp — skip row.
                continue;
            }
        };

        let open = get("open")?.to_string();
        let high = get("high")?.to_string();
        let low = get("low")?.to_string();
        let close = get("close")?.to_string();

        let volume_s = get("volume")?;
        let volume: i64 = match volume_s.parse() {
            Ok(v) => v,
            Err(_) => {
                // Unparseable volume — skip row.
                continue;
            }
        };

        let is_complete_s = get("is_complete")?;
        let is_complete = match parse_is_complete(is_complete_s) {
            Some(v) => v,
            None => {
                // Unparseable bool — skip row.
                continue;
            }
        };

        out.push(
            CsvRow {
                symbol,
                timeframe,
                end_ts,
                open,
                high,
                low,
                close,
                volume,
                is_complete,
            }
            .into(),
        );
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse boolean values per spec: `true/false/1/0/yes/no` (case-insensitive).
pub fn parse_is_complete(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Build a case-insensitive column-name → index map from a CSV header line.
fn build_col_index(header_line: &str) -> Result<HashMap<String, usize>, CsvIngestError> {
    let required = [
        "symbol",
        "timeframe",
        "end_ts",
        "open",
        "high",
        "low",
        "close",
        "volume",
        "is_complete",
    ];

    let mut idx: HashMap<String, usize> = HashMap::new();
    for (i, col) in header_line.split(',').enumerate() {
        idx.insert(col.trim().to_ascii_lowercase(), i);
    }

    for req in required {
        if !idx.contains_key(req) {
            return Err(CsvIngestError::MissingHeader(req.to_string()));
        }
    }

    Ok(idx)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "symbol,timeframe,end_ts,open,high,low,close,volume,is_complete";

    fn row(sym: &str, tf: &str, ts: i64, vol: i64, ok: bool) -> String {
        format!("{sym},{tf},{ts},10,12,9,11,{vol},{ok}")
    }

    // --- parse_is_complete ---

    #[test]
    fn parse_is_complete_variants() {
        for s in ["true", "True", "TRUE", "1", "yes", "YES"] {
            assert_eq!(parse_is_complete(s), Some(true), "failed for '{s}'");
        }
        for s in ["false", "False", "FALSE", "0", "no", "NO"] {
            assert_eq!(parse_is_complete(s), Some(false), "failed for '{s}'");
        }
        assert_eq!(parse_is_complete("maybe"), None);
        assert_eq!(parse_is_complete(""), None);
    }

    // --- parse_csv_str ---

    #[test]
    fn empty_input_returns_empty_vec() {
        let result = parse_csv_str("", "1D").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn header_only_returns_empty_vec() {
        let result = parse_csv_str(HEADER, "1D").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn missing_required_header_returns_err() {
        // strip 'volume' from header
        let bad_header = "symbol,timeframe,end_ts,open,high,low,close,is_complete";
        let err = parse_csv_str(bad_header, "1D").unwrap_err();
        assert!(matches!(err, CsvIngestError::MissingHeader(_)));
    }

    #[test]
    fn rows_with_matching_timeframe_included() {
        let csv = format!(
            "{HEADER}\n{}\n{}",
            row("AAPL", "1D", 1_000_000, 100, true),
            row("MSFT", "1D", 2_000_000, 200, true),
        );
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].symbol, "AAPL");
        assert_eq!(result[1].symbol, "MSFT");
    }

    #[test]
    fn rows_with_non_matching_timeframe_skipped() {
        let csv = format!(
            "{HEADER}\n{}\n{}",
            row("AAPL", "1D", 1_000_000, 100, true),
            row("MSFT", "1m", 2_000_000, 200, true),
        );
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "AAPL");
    }

    #[test]
    fn timeframe_filter_is_case_insensitive() {
        let csv = format!("{HEADER}\n{}", row("SPY", "1d", 1_000_000, 50, true));
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn unparseable_end_ts_skips_row() {
        let csv = format!("{HEADER}\nAAA,1D,NOT_A_TS,10,12,9,11,100,true");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn unparseable_volume_skips_row() {
        let csv = format!("{HEADER}\nAAA,1D,1000000,10,12,9,11,BAD_VOL,true");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn unparseable_is_complete_skips_row() {
        let csv = format!("{HEADER}\nAAA,1D,1000000,10,12,9,11,100,maybe");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn negative_volume_parsed_through_to_raw_bar() {
        // ingest_csv.rs does NOT enforce volume >= 0; that is the DB layer's job.
        let csv = format!("{HEADER}\nAAA,1D,1000000,10,12,9,11,-5,true");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].volume, -5);
    }

    #[test]
    fn decimal_price_strings_preserved_exactly() {
        let csv = format!("{HEADER}\nAAPL,1D,1708041600,182.34,185.00,181.00,184.50,1000000,true");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
        let bar = &result[0];
        assert_eq!(bar.open, "182.34");
        assert_eq!(bar.high, "185.00");
        assert_eq!(bar.low, "181.00");
        assert_eq!(bar.close, "184.50");
    }

    #[test]
    fn is_complete_false_parsed() {
        let csv = format!("{HEADER}\nAAPL,1D,1708041600,100,105,99,103,500,false");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_complete);
    }

    #[test]
    fn end_ts_and_volume_preserved() {
        let ts = 1_708_041_600_i64;
        let csv = format!("{HEADER}\nAAPL,1D,{ts},100,105,99,103,999999,true");
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result[0].end_ts, ts);
        assert_eq!(result[0].volume, 999_999);
    }

    #[test]
    fn blank_lines_skipped() {
        let csv = format!("{HEADER}\n\n{}\n\n", row("AAPL", "1D", 1_000_000, 100, true));
        let result = parse_csv_str(&csv, "1D").unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn raw_bar_from_csv_row_conversion() {
        let csv_row = CsvRow {
            symbol: "ZZZ".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 42,
            open: "1.00".to_string(),
            high: "2.00".to_string(),
            low: "0.50".to_string(),
            close: "1.50".to_string(),
            volume: 777,
            is_complete: true,
        };
        let bar: RawBar = csv_row.into();
        assert_eq!(bar.symbol, "ZZZ");
        assert_eq!(bar.end_ts, 42);
        assert_eq!(bar.volume, 777);
    }

    #[test]
    fn error_display_io() {
        let e = CsvIngestError::Io("file not found".to_string());
        assert!(e.to_string().contains("file not found"));
    }

    #[test]
    fn error_display_missing_header() {
        let e = CsvIngestError::MissingHeader("volume".to_string());
        assert!(e.to_string().contains("volume"));
    }

    #[test]
    fn error_display_parse_field() {
        let e = CsvIngestError::ParseField {
            row: 5,
            field: "end_ts",
            raw: "bad".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("row 5"));
        assert!(s.contains("end_ts"));
        assert!(s.contains("bad"));
    }
}
