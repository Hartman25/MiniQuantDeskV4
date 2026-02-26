//! Backtest CSV loader (deterministic).
//!
//! CSV format
//!
//! Required columns:
//! - `symbol`
//! - `end_ts`
//! - `open_micros`
//! - `high_micros`
//! - `low_micros`
//! - `close_micros`
//! - `volume`
//!
//! Optional columns:
//! - `is_complete` (bool; default: true)
//! - `day_id` (u32; default: derived from `end_ts` as YYYYMMDD in UTC)
//! - `reject_window_id` (u32; default: `end_ts / 60`)

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::types::BacktestBar;

/// Loader errors are small, explicit, and test-friendly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    EmptyInput,
    MissingHeader(&'static str),
    ParseInt { column: String, value: String },
    ParseBool { column: String, value: String },
    BadRow { line: usize, reason: String },
    Io(String),
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e.to_string())
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::EmptyInput => write!(f, "empty input"),
            LoadError::MissingHeader(h) => write!(f, "missing header: {}", h),
            LoadError::ParseInt { column, value } => {
                write!(f, "failed to parse int in column {}: {}", column, value)
            }
            LoadError::ParseBool { column, value } => {
                write!(f, "failed to parse bool in column {}: {}", column, value)
            }
            LoadError::BadRow { line, reason } => write!(f, "bad row at line {}: {}", line, reason),
            LoadError::Io(e) => write!(f, "io error: {}", e),
        }
    }
}

impl std::error::Error for LoadError {}

/// Load bars from a CSV file on disk.
///
/// IO is explicit; parsing and sorting are deterministic.
pub fn load_csv_file(path: impl AsRef<Path>) -> Result<Vec<BacktestBar>, LoadError> {
    let s = fs::read_to_string(path)?;
    parse_csv_bars(&s)
}

/// Parse bars from CSV content (pure, deterministic).
pub fn parse_csv_bars(csv: &str) -> Result<Vec<BacktestBar>, LoadError> {
    let mut lines = csv.lines();

    let header_line = lines.next().ok_or(LoadError::EmptyInput)?;
    // Normalize header: trim whitespace and strip UTF-8 BOM if present.
    let header_line = header_line.trim().trim_start_matches('\u{feff}');
    if header_line.is_empty() {
        return Err(LoadError::EmptyInput);
    }

    let headers: Vec<String> = split_csv_line(header_line)
        .into_iter()
        .map(|s| s.trim().to_string())
        .collect();

    // Build header index map (case-sensitive, deterministic).
    let mut idx: BTreeMap<String, usize> = BTreeMap::new();
    for (i, h) in headers.iter().enumerate() {
        idx.insert(h.clone(), i);
    }

    // Required columns.
    let col_symbol = find_required(&idx, "symbol")?;
    let col_end_ts = find_required(&idx, "end_ts")?;
    let col_open = find_required(&idx, "open_micros")?;
    let col_high = find_required(&idx, "high_micros")?;
    let col_low = find_required(&idx, "low_micros")?;
    let col_close = find_required(&idx, "close_micros")?;
    let col_volume = find_required(&idx, "volume")?;

    // Optional columns.
    let col_is_complete = idx.get("is_complete").copied();
    let col_day_id = idx.get("day_id").copied();
    let col_reject_window_id = idx.get("reject_window_id").copied();

    let mut out: Vec<BacktestBar> = Vec::new();

    for (line_idx0, raw) in lines.enumerate() {
        let line_no = line_idx0 + 2; // 1-based, counting header as line 1

        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }

        let fields = split_csv_line(raw);
        let get = |col: usize| -> Result<&str, LoadError> {
            fields
                .get(col)
                .map(|s| s.as_str())
                .ok_or_else(|| LoadError::BadRow {
                    line: line_no,
                    reason: format!("missing column index {col}"),
                })
        };

        let symbol = get(col_symbol)?.trim().to_string();
        if symbol.is_empty() {
            return Err(LoadError::BadRow {
                line: line_no,
                reason: "symbol is empty".to_string(),
            });
        }

        let end_ts = parse_i64(get(col_end_ts)?, "end_ts")?;
        let open_micros = parse_i64(get(col_open)?, "open_micros")?;
        let high_micros = parse_i64(get(col_high)?, "high_micros")?;
        let low_micros = parse_i64(get(col_low)?, "low_micros")?;
        let close_micros = parse_i64(get(col_close)?, "close_micros")?;
        let volume = parse_i64(get(col_volume)?, "volume")?;

        let is_complete = match col_is_complete {
            Some(c) => parse_bool(get(c)?, "is_complete")?,
            None => true,
        };

        let day_id = match col_day_id {
            Some(c) => parse_u32(get(c)?, "day_id")?,
            None => epoch_secs_to_yyyymmdd(end_ts),
        };

        let reject_window_id = match col_reject_window_id {
            Some(c) => parse_u32(get(c)?, "reject_window_id")?,
            None => end_ts.div_euclid(60).try_into().unwrap_or(u32::MAX),
        };

        out.push(BacktestBar {
            symbol,
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete,
            day_id,
            reject_window_id,
        });
    }

    // Deterministic ordering: (end_ts ASC, symbol ASC)
    out.sort_by(|a, b| {
        a.end_ts
            .cmp(&b.end_ts)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    Ok(out)
}

fn find_required(idx: &BTreeMap<String, usize>, name: &'static str) -> Result<usize, LoadError> {
    idx.get(name).copied().ok_or(LoadError::MissingHeader(name))
}

fn parse_i64(s: &str, col: &str) -> Result<i64, LoadError> {
    let t = s.trim();
    t.parse::<i64>().map_err(|_| LoadError::ParseInt {
        column: col.to_string(),
        value: t.to_string(),
    })
}

fn parse_u32(s: &str, col: &str) -> Result<u32, LoadError> {
    let t = s.trim();
    t.parse::<u32>().map_err(|_| LoadError::ParseInt {
        column: col.to_string(),
        value: t.to_string(),
    })
}

fn parse_bool(s: &str, col: &str) -> Result<bool, LoadError> {
    let t = s.trim();
    match t {
        "1" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(LoadError::ParseBool {
            column: col.to_string(),
            value: t.to_string(),
        }),
    }
}

/// Minimal CSV splitting (no quoting support).
fn split_csv_line(line: &str) -> Vec<String> {
    line.split(',').map(|s| s.trim().to_string()).collect()
}

/// Deterministically convert epoch seconds to YYYYMMDD (UTC).
fn epoch_secs_to_yyyymmdd(epoch_secs: i64) -> u32 {
    let days = epoch_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    (y * 10_000 + m * 100 + d).try_into().unwrap_or(19700101)
}

/// civil_from_days (public domain; Howard Hinnant)
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let y = (yoe as i32) + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let d = (doy - (153 * mp + 2).div_euclid(5) + 1) as u32;
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_sorts_deterministically() {
        let csv = r#"symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume
B,60,10,12,9,11,100
A,60,20,22,19,21,200
A,0,1,1,1,1,1
"#;

        let bars = parse_csv_bars(csv).expect("parse");
        assert_eq!(bars.len(), 3);

        // Sorted by end_ts ASC, then symbol ASC
        assert_eq!(bars[0].symbol, "A");
        assert_eq!(bars[0].end_ts, 0);
        assert_eq!(bars[1].symbol, "A");
        assert_eq!(bars[1].end_ts, 60);
        assert_eq!(bars[2].symbol, "B");
        assert_eq!(bars[2].end_ts, 60);
    }

    #[test]
    fn epoch_to_day_id_is_stable() {
        assert_eq!(epoch_secs_to_yyyymmdd(0), 19700101);
        assert_eq!(epoch_secs_to_yyyymmdd(86_400), 19700102);
    }
}
