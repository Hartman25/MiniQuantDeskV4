# APPLY_BACKTEST_PATCHSET.ps1
# Applies backtest patchset (BKT1–BKT6) in one shot.
# Assumes repo root contains `core-rs\`.
# Safe-ish: overwrites specific files; appends small additions where feasible.

$ErrorActionPreference = "Stop"

function Ensure-Dir($Path) {
  if (!(Test-Path $Path)) { New-Item -ItemType Directory -Path $Path | Out-Null }
}

function Write-File($Path, $Content) {
  Ensure-Dir (Split-Path -Parent $Path)
  Set-Content -Path $Path -Value $Content -Encoding UTF8
  Write-Host "WROTE: $Path"
}

function Append-IfMissing($Path, $Needle, $AppendContent) {
  if (!(Test-Path $Path)) { throw "Missing file: $Path" }
  $txt = Get-Content -Raw -Path $Path -Encoding UTF8
  if ($txt -notmatch [regex]::Escape($Needle)) {
    Add-Content -Path $Path -Value "`r`n$AppendContent`r`n" -Encoding UTF8
    Write-Host "APPENDED: $Path"
  } else {
    Write-Host "SKIP APPEND (already present): $Path"
  }
}

function Replace-Text($Path, $Old, $New) {
  if (!(Test-Path $Path)) { throw "Missing file: $Path" }
  $txt = Get-Content -Raw -Path $Path -Encoding UTF8
  if ($txt -notmatch [regex]::Escape($Old)) {
    throw "Replace failed (pattern not found) in $Path"
  }
  $txt = $txt.Replace($Old, $New)
  Set-Content -Path $Path -Value $txt -Encoding UTF8
  Write-Host "PATCHED: $Path"
}

function Ensure-SqlxRowImport($Path) {
  if (!(Test-Path $Path)) { throw "Missing file: $Path" }
  $txt = Get-Content -Raw -Path $Path -Encoding UTF8

  # If Row is already imported, do nothing.
  if ($txt -match "use\s+sqlx\s*::\s*\{[^}]*\bRow\b[^}]*\}\s*;" -or $txt -match "use\s+sqlx\s*::\s*Row\s*;") {
    Write-Host "SKIP sqlx Row import (already present): $Path"
    return
  }

  # Case 1: use sqlx::PgPool;
  if ($txt -match "use\s+sqlx\s*::\s*PgPool\s*;") {
    $txt = [regex]::Replace($txt, "use\s+sqlx\s*::\s*PgPool\s*;", "use sqlx::{PgPool, Row};", 1)
    Set-Content -Path $Path -Value $txt -Encoding UTF8
    Write-Host "PATCHED sqlx import (PgPool -> {PgPool, Row}): $Path"
    return
  }

  # Case 2: use sqlx::{PgPool, ...};
  if ($txt -match "use\s+sqlx\s*::\s*\{[^}]*\bPgPool\b[^}]*\}\s*;") {
    # Insert Row into the existing brace list.
    $txt = [regex]::Replace(
      $txt,
      "use\s+sqlx\s*::\s*\{([^}]*)\}\s*;",
      {
        param($m)
        $inner = $m.Groups[1].Value
        if ($inner -match "\bRow\b") { return $m.Value }
        # Keep deterministic formatting: append ", Row" at end (with trimming).
        $inner2 = $inner.Trim()
        if ($inner2.EndsWith(",")) {
          return "use sqlx::{${inner2} Row};"
        } elseif ($inner2.Length -eq 0) {
          return "use sqlx::{Row};"
        } else {
          return "use sqlx::{${inner2}, Row};"
        }
      },
      1
    )
    Set-Content -Path $Path -Value $txt -Encoding UTF8
    Write-Host "PATCHED sqlx import (added Row): $Path"
    return
  }

  # Fallback: add a new import line near the top (after existing use lines if possible).
  # This is safe and minimal; rustfmt will clean it up.
  $lines = $txt -split "(`r`n|`n)"
  $insertAt = 0
  for ($i=0; $i -lt $lines.Length; $i++) {
    if ($lines[$i] -match "^\s*use\s+") { $insertAt = $i + 1 }
  }
  $newLines = @()
  for ($i=0; $i -lt $lines.Length; $i++) {
    $newLines += $lines[$i]
    if ($i -eq $insertAt) {
      $newLines += "use sqlx::Row;"
    }
  }
  $out = ($newLines -join "`r`n")
  Set-Content -Path $Path -Value $out -Encoding UTF8
  Write-Host "PATCHED sqlx import (fallback inserted use sqlx::Row;): $Path"
}

# ------------------------------------------------------------
# Paths
# ------------------------------------------------------------
$backtestLib   = "core-rs\crates\mqk-backtest\src\lib.rs"
$backtestLoader= "core-rs\crates\mqk-backtest\src\loader.rs"
$btTest        = "core-rs\crates\mqk-backtest\tests\determinism_smoke.rs"
$btFixture     = "core-rs\crates\mqk-backtest\tests\fixtures\bkt4_bars.csv"

$cliCargo      = "core-rs\crates\mqk-cli\Cargo.toml"
$cliMain       = "core-rs\crates\mqk-cli\src\main.rs"
$cliCmdMod     = "core-rs\crates\mqk-cli\src\commands\mod.rs"
$cliCmdBkt     = "core-rs\crates\mqk-cli\src\commands\bkt.rs"

$artCargo      = "core-rs\crates\mqk-artifacts\Cargo.toml"
$artLib        = "core-rs\crates\mqk-artifacts\src\lib.rs"

$dbLib         = "core-rs\crates\mqk-db\src\lib.rs"
$dbMd          = "core-rs\crates\mqk-db\src\md.rs"

# ------------------------------------------------------------
# BKT1: mqk-backtest loader + exports
# ------------------------------------------------------------
$loaderRs = @"
 //! Deterministic bar loaders for mqk-backtest.
 //!
 //! This module intentionally keeps core parsing pure and deterministic.
 //! Any IO is explicit (e.g., `load_csv_file`).
 //!
 //! ## CSV format (integers; prices are in micros)
 //!
 //! Required columns:
 //! - `symbol`
 //! - `end_ts` (epoch seconds, bar end)
 //! - `open_micros`, `high_micros`, `low_micros`, `close_micros`
 //! - `volume`
 //!
 //! Optional columns:
 //! - `is_complete` (0/1 or false/true; default: true)
 //! - `day_id` (YYYYMMDD; default: computed from `end_ts`)
 //! - `reject_window_id` (u32; default: `end_ts / 60`)
 //!
 //! Output ordering is deterministic: sorted by `(end_ts ASC, symbol ASC)`.

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

    let header_line = lines.next().ok_or(LoadError::EmptyInput)?.trim();
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
            fields.get(col).map(|s| s.as_str()).ok_or_else(|| LoadError::BadRow {
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
    out.sort_by(|a, b| a.end_ts.cmp(&b.end_ts).then_with(|| a.symbol.cmp(&b.symbol)));
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
"@
Write-File $backtestLoader $loaderRs

# Update mqk-backtest/src/lib.rs (small file; overwrite safely)
$btLib = @"
 //! mqk-backtest
 //!
 //! PATCH 11 – Backtest Engine (Event-Sourced Replay)
 //!
 //! Pipeline: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
 //!
 //! - Deterministic replay (same bars + config => identical results)
 //! - No lookahead (incomplete bars rejected)
 //! - Conservative fill pricing (worst-case ambiguity: BUY@HIGH, SELL@LOW)
 //! - Stress profiles (slippage basis points)
 //! - Shadow mode support (strategy runs but trades not executed)
 //! - Risk enforcement via mqk-risk (daily loss, drawdown, PDT, reject storm)
 //! - FIFO portfolio accounting via mqk-portfolio

pub mod corporate_actions; // Patch B4
pub mod loader;
mod engine;
pub mod types;

pub use corporate_actions::{CorporateActionPolicy, ForbidEntry}; // Patch B4
pub use engine::{BacktestEngine, BacktestError};
pub use loader::{load_csv_file, parse_csv_bars, LoadError};
pub use types::{BacktestBar, BacktestConfig, BacktestReport, StressProfile};
"@
Write-File $backtestLib $btLib

# ------------------------------------------------------------
# BKT4: determinism integration test + fixture
# ------------------------------------------------------------
$fixture = @"
symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete
TEST,60,995000,1010000,990000,1000000,1000,1
TEST,120,1000000,1030000,1010000,1020000,1100,1
TEST,180,1020000,1020000,1000000,1015000,900,1
"@
Write-File $btFixture $fixture

$detTest = @"
use mqk_backtest::{BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const BARS_CSV: &str = include_str!(\"fixtures/bkt4_bars.csv\");

struct BuyHoldExit {
    spec: StrategySpec,
}

impl BuyHoldExit {
    fn new(timeframe_secs: i64) -> Self {
        Self {
            spec: StrategySpec::new(\"bkt4_buy_hold_exit\", timeframe_secs),
        }
    }
}

impl Strategy for BuyHoldExit {
    fn spec(&self) -> StrategySpec {
        self.spec.clone()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let target_qty = if ctx.now_tick == 0 { 10 } else if ctx.now_tick >= 2 { 0 } else { 10 };
        StrategyOutput::new(vec![TargetPosition {
            symbol: \"TEST\".to_string(),
            target_qty,
        }])
    }
}

#[test]
fn determinism_equity_curve_and_fills_are_stable() {
    let bars = mqk_backtest::parse_csv_bars(BARS_CSV).expect(\"parse fixture csv\");

    let mut cfg = BacktestConfig::test_defaults();
    cfg.timeframe_secs = 60;
    cfg.initial_cash_micros = 100_000_000_000;
    cfg.shadow_mode = false;
    cfg.integrity_enabled = false;
    cfg.stress.slippage_bps = 0;
    cfg.stress.volatility_mult_bps = 0;

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyHoldExit::new(60))).unwrap();

    let report = engine.run(&bars).expect(\"run backtest\");

    assert_eq!(report.fills.len(), 2);

    assert_eq!(report.fills[0].symbol, \"TEST\");
    assert_eq!(format!(\"{:?}\", report.fills[0].side), \"Buy\");
    assert_eq!(report.fills[0].qty, 10);
    assert_eq!(report.fills[0].price_micros, 1_010_000);
    assert_eq!(report.fills[0].fee_micros, 0);

    assert_eq!(report.fills[1].symbol, \"TEST\");
    assert_eq!(format!(\"{:?}\", report.fills[1].side), \"Sell\");
    assert_eq!(report.fills[1].qty, 10);
    assert_eq!(report.fills[1].price_micros, 1_000_000);
    assert_eq!(report.fills[1].fee_micros, 0);

    let expected = vec![
        (60, 99_999_900_000),
        (120, 100_000_100_000),
        (180, 99_999_900_000),
    ];
    assert_eq!(report.equity_curve, expected);

    assert_eq!(report.last_prices.get(\"TEST\").copied(), Some(1_015_000));
}
"@
Write-File $btTest $detTest

# ------------------------------------------------------------
# BKT3: mqk-artifacts writer for BacktestReport
# ------------------------------------------------------------
# Add dependency to mqk-artifacts/Cargo.toml if missing
Append-IfMissing $artCargo "mqk-backtest" "mqk-backtest = { path = `"../mqk-backtest`" }"

# Append writer function to mqk-artifacts/src/lib.rs if missing
$writerNeedle = "pub fn write_backtest_report"
$writerAppend = @"
use std::collections::BTreeMap;

/// Write deterministic backtest artifacts into an existing run directory.
///
/// Files written (overwritten):
/// - fills.csv
/// - equity_curve.csv
/// - metrics.json
///
/// No wall-clock time is used; `ts_utc` values are derived from the equity curve.
pub fn write_backtest_report(run_dir: &Path, report: &mqk_backtest::BacktestReport) -> Result<()> {
    fs::create_dir_all(run_dir)
        .with_context(|| format!(\"create backtest artifacts dir failed: {}\", run_dir.display()))?;

    let default_ts = report.equity_curve.first().map(|(ts, _)| *ts).unwrap_or(0);

    // fills.csv
    let mut fills_csv = String::from(\"ts_utc,fill_id,order_id,symbol,side,qty,price,fee\\n\");
    for f in &report.fills {
        let side = format!(\"{:?}\", f.side).to_uppercase();
        fills_csv.push_str(&format!(
            \"{},{},{},{},{},{},{},{}\\n\",
            default_ts, \"\", \"\", f.symbol, side, f.qty, f.price_micros, f.fee_micros
        ));
    }
    let fills_path = run_dir.join(\"fills.csv\");
    fs::write(&fills_path, fills_csv)
        .with_context(|| format!(\"write fills.csv failed: {}\", fills_path.display()))?;

    // equity_curve.csv
    let mut eq_csv = String::from(\"ts_utc,equity\\n\");
    for (ts, eq) in &report.equity_curve {
        eq_csv.push_str(&format!(\"{},{}\\n\", ts, eq));
    }
    let eq_path = run_dir.join(\"equity_curve.csv\");
    fs::write(&eq_path, eq_csv)
        .with_context(|| format!(\"write equity_curve.csv failed: {}\", eq_path.display()))?;

    // metrics.json
    let final_equity = report.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(0);

    let mut symbols: Vec<&str> = report.last_prices.keys().map(|s| s.as_str()).collect();
    symbols.sort();

    let last_prices_micros: BTreeMap<&str, i64> = report
        .last_prices
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();

    #[derive(Serialize)]
    struct BacktestMetrics<'a> {
        schema_version: i32,
        halted: bool,
        halt_reason: Option<&'a str>,
        execution_blocked: bool,
        bars: usize,
        fills: usize,
        final_equity_micros: i64,
        symbols: Vec<&'a str>,
        last_prices_micros: BTreeMap<&'a str, i64>,
    }

    let metrics = BacktestMetrics {
        schema_version: 1,
        halted: report.halted,
        halt_reason: report.halt_reason.as_deref(),
        execution_blocked: report.execution_blocked,
        bars: report.equity_curve.len(),
        fills: report.fills.len(),
        final_equity_micros: final_equity,
        symbols,
        last_prices_micros,
    };

    let metrics_path = run_dir.join(\"metrics.json\");
    let json = serde_json::to_string_pretty(&metrics).context(\"serialize metrics failed\")?;
    fs::write(&metrics_path, format!(\"{}\\n\", json))
        .with_context(|| format!(\"write metrics.json failed: {}\", metrics_path.display()))?;

    Ok(())
}
"@
Append-IfMissing $artLib $writerNeedle $writerAppend

# ------------------------------------------------------------
# BKT5+BKT6: mqk-db loader and CLI wiring (CSV + DB)
# ------------------------------------------------------------

# Patch mqk-db/src/md.rs import
Ensure-SqlxRowImport $dbMd

# Insert MdBarRow + loader after "use uuid::Uuid;"
$mdNeedle = "use uuid::Uuid;"
$mdInsert = @"
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Backtest loader (DB -> canonical rows)
// ---------------------------------------------------------------------------

/// Canonical md_bars row used by deterministic backtest loaders.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Load canonical bars from the `md_bars` table for use in backtesting.
///
/// Determinism guarantees:
/// - Rows are returned in stable order: `(end_ts ASC, symbol ASC)`.
/// - No implicit time sources are used.
///
/// `symbols` empty => loads all symbols for the timeframe/time-range.
///
/// Uses sqlx `query()` + binds (no macros).
pub async fn load_md_bars_for_backtest(
    pool: &PgPool,
    timeframe: &str,
    start_end_ts_inclusive: i64,
    end_end_ts_inclusive: i64,
    symbols: &[String],
) -> Result<Vec<MdBarRow>> {
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
        out.push(MdBarRow {
            symbol: r.try_get::<String, _>("symbol").context("md_bars.symbol")?,
            timeframe: r.try_get::<String, _>("timeframe").context("md_bars.timeframe")?,
            end_ts: r.try_get::<i64, _>("end_ts").context("md_bars.end_ts")?,
            open_micros: r.try_get::<i64, _>("open_micros").context("md_bars.open_micros")?,
            high_micros: r.try_get::<i64, _>("high_micros").context("md_bars.high_micros")?,
            low_micros: r.try_get::<i64, _>("low_micros").context("md_bars.low_micros")?,
            close_micros: r.try_get::<i64, _>("close_micros").context("md_bars.close_micros")?,
            volume: r.try_get::<i64, _>("volume").context("md_bars.volume")?,
            is_complete: r.try_get::<bool, _>("is_complete").context("md_bars.is_complete")?,
        });
    }
    Ok(out)
}
"@

# Only insert once
$mdTxt = Get-Content -Raw -Path $dbMd -Encoding UTF8
if ($mdTxt -notmatch "pub struct MdBarRow") {
  Replace-Text $dbMd $mdNeedle $mdInsert
} else {
  Write-Host "SKIP INSERT (MdBarRow already present): $dbMd"
}

# Patch mqk-db/src/lib.rs re-exports (best-effort)
if (Test-Path $dbLib) {
  $libTxt = Get-Content -Raw -Path $dbLib -Encoding UTF8
  if ($libTxt -notmatch "load_md_bars_for_backtest") {
    # crude insert into pub use md::{ ... }
    $libTxt = $libTxt -replace "pub use md::\{", "pub use md::{ load_md_bars_for_backtest, "
    if ($libTxt -notmatch "MdBarRow") {
      $libTxt = $libTxt -replace "MdQualityReport", "MdBarRow, MdQualityReport"
    }
    Set-Content -Path $dbLib -Value $libTxt -Encoding UTF8
    Write-Host "PATCHED: $dbLib"
  } else {
    Write-Host "SKIP PATCH (already present): $dbLib"
  }
}

# ------------------------------------------------------------
# mqk-cli deps and new backtest command (CSV + DB)
# ------------------------------------------------------------

# Ensure mqk-cli has needed internal deps
Append-IfMissing $cliCargo "mqk-backtest" "mqk-backtest = { path = `"../mqk-backtest`" }"
Append-IfMissing $cliCargo "mqk-execution" "mqk-execution = { path = `"../mqk-execution`" }"
Append-IfMissing $cliCargo "mqk-strategy" "mqk-strategy = { path = `"../mqk-strategy`" }"

# commands/mod.rs: add bkt module
$cmdModTxt = Get-Content -Raw -Path $cliCmdMod -Encoding UTF8
if ($cmdModTxt -notmatch "pub mod bkt;") {
  $cmdModTxt = $cmdModTxt -replace "pub mod backtest;\s*\r?\n", "pub mod backtest;`r`npub mod bkt;`r`n"
  Set-Content -Path $cliCmdMod -Value $cmdModTxt -Encoding UTF8
  Write-Host "PATCHED: $cliCmdMod"
} else {
  Write-Host "SKIP PATCH: $cliCmdMod"
}

# Write commands/bkt.rs containing BOTH CSV + DB runners
$cliBktRs = @"
use anyhow::{Context, Result};
use std::path::Path;

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

pub async fn run_backtest_csv(
    bars_path: String,
    timeframe_secs: i64,
    initial_cash_micros: i64,
    shadow: bool,
    integrity_enabled: bool,
    integrity_stale_threshold_ticks: u64,
    integrity_gap_tolerance_bars: u32,
    out_dir: Option<String>,
) -> Result<()> {
    let bars = mqk_backtest::load_csv_file(&bars_path)
        .with_context(|| format!("load bars csv failed: {}", bars_path))?;

    if timeframe_secs <= 0 {
        anyhow::bail!("--timeframe-secs must be > 0");
    }
    if initial_cash_micros <= 0 {
        anyhow::bail!("--initial-cash-micros must be > 0");
    }

    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = timeframe_secs;
    cfg.initial_cash_micros = initial_cash_micros;
    cfg.shadow_mode = shadow;

    cfg.integrity_enabled = integrity_enabled;
    cfg.integrity_stale_threshold_ticks = integrity_stale_threshold_ticks;
    cfg.integrity_gap_tolerance_bars = integrity_gap_tolerance_bars;

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(NoOpStrategy::new(timeframe_secs)))?;

    let report = engine.run(&bars).context("backtest run failed")?;

    if let Some(dir) = out_dir.as_deref() {
        mqk_artifacts::write_backtest_report(Path::new(dir), &report)
            .with_context(|| format!("write backtest artifacts failed: {}", dir))?;
        println!("artifacts_written=true out_dir={}", dir);
    } else {
        println!("artifacts_written=false");
    }

    let final_equity = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash_micros);

    println!("backtest_ok=true");
    println!("source=csv");
    println!("bars_loaded={}", bars.len());
    println!("fills={}", report.fills.len());
    println!("execution_blocked={}", report.execution_blocked);
    println!("halted={}", report.halted);
    if let Some(r) = report.halt_reason {
        println!("halt_reason={}", r);
    }
    println!("final_equity_micros={}", final_equity);

    Ok(())
}

pub async fn run_backtest_db(
    timeframe: String,
    start_end_ts: i64,
    end_end_ts: i64,
    symbols_csv: Option<String>,
    timeframe_secs: i64,
    initial_cash_micros: i64,
    shadow: bool,
    integrity_enabled: bool,
) -> Result<()> {
    if timeframe_secs <= 0 {
        anyhow::bail!("--timeframe-secs must be > 0");
    }
    if initial_cash_micros <= 0 {
        anyhow::bail!("--initial-cash-micros must be > 0");
    }
    if end_end_ts < start_end_ts {
        anyhow::bail!("--end-end-ts must be >= --start-end-ts");
    }

    let symbols: Vec<String> = symbols_csv
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let pool = mqk_db::connect_from_env().await?;

    let rows = mqk_db::md::load_md_bars_for_backtest(
        &pool,
        &timeframe,
        start_end_ts,
        end_end_ts,
        &symbols,
    )
    .await
    .context("load_md_bars_for_backtest failed")?;

    let mut bars: Vec<BacktestBar> = Vec::with_capacity(rows.len());
    for r in rows {
        let day_id = epoch_secs_to_yyyymmdd(r.end_ts);
        let reject_window_id = r.end_ts.div_euclid(60).try_into().unwrap_or(u32::MAX);
        bars.push(BacktestBar {
            symbol: r.symbol,
            end_ts: r.end_ts,
            open_micros: r.open_micros,
            high_micros: r.high_micros,
            low_micros: r.low_micros,
            close_micros: r.close_micros,
            volume: r.volume,
            is_complete: r.is_complete,
            day_id,
            reject_window_id,
        });
    }

    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = timeframe_secs;
    cfg.initial_cash_micros = initial_cash_micros;
    cfg.shadow_mode = shadow;
    cfg.integrity_enabled = integrity_enabled;

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(NoOpStrategy::new(timeframe_secs)))?;

    let report = engine.run(&bars).context("backtest run failed")?;

    let final_equity = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash_micros);

    println!("backtest_ok=true");
    println!("source=db");
    println!("timeframe={}", timeframe);
    println!("bars_loaded={}", bars.len());
    println!("fills={}", report.fills.len());
    println!("execution_blocked={}", report.execution_blocked);
    println!("halted={}", report.halted);
    if let Some(r) = report.halt_reason {
        println!("halt_reason={}", r);
    }
    println!("final_equity_micros={}", final_equity);

    Ok(())
}

struct NoOpStrategy {
    spec: StrategySpec,
}

impl NoOpStrategy {
    fn new(timeframe_secs: i64) -> Self {
        Self {
            spec: StrategySpec::new("noop", timeframe_secs),
        }
    }
}

impl Strategy for NoOpStrategy {
    fn spec(&self) -> StrategySpec {
        self.spec.clone()
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

#[allow(dead_code)]
struct BuyThenExitStrategy {
    spec: StrategySpec,
    qty: i64,
    exit_tick: u64,
}

#[allow(dead_code)]
impl BuyThenExitStrategy {
    fn new(timeframe_secs: i64, qty: i64, exit_tick: u64) -> Self {
        Self {
            spec: StrategySpec::new("buy_then_exit", timeframe_secs),
            qty,
            exit_tick,
        }
    }
}

#[allow(dead_code)]
impl Strategy for BuyThenExitStrategy {
    fn spec(&self) -> StrategySpec {
        self.spec.clone()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let target_qty = if ctx.now_tick == 0 {
            self.qty
        } else if ctx.now_tick >= self.exit_tick {
            0
        } else {
            self.qty
        };
        StrategyOutput::new(vec![TargetPosition {
            symbol: "TEST".to_string(),
            target_qty,
        }])
    }
}

fn epoch_secs_to_yyyymmdd(epoch_secs: i64) -> u32 {
    let days = epoch_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    (y * 10_000 + m * 100 + d).try_into().unwrap_or(19700101)
}

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
"@
Write-File $cliCmdBkt $cliBktRs

# Overwrite mqk-cli/src/main.rs with an updated version based on your current file + Backtest command
# (Safe because it keeps existing commands and only adds new ones.)
$mainRs = @"
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

use commands::{
    backtest::{md_ingest_csv, md_ingest_provider},
    bkt::{run_backtest_csv, run_backtest_db},
    load_payload,
    run::{
        run_arm, run_begin, run_deadman_check, run_deadman_enforce, run_halt, run_heartbeat,
        run_loop, run_start, run_status, run_stop,
    },
};

// ---------------------------------------------------------------------------
// Clap CLI structure
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "mqk")]
#[command(about = "MiniQuantDesk V4 CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Database commands
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },

    /// Compute layered config hash + print canonical JSON
    ConfigHash {
        /// Paths in merge order (base -> env -> engine -> risk -> stress...)
        #[arg(required = true)]
        paths: Vec<String>,
    },

    /// Run lifecycle commands
    Run {
        #[command(subcommand)]
        cmd: RunCmd,
    },

    /// Audit trail utilities
    Audit {
        #[command(subcommand)]
        cmd: AuditCmd,
    },

    /// Market data utilities (canonical md_bars)
    Md {
        #[command(subcommand)]
        cmd: MdCmd,
    },

    /// Deterministic backtest tools.
    Backtest {
        #[command(subcommand)]
        cmd: BacktestCmd,
    },
}

#[derive(Subcommand)]
enum BacktestCmd {
    /// Run an end-to-end deterministic backtest from a CSV bars file.
    Csv {
        /// Path to bars CSV file (see mqk-backtest loader docs).
        #[arg(long)]
        bars: String,

        /// Timeframe seconds (must match strategy spec).
        #[arg(long, default_value_t = 60)]
        timeframe_secs: i64,

        /// Initial cash in micros.
        #[arg(long, default_value_t = 100_000_000_000)]
        initial_cash_micros: i64,

        /// Shadow mode: run strategy but do not execute trades.
        #[arg(long, default_value_t = false)]
        shadow: bool,

        /// Enable integrity checks.
        #[arg(long, default_value_t = true)]
        integrity_enabled: bool,

        /// Integrity stale threshold (ticks).
        #[arg(long, default_value_t = 120)]
        integrity_stale_threshold_ticks: u64,

        /// Integrity gap tolerance (missing bars).
        #[arg(long, default_value_t = 0)]
        integrity_gap_tolerance_bars: u32,

        /// Optional output directory for deterministic artifacts (fills/equity/metrics).
        #[arg(long)]
        out_dir: Option<String>,
    },

    /// Load canonical bars from Postgres md_bars and run a deterministic backtest.
    Db {
        /// Timeframe string as stored in md_bars (e.g. 1m, 1h, 1D).
        #[arg(long)]
        timeframe: String,

        /// Inclusive start end_ts (epoch seconds).
        #[arg(long)]
        start_end_ts: i64,

        /// Inclusive end end_ts (epoch seconds).
        #[arg(long)]
        end_end_ts: i64,

        /// Optional comma-separated symbol list. If omitted, loads all symbols.
        #[arg(long)]
        symbols: Option<String>,

        /// Strategy timeframe in seconds.
        #[arg(long, default_value_t = 60)]
        timeframe_secs: i64,

        /// Initial cash in micros.
        #[arg(long, default_value_t = 100_000_000_000)]
        initial_cash_micros: i64,

        /// Shadow mode: run strategy but do not execute trades.
        #[arg(long, default_value_t = false)]
        shadow: bool,

        /// Enable integrity checks.
        #[arg(long, default_value_t = true)]
        integrity_enabled: bool,
    },
}

#[derive(Subcommand)]
enum MdCmd {
    /// PATCH B: Ingest canonical bars from a CSV file into md_bars and write a Data Quality Gate v1 report.
    IngestCsv {
        /// Path to CSV file
        #[arg(long)]
        path: String,

        /// Timeframe (e.g. 1D)
        #[arg(long)]
        timeframe: String,

        /// Source label for report (default: csv)
        #[arg(long, default_value = "csv")]
        source: String,
    },

    /// PATCH C: Ingest historical bars from a provider into canonical md_bars.
    IngestProvider {
        /// Provider source name (only: twelvedata)
        #[arg(long)]
        source: String,

        /// Comma-separated symbols
        #[arg(long)]
        symbols: String,

        /// Timeframe (1D | 1m | 5m)
        #[arg(long)]
        timeframe: String,

        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: String,

        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: String,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    Status,

    /// Apply SQL migrations. Guardrail: refuses when any LIVE run is ARMED/RUNNING unless --yes is provided.
    Migrate {
        /// Acknowledge you are migrating a DB that may be used for LIVE trading.
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum RunCmd {
    /// Create a new run row in DB and print run_id + hashes.
    Start {
        /// Engine ID (e.g. MAIN, EXP)
        #[arg(long)]
        engine: String,

        /// Mode (BACKTEST | PAPER | LIVE)
        #[arg(long)]
        mode: String,

        /// Layered config paths in merge order
        #[arg(long = "config", required = true)]
        config_paths: Vec<String>,
    },

    /// Arm an existing run (CREATED/STOPPED -> ARMED)
    Arm {
        /// Run id
        #[arg(long)]
        run_id: String,

        /// Manual confirmation string (required for LIVE when configured)
        #[arg(long)]
        confirm: Option<String>,
    },

    /// Begin an armed run (ARMED -> RUNNING)
    Begin {
        /// Run id
        #[arg(long)]
        run_id: String,
    },

    /// Stop an armed/running run (ARMED/RUNNING -> STOPPED)
    Stop {
        /// Run id
        #[arg(long)]
        run_id: String,
    },

    /// Halt a run (ANY -> HALTED)
    Halt {
        /// Run id
        #[arg(long)]
        run_id: String,

        /// Human reason (printed; not stored in DB in Phase 1)
        #[arg(long)]
        reason: String,
    },

    /// Emit a heartbeat for a running run (RUNNING only)
    Heartbeat {
        /// Run id
        #[arg(long)]
        run_id: String,
    },

    /// Print run status row
    Status {
        /// Run id
        #[arg(long)]
        run_id: String,
    },

    /// Check if deadman is expired for a RUNNING run
    DeadmanCheck {
        #[arg(long)]
        run_id: String,

        /// Heartbeat TTL in seconds
        #[arg(long)]
        ttl_seconds: i64,
    },

    /// Enforce deadman: halt the run if expired
    DeadmanEnforce {
        #[arg(long)]
        run_id: String,

        /// Heartbeat TTL in seconds
        #[arg(long)]
        ttl_seconds: i64,
    },

    /// Execute a deterministic orchestrator loop (testkit) with synthetic bars.
    Loop {
        #[arg(long)]
        run_id: String,

        #[arg(long)]
        symbol: String,

        /// How many bars to generate and feed to the orchestrator.
        #[arg(long, default_value_t = 50)]
        bars: usize,

        /// Timeframe seconds for each bar.
        #[arg(long, default_value_t = 60)]
        timeframe_secs: u64,

        /// (Kept for CLI compatibility; orchestrator currently does not write exports)
        #[arg(long, default_value = "artifacts/exports")]
        exports_root: PathBuf,

        /// (Kept for CLI compatibility; orchestrator meta currently does not store label)
        #[arg(long, default_value = "cli_loop")]
        label: String,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Emit an audit event to JSONL (exports/<run_id>/audit.jsonl) AND to DB.
    Emit {
        /// Run id to attach this event to
        #[arg(long)]
        run_id: String,

        /// Topic (e.g. runtime, data, broker, risk, exec)
        #[arg(long)]
        topic: String,

        /// Event type (e.g. START, BAR, SIGNAL, ORDER_SUBMIT, FILL, KILL_SWITCH)
        #[arg(long = "type")]
        event_type: String,

        /// Payload JSON string (avoid if possible; PowerShell quoting is annoying)
        #[arg(long, conflicts_with = "payload_file")]
        payload: Option<String>,

        /// Path to a payload JSON file (recommended on Windows)
        #[arg(long = "payload-file", conflicts_with = "payload")]
        payload_file: Option<String>,

        /// Enable hash chain (flag presence => true)
        #[arg(long, default_value_t = true, action = clap::ArgAction::SetTrue)]
        hash_chain: bool,

        /// Disable hash chain explicitly
        #[arg(long = "no-hash-chain", action = clap::ArgAction::SetFalse)]
        #[arg(default_value_t = true)]
        _hash_chain_off: bool,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_filename(".env.local");

    let cli = Cli::parse();

    match cli.cmd {
        Commands::Db { cmd } => {
            let pool = mqk_db::connect_from_env().await?;
            match cmd {
                DbCmd::Status => {
                    let s = mqk_db::status(&pool).await?;
                    println!("db_ok={} has_runs_table={}", s.ok, s.has_runs_table);
                }
                DbCmd::Migrate { yes } => {
                    let n = mqk_db::count_active_live_runs(&pool).await?;
                    if n > 0 && !yes {
                        anyhow::bail!(
                            "REFUSING MIGRATE: detected {} active LIVE run(s) in ARMED/RUNNING. Re-run with: `mqk db migrate --yes`",
                            n
                        );
                    }
                    mqk_db::migrate(&pool).await?;
                    println!("migrations_applied=true");
                }
            }
        }

        Commands::ConfigHash { paths } => {
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            let loaded = mqk_config::load_layered_yaml(&path_refs)?;
            println!("config_hash={}", loaded.config_hash);
            println!("{}", loaded.canonical_json);
        }

        Commands::Md { cmd } => match cmd {
            MdCmd::IngestCsv { path, timeframe, source } => {
                md_ingest_csv(path, timeframe, source).await?;
            }
            MdCmd::IngestProvider { source, symbols, timeframe, start, end } => {
                md_ingest_provider(source, symbols, timeframe, start, end).await?;
            }
        },

        Commands::Backtest { cmd } => match cmd {
            BacktestCmd::Csv {
                bars,
                timeframe_secs,
                initial_cash_micros,
                shadow,
                integrity_enabled,
                integrity_stale_threshold_ticks,
                integrity_gap_tolerance_bars,
                out_dir,
            } => {
                run_backtest_csv(
                    bars,
                    timeframe_secs,
                    initial_cash_micros,
                    shadow,
                    integrity_enabled,
                    integrity_stale_threshold_ticks,
                    integrity_gap_tolerance_bars,
                    out_dir,
                )
                .await?;
            }
            BacktestCmd::Db {
                timeframe,
                start_end_ts,
                end_end_ts,
                symbols,
                timeframe_secs,
                initial_cash_micros,
                shadow,
                integrity_enabled,
            } => {
                run_backtest_db(
                    timeframe,
                    start_end_ts,
                    end_end_ts,
                    symbols,
                    timeframe_secs,
                    initial_cash_micros,
                    shadow,
                    integrity_enabled,
                )
                .await?;
            }
        },

        Commands::Run { cmd } => match cmd {
            RunCmd::Start { engine, mode, config_paths } => {
                run_start(engine, mode, config_paths).await?;
            }
            RunCmd::Arm { run_id, confirm } => {
                run_arm(run_id, confirm).await?;
            }
            RunCmd::Begin { run_id } => {
                run_begin(run_id).await?;
            }
            RunCmd::Stop { run_id } => {
                run_stop(run_id).await?;
            }
            RunCmd::Halt { run_id, reason } => {
                run_halt(run_id, reason).await?;
            }
            RunCmd::Heartbeat { run_id } => {
                run_heartbeat(run_id).await?;
            }
            RunCmd::Status { run_id } => {
                run_status(run_id).await?;
            }
            RunCmd::DeadmanCheck { run_id, ttl_seconds } => {
                run_deadman_check(run_id, ttl_seconds).await?;
            }
            RunCmd::DeadmanEnforce { run_id, ttl_seconds } => {
                run_deadman_enforce(run_id, ttl_seconds).await?;
            }
            RunCmd::Loop { run_id, symbol, bars, timeframe_secs, exports_root, label } => {
                run_loop(run_id, symbol, bars, timeframe_secs, exports_root, label)?;
            }
        },

        Commands::Audit { cmd } => match cmd {
            AuditCmd::Emit {
                run_id,
                topic,
                event_type,
                payload,
                payload_file,
                hash_chain,
                ..
            } => {
                use anyhow::Context;
                use uuid::Uuid;

                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                let payload_json = load_payload(payload, payload_file)?;

                let path = format!("../exports/{}/audit.jsonl", run_id);
                let mut writer = mqk_audit::AuditWriter::new(&path, hash_chain)?;
                let ev = writer.append(run_uuid, &topic, &event_type, payload_json)?;

                let db_ev = mqk_db::NewAuditEvent {
                    event_id: ev.event_id,
                    run_id: ev.run_id,
                    ts_utc: ev.ts_utc,
                    topic: ev.topic,
                    event_type: ev.event_type,
                    payload: ev.payload,
                    hash_prev: ev.hash_prev,
                    hash_self: ev.hash_self,
                };
                mqk_db::insert_audit_event(&pool, &db_ev).await?;

                println!("audit_written=true path={}", path);
                println!("event_id={}", db_ev.event_id);
                if let Some(h) = db_ev.hash_self {
                    println!("hash_self={}", h);
                }
            }
        },
    }

    Ok(())
}
"@
Write-File $cliMain $mainRs

Write-Host "`nDONE applying backtest patchset."
Write-Host "Next: run the test command block from the instructions."