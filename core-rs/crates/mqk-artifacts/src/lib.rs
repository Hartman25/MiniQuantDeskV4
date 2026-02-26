use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String,
    pub git_hash: String,
    pub config_hash: String,
    pub host_fingerprint: String,
    pub created_at_utc: DateTime<Utc>,
    pub artifacts: ArtifactList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactList {
    pub audit_jsonl: String,
    pub manifest_json: String,
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
}

pub struct InitRunArtifactsArgs<'a> {
    pub exports_root: &'a Path, // e.g. ../exports
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: &'a str,
    pub mode: &'a str,
    pub git_hash: &'a str,
    pub config_hash: &'a str,
    pub host_fingerprint: &'a str,
}

pub struct InitRunArtifactsResult {
    pub run_dir: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn init_run_artifacts(args: InitRunArtifactsArgs<'_>) -> Result<InitRunArtifactsResult> {
    // exports/<run_id>/
    let run_dir = args.exports_root.join(args.run_id.to_string());
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create exports dir failed: {}", run_dir.display()))?;

    // Create placeholder files if missing (do not overwrite existing).
    ensure_file_exists_with(&run_dir.join("audit.jsonl"), "")?;
    ensure_file_exists_with(
        &run_dir.join("orders.csv"),
        "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n",
    )?;
    ensure_file_exists_with(
        &run_dir.join("fills.csv"),
        "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n",
    )?;
    ensure_file_exists_with(&run_dir.join("equity_curve.csv"), "ts_utc,equity\n")?;
    ensure_file_exists_with(&run_dir.join("metrics.json"), "{}\n")?;

    // Write manifest.json (overwrite is OK; it’s deterministic for a run start).
    let manifest = RunManifest {
        schema_version: args.schema_version,
        run_id: args.run_id,
        engine_id: args.engine_id.to_string(),
        mode: args.mode.to_string(),
        git_hash: args.git_hash.to_string(),
        config_hash: args.config_hash.to_string(),
        host_fingerprint: args.host_fingerprint.to_string(),
        created_at_utc: Utc::now(),
        artifacts: ArtifactList {
            audit_jsonl: "audit.jsonl".to_string(),
            manifest_json: "manifest.json".to_string(),
            orders_csv: "orders.csv".to_string(),
            fills_csv: "fills.csv".to_string(),
            equity_curve_csv: "equity_curve.csv".to_string(),
            metrics_json: "metrics.json".to_string(),
        },
    };

    let manifest_path = run_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest).context("serialize manifest failed")?;
    fs::write(&manifest_path, format!("{json}\n"))
        .with_context(|| format!("write manifest failed: {}", manifest_path.display()))?;

    Ok(InitRunArtifactsResult {
        run_dir,
        manifest_path,
    })
}

fn ensure_file_exists_with(path: &Path, contents_if_create: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents_if_create)
        .with_context(|| format!("create placeholder failed: {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Backtest report writer (deterministic outputs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct BacktestMetrics<'a> {
    schema_version: i32,
    halted: bool,
    halt_reason: Option<&'a str>,
    execution_blocked: bool,
    bars: usize,
    fills: usize,
    final_equity_micros: i64,
    symbols: Vec<&'a str>,
    last_prices_micros: std::collections::BTreeMap<&'a str, i64>,
}

/// Write deterministic backtest artifacts into an existing run directory.
///
/// This function performs explicit IO. It is intended to be called by CLI/daemons.
/// No wall-clock time is used; timestamps are derived from `report.equity_curve` / bar end_ts.
///
/// Files written (overwritten):
/// - `fills.csv`
/// - `equity_curve.csv`
/// - `metrics.json`
pub fn write_backtest_report(run_dir: &Path, report: &mqk_backtest::BacktestReport) -> Result<()> {
    fs::create_dir_all(run_dir).with_context(|| {
        format!(
            "create backtest artifacts dir failed: {}",
            run_dir.display()
        )
    })?;

    // fills.csv (match placeholder header used by init_run_artifacts)
    // NOTE: Fill currently has no IDs or timestamps in core structs, so we emit blank IDs.
    // `ts_utc` is emitted as the first equity_curve timestamp when available; otherwise 0.
    let default_ts = report.equity_curve.first().map(|(ts, _)| *ts).unwrap_or(0);
    let mut fills_csv = String::from("ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n");
    for f in &report.fills {
        let side = format!("{:?}", f.side).to_uppercase(); // BUY / SELL deterministically
        fills_csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            default_ts, "", "", f.symbol, side, f.qty, f.price_micros, f.fee_micros
        ));
    }
    let fills_path = run_dir.join("fills.csv");
    fs::write(&fills_path, fills_csv)
        .with_context(|| format!("write fills.csv failed: {}", fills_path.display()))?;

    // equity_curve.csv (match placeholder header)
    let mut eq_csv = String::from("ts_utc,equity\n");
    for (ts, eq) in &report.equity_curve {
        eq_csv.push_str(&format!("{},{}\n", ts, eq));
    }
    let eq_path = run_dir.join("equity_curve.csv");
    fs::write(&eq_path, eq_csv)
        .with_context(|| format!("write equity_curve.csv failed: {}", eq_path.display()))?;

    // metrics.json
    let final_equity = report.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(0);

    // deterministic symbol listing
    let mut symbols: Vec<&str> = report.last_prices.keys().map(|s| s.as_str()).collect();
    symbols.sort();

    let last_prices_micros = report
        .last_prices
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect::<std::collections::BTreeMap<_, _>>();

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

    let metrics_path = run_dir.join("metrics.json");
    let json = serde_json::to_string_pretty(&metrics).context("serialize metrics failed")?;
    fs::write(&metrics_path, format!("{json}\n"))
        .with_context(|| format!("write metrics.json failed: {}", metrics_path.display()))?;

    Ok(())
}
