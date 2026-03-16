use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::{engines::register_builtin_strategies, PluginRegistry};

// ---------------------------------------------------------------------------
// CSV backtest runner
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn run_backtest_csv(
    bars_path: String,
    strategy: String,
    symbol: String,
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

    // BKT-06P: resolve strategy from built-in plugin registry.
    let mut reg = PluginRegistry::new();
    register_builtin_strategies(&mut reg, &symbol)
        .with_context(|| format!("register_builtin_strategies failed for symbol={}", symbol))?;
    let strategy_instance = reg.instantiate(&strategy).with_context(|| {
        let available: Vec<_> = reg.list().iter().map(|m| m.name.as_str()).collect();
        format!(
            "unknown strategy '{}'; available: {}",
            strategy,
            available.join(", ")
        )
    })?;

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(strategy_instance)
        .with_context(|| format!("add_strategy failed for '{}'", strategy))?;

    let report = engine.run(&bars).context("backtest run failed")?;

    // BKT-05P: run identity — deterministic, from the report (NOT environmental).
    let config_hash = report.config_id.to_string();
    // BKT-06P: git_hash is operational artifact metadata — NOT part of run_id.
    let git_hash = bkt_git_hash();

    println!("run_id={}", report.run_id);
    println!("strategy={}", report.strategy_name);
    println!("git_hash={}", git_hash);
    println!("config_hash={}", config_hash);

    // BKT-02P: if an output directory is requested, initialize the full run
    // artifact structure (manifest.json + placeholder files) before writing
    // the backtest report into the run subdirectory.
    if let Some(dir) = out_dir.as_deref() {
        let host_fp = bkt_host_fingerprint();
        let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
            exports_root: Path::new(dir),
            schema_version: 1,
            run_id: report.run_id,
            strategy_name: &report.strategy_name,
            engine_id: "mqk-backtest",
            mode: "backtest",
            git_hash: &git_hash,
            config_hash: &config_hash,
            host_fingerprint: &host_fp,
            now_utc: Utc::now(), // allow: operational manifest timestamp
        })
        .with_context(|| format!("init run artifacts failed: {}", dir))?;

        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report).with_context(|| {
            format!(
                "write backtest artifacts failed: {}",
                init_result.run_dir.display()
            )
        })?;

        println!("artifacts_written=true");
        println!("artifacts_dir={}", init_result.run_dir.display());
        println!("manifest={}", init_result.manifest_path.display());
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
    println!("orders={}", report.orders.len());
    println!("fills={}", report.fills.len());
    println!("execution_blocked={}", report.execution_blocked);
    println!("halted={}", report.halted);
    if let Some(r) = report.halt_reason {
        println!("halt_reason={}", r);
    }
    println!("final_equity_micros={}", final_equity);

    Ok(())
}

// ---------------------------------------------------------------------------
// DB backtest runner
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn run_backtest_db(
    timeframe: String,
    start_end_ts: i64,
    end_end_ts: i64,
    symbols_csv: Option<String>,
    strategy: String,
    symbol: String,
    timeframe_secs: i64,
    initial_cash_micros: i64,
    shadow: bool,
    integrity_enabled: bool,
    out_dir: Option<String>,
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

    let rows = mqk_db::md::load_md_bars_for_backtest_symbols(
        &pool,
        &timeframe,
        start_end_ts,
        end_end_ts,
        &symbols,
    )
    .await
    .context("load_md_bars_for_backtest_symbols failed")?;

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

    // BKT-06P: resolve strategy from built-in plugin registry.
    let mut reg = PluginRegistry::new();
    register_builtin_strategies(&mut reg, &symbol)
        .with_context(|| format!("register_builtin_strategies failed for symbol={}", symbol))?;
    let strategy_instance = reg.instantiate(&strategy).with_context(|| {
        let available: Vec<_> = reg.list().iter().map(|m| m.name.as_str()).collect();
        format!(
            "unknown strategy '{}'; available: {}",
            strategy,
            available.join(", ")
        )
    })?;

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(strategy_instance)
        .with_context(|| format!("add_strategy failed for '{}'", strategy))?;

    let report = engine.run(&bars).context("backtest run failed")?;

    // BKT-05P: run identity — deterministic, from the report (NOT environmental).
    let config_hash = report.config_id.to_string();
    // BKT-06P: git_hash is operational artifact metadata — NOT part of run_id.
    let git_hash = bkt_git_hash();

    println!("run_id={}", report.run_id);
    println!("strategy={}", report.strategy_name);
    println!("git_hash={}", git_hash);
    println!("config_hash={}", config_hash);

    // BKT-02P: if an output directory is requested, initialize the full run
    // artifact structure (manifest.json + placeholder files) before writing
    // the backtest report into the run subdirectory.
    if let Some(dir) = out_dir.as_deref() {
        let host_fp = bkt_host_fingerprint();
        let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
            exports_root: Path::new(dir),
            schema_version: 1,
            run_id: report.run_id,
            strategy_name: &report.strategy_name,
            engine_id: "mqk-backtest",
            mode: "backtest",
            git_hash: &git_hash,
            config_hash: &config_hash,
            host_fingerprint: &host_fp,
            now_utc: Utc::now(), // allow: operational manifest timestamp
        })
        .with_context(|| format!("init run artifacts failed: {}", dir))?;

        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report).with_context(|| {
            format!(
                "write backtest artifacts failed: {}",
                init_result.run_dir.display()
            )
        })?;

        println!("artifacts_written=true");
        println!("artifacts_dir={}", init_result.run_dir.display());
        println!("manifest={}", init_result.manifest_path.display());
    } else {
        println!("artifacts_written=false");
    }

    let final_equity = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash_micros);

    println!("backtest_ok=true");
    println!("source=db");
    println!("timeframe={}", timeframe);
    println!("bars_loaded={}", bars.len());
    println!("orders={}", report.orders.len());
    println!("fills={}", report.fills.len());
    println!("execution_blocked={}", report.execution_blocked);
    println!("halted={}", report.halted);
    if let Some(r) = report.halt_reason {
        println!("halt_reason={}", r);
    }
    println!("final_equity_micros={}", final_equity);

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Best-effort short git hash of the running binary.
fn bkt_git_hash() -> String {
    use std::process::Command;
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

/// Best-effort host fingerprint for the artifact manifest.
fn bkt_host_fingerprint() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    format!(
        "{}@{}:{}/{}",
        username,
        hostname,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

// ---------------------------------------------------------------------------
// Date utilities (DB loader path)
// ---------------------------------------------------------------------------

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
