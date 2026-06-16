use anyhow::{Context, Result};
use chrono::Utc;
use clap::ValueEnum;
use std::path::Path;

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, StrategySizingConfig, SweepGrid, SweepRowResult,
    SWEEP_MAX_COMBINATIONS,
};
use mqk_integrity::CalendarSpec;
use mqk_strategy::{engines::register_builtin_strategies_with_sizing, PluginRegistry};

/// CLI-facing integrity calendar selector for CSV backtests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum IntegrityCalendarArg {
    /// Preserve existing 24/7 gap detection semantics.
    AlwaysOn,
    /// NYSE-style regular equity session calendar.
    UsEquityRegular,
}

impl From<IntegrityCalendarArg> for CalendarSpec {
    fn from(value: IntegrityCalendarArg) -> Self {
        match value {
            IntegrityCalendarArg::AlwaysOn => CalendarSpec::AlwaysOn,
            IntegrityCalendarArg::UsEquityRegular => CalendarSpec::NyseWeekdaysStartAnchored,
        }
    }
}

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
    integrity_calendar: IntegrityCalendarArg,
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_position_notional_usd: Option<i64>,
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
    if target_qty <= 0 {
        anyhow::bail!("--target-qty must be > 0");
    }

    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = timeframe_secs;
    cfg.initial_cash_micros = initial_cash_micros;
    cfg.shadow_mode = shadow;
    cfg.integrity_enabled = integrity_enabled;
    cfg.integrity_stale_threshold_ticks = integrity_stale_threshold_ticks;
    cfg.integrity_gap_tolerance_bars = integrity_gap_tolerance_bars;
    cfg.integrity_calendar = integrity_calendar.into();
    cfg.sizing = StrategySizingConfig {
        target_qty,
        max_target_qty,
        max_position_notional_usd,
    };

    // BACKTEST-CONFIG-DETERMINISM-SIZING-01: use sizing-aware registration so
    // the strategy is constructed from cfg.sizing, not ambient env vars.
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(
        &mut reg,
        &symbol,
        cfg.sizing.target_qty,
        cfg.sizing.max_target_qty,
        cfg.sizing.max_position_notional_usd,
    )
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
            timeframe: None,
            timeframe_secs: Some(timeframe_secs),
            git_hash: &git_hash,
            config_hash: &config_hash,
            host_fingerprint: &host_fp,
            now_utc: Utc::now(), // allow: operational manifest timestamp
        })
        .with_context(|| format!("init run artifacts failed: {}", dir))?;

        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash_micros)
            .with_context(|| {
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
// Strategy Lab artifact report
// ---------------------------------------------------------------------------

pub fn run_strategy_lab_evaluate(artifact_dir: String, json: bool) -> Result<()> {
    let evaluation = mqk_artifacts::evaluate_strategy_lab_artifact_dir(Path::new(&artifact_dir))
        .with_context(|| format!("strategy lab artifact evaluation failed: {}", artifact_dir))?;
    let reason_codes: Vec<&str> = evaluation.reason_codes.iter().map(|r| r.code()).collect();

    if json {
        let report = serde_json::json!({
            "strategy_id": evaluation.strategy_id,
            "symbol": evaluation.symbol,
            "timeframe": evaluation.timeframe,
            "score": evaluation.score,
            "grade": evaluation.grade.code(),
            "decision": evaluation.decision.code(),
            "reason_codes": reason_codes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .context("serialize strategy lab report failed")?
        );
    } else {
        println!("strategy_id={}", evaluation.strategy_id);
        println!("symbol={}", evaluation.symbol);
        println!("timeframe={}", evaluation.timeframe);
        println!("score={:.2}", evaluation.score);
        println!("grade={}", evaluation.grade.code());
        println!("decision={}", evaluation.decision.code());
        println!("reason_codes={}", reason_codes.join(","));
    }

    Ok(())
}

pub fn run_strategy_lab_rank(artifacts_root: String, top: Option<usize>, json: bool) -> Result<()> {
    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        Path::new(&artifacts_root),
        mqk_artifacts::StrategyLabArtifactRankOptions { top_n: top },
    )
    .with_context(|| format!("strategy lab artifact ranking failed: {}", artifacts_root))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .context("serialize strategy lab rank report failed")?
        );
    } else {
        println!("root_path={}", report.root_path.display());
        println!("candidates_scanned={}", report.candidates_scanned);
        println!("evaluations_count={}", report.evaluations_count);
        println!("failures_count={}", report.failures_count);
        for row in &report.ranked {
            println!(
                "rank={} artifact_path={} strategy_id={} symbol={} timeframe={} score={:.2} grade={} decision={} reason_codes={}",
                row.rank,
                row.artifact_path.display(),
                row.strategy_id,
                row.symbol,
                row.timeframe,
                row.score,
                row.grade,
                row.decision,
                row.reason_codes.join(",")
            );
        }
        for failure in &report.failures {
            println!(
                "failure_artifact_path={} error={}",
                failure.artifact_path.display(),
                failure.error
            );
        }
    }

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
    integrity_stale_threshold_ticks: u64,
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_position_notional_usd: Option<i64>,
    out_dir: Option<String>,
) -> Result<()> {
    if timeframe_secs <= 0 {
        anyhow::bail!("--timeframe-secs must be > 0");
    }
    if initial_cash_micros <= 0 {
        anyhow::bail!("--initial-cash-micros must be > 0");
    }
    if target_qty <= 0 {
        anyhow::bail!("--target-qty must be > 0");
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
    cfg.integrity_stale_threshold_ticks = integrity_stale_threshold_ticks;
    cfg.sizing = StrategySizingConfig {
        target_qty,
        max_target_qty,
        max_position_notional_usd,
    };

    // BACKTEST-CONFIG-DETERMINISM-SIZING-01: use sizing-aware registration.
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(
        &mut reg,
        &symbol,
        cfg.sizing.target_qty,
        cfg.sizing.max_target_qty,
        cfg.sizing.max_position_notional_usd,
    )
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
            timeframe: Some(&timeframe),
            timeframe_secs: Some(timeframe_secs),
            git_hash: &git_hash,
            config_hash: &config_hash,
            host_fingerprint: &host_fp,
            now_utc: Utc::now(), // allow: operational manifest timestamp
        })
        .with_context(|| format!("init run artifacts failed: {}", dir))?;

        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash_micros)
            .with_context(|| {
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
// CSV sweep runner
// ---------------------------------------------------------------------------

/// Parse a comma-separated list of i64 values (e.g. "1,3,5").
fn parse_i64_list(s: &str) -> Result<Vec<i64>> {
    s.split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.parse::<i64>()
                .with_context(|| format!("invalid integer: '{v}'"))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_sweep_csv(
    bars_path: String,
    strategy: String,
    symbol: String,
    timeframe_secs: i64,
    initial_cash_micros: i64,
    integrity_enabled: bool,
    integrity_stale_threshold_ticks: u64,
    integrity_gap_tolerance_bars: u32,
    target_qty_list: String,
    slippage_bps_list: String,
    volatility_mult_bps_list: String,
    out_dir: Option<String>,
    max_combinations_override: Option<usize>,
) -> Result<()> {
    if timeframe_secs <= 0 {
        anyhow::bail!("--timeframe-secs must be > 0");
    }
    if initial_cash_micros <= 0 {
        anyhow::bail!("--initial-cash-micros must be > 0");
    }

    let bars = mqk_backtest::load_csv_file(&bars_path)
        .with_context(|| format!("load bars csv failed: {}", bars_path))?;

    let target_qty_vals = parse_i64_list(&target_qty_list)?;
    let slippage_bps_vals = parse_i64_list(&slippage_bps_list)?;
    let vol_mult_vals = parse_i64_list(&volatility_mult_bps_list)?;

    if target_qty_vals.is_empty() {
        anyhow::bail!("--target-qty must contain at least one value");
    }
    if slippage_bps_vals.is_empty() {
        anyhow::bail!("--slippage-bps must contain at least one value");
    }

    let limit = max_combinations_override.unwrap_or(SWEEP_MAX_COMBINATIONS);

    let mut base_cfg = BacktestConfig::conservative_defaults();
    base_cfg.timeframe_secs = timeframe_secs;
    base_cfg.initial_cash_micros = initial_cash_micros;
    base_cfg.integrity_enabled = integrity_enabled;
    base_cfg.integrity_stale_threshold_ticks = integrity_stale_threshold_ticks;
    base_cfg.integrity_gap_tolerance_bars = integrity_gap_tolerance_bars;

    let grid = SweepGrid {
        target_qty: target_qty_vals,
        slippage_bps: slippage_bps_vals,
        volatility_mult_bps: vol_mult_vals,
        max_target_qty: vec![],
        max_position_notional_usd: vec![],
    };

    let combo_count = grid.combination_count(&base_cfg);
    println!("sweep_combinations={}", combo_count);

    if combo_count == 0 {
        anyhow::bail!("sweep grid is empty — at least one combination required");
    }
    if combo_count > limit {
        anyhow::bail!(
            "sweep grid has {} combinations which exceeds the limit of {} (use --max-combinations to override)",
            combo_count, limit
        );
    }

    let combos = grid.combinations(&base_cfg);
    let mut results: Vec<SweepRowResult> = Vec::with_capacity(combos.len());

    for (i, pt) in combos.iter().enumerate() {
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(
            &mut reg,
            &symbol,
            pt.target_qty,
            pt.max_target_qty,
            pt.max_position_notional_usd,
        )
        .with_context(|| format!("register_builtin_strategies failed for symbol={}", symbol))?;
        let strategy_instance = reg.instantiate(&strategy).with_context(|| {
            let available: Vec<_> = reg.list().iter().map(|m| m.name.as_str()).collect();
            format!(
                "unknown strategy '{}'; available: {}",
                strategy,
                available.join(", ")
            )
        })?;

        let mut cfg = base_cfg.clone();
        cfg.sizing = StrategySizingConfig {
            target_qty: pt.target_qty,
            max_target_qty: pt.max_target_qty,
            max_position_notional_usd: pt.max_position_notional_usd,
        };
        cfg.stress = mqk_backtest::StressProfile {
            slippage_bps: pt.slippage_bps,
            volatility_mult_bps: pt.volatility_mult_bps,
        };

        let mut engine = BacktestEngine::new(cfg);
        engine
            .add_strategy(strategy_instance)
            .with_context(|| format!("add_strategy failed for '{}'", strategy))?;

        let report = engine.run(&bars).context("backtest run failed")?;

        // Write individual run artifacts if out_dir provided.
        let artifact_path = if let Some(ref dir) = out_dir {
            let config_hash = report.config_id.to_string();
            let git_hash = bkt_git_hash();
            let host_fp = bkt_host_fingerprint();
            let init_result =
                mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
                    exports_root: Path::new(dir),
                    schema_version: 1,
                    run_id: report.run_id,
                    strategy_name: &report.strategy_name,
                    engine_id: "mqk-backtest",
                    mode: "backtest-sweep",
                    timeframe: None,
                    timeframe_secs: Some(timeframe_secs),
                    git_hash: &git_hash,
                    config_hash: &config_hash,
                    host_fingerprint: &host_fp,
                    now_utc: Utc::now(), // allow: operational manifest timestamp
                })
                .with_context(|| format!("init run artifacts failed for sweep point {}", i))?;

            mqk_artifacts::write_backtest_report(
                &init_result.run_dir,
                &report,
                initial_cash_micros,
            )
            .with_context(|| format!("write backtest artifacts failed for sweep point {}", i))?;

            Some(init_result.run_dir.display().to_string())
        } else {
            None
        };

        let row = mqk_backtest::sweep_row_from_report(&report, pt, artifact_path);
        results.push(row);
        println!(
            "sweep_run={}/{} tq={} slip={} vol={} return={:.2}% halted={}",
            i + 1,
            combos.len(),
            pt.target_qty,
            pt.slippage_bps,
            pt.volatility_mult_bps,
            results.last().unwrap().total_return_pct,
            results.last().unwrap().halted,
        );
    }

    mqk_backtest::rank_sweep_results(&mut results);

    // Write sweep summary artifacts.
    if let Some(ref dir) = out_dir {
        write_sweep_artifacts(Path::new(dir), &results)?;
        println!("sweep_artifacts_written=true");
        println!("sweep_dir={}", dir);
    }

    println!("sweep_ok=true");
    println!("sweep_total_runs={}", results.len());
    if let Some(best) = results.first() {
        println!(
            "sweep_best_run_id={} tq={} slip={} vol={} return={:.2}% alpha={:.2}% dd={:.2}%",
            best.run_id,
            best.target_qty,
            best.slippage_bps,
            best.volatility_mult_bps,
            best.total_return_pct,
            best.alpha_pct.unwrap_or(f64::NAN),
            best.max_drawdown_pct,
        );
    }

    Ok(())
}

/// Write sweep summary CSV, JSON, and Markdown to the sweep root directory.
fn write_sweep_artifacts(sweep_dir: &Path, rows: &[SweepRowResult]) -> Result<()> {
    use std::fmt::Write as FmtWrite;

    std::fs::create_dir_all(sweep_dir)
        .with_context(|| format!("create sweep dir failed: {}", sweep_dir.display()))?;

    // --- sweep_summary.csv ---
    let csv_path = sweep_dir.join("sweep_summary.csv");
    let mut csv = String::from(
        "rank,run_id,config_id,target_qty,max_target_qty,max_position_notional_usd,\
         slippage_bps,volatility_mult_bps,total_return_pct,buy_and_hold_return_pct,\
         alpha_pct,max_drawdown_pct,fill_count,trade_count,win_rate_pct,profit_factor,\
         halted,artifact_path\n",
    );
    for r in rows {
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{:.4},{},{},{:.4},{},{},{},{},{},{}",
            r.rank,
            r.run_id,
            r.config_id,
            r.target_qty,
            r.max_target_qty.map(|v| v.to_string()).unwrap_or_default(),
            r.max_position_notional_usd
                .map(|v| v.to_string())
                .unwrap_or_default(),
            r.slippage_bps,
            r.volatility_mult_bps,
            r.total_return_pct,
            r.buy_and_hold_return_pct
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            r.alpha_pct.map(|v| format!("{v:.4}")).unwrap_or_default(),
            r.max_drawdown_pct,
            r.fill_count,
            r.trade_count,
            r.win_rate_pct
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            r.profit_factor
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            r.halted,
            r.artifact_path.as_deref().unwrap_or(""),
        )
        .unwrap();
    }
    std::fs::write(&csv_path, &csv)
        .with_context(|| format!("write sweep_summary.csv failed: {}", csv_path.display()))?;

    // --- sweep_summary.json ---
    let json_path = sweep_dir.join("sweep_summary.json");
    let json_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "rank": r.rank,
                "run_id": r.run_id,
                "config_id": r.config_id,
                "target_qty": r.target_qty,
                "max_target_qty": r.max_target_qty,
                "max_position_notional_usd": r.max_position_notional_usd,
                "slippage_bps": r.slippage_bps,
                "volatility_mult_bps": r.volatility_mult_bps,
                "total_return_pct": r.total_return_pct,
                "buy_and_hold_return_pct": r.buy_and_hold_return_pct,
                "alpha_pct": r.alpha_pct,
                "max_drawdown_pct": r.max_drawdown_pct,
                "fill_count": r.fill_count,
                "trade_count": r.trade_count,
                "win_rate_pct": r.win_rate_pct,
                "profit_factor": r.profit_factor,
                "halted": r.halted,
                "artifact_path": r.artifact_path,
            })
        })
        .collect();
    let json_obj = serde_json::json!({
        "schema_version": "sweep-summary-v1",
        "total_runs": rows.len(),
        "ranking_note": "sorted by alpha_pct desc (or total_return_pct if no benchmark), then max_drawdown_pct asc, then run_id asc",
        "rows": json_rows,
    });
    let json_str =
        serde_json::to_string_pretty(&json_obj).context("serialize sweep_summary.json failed")?;
    std::fs::write(&json_path, format!("{json_str}\n"))
        .with_context(|| format!("write sweep_summary.json failed: {}", json_path.display()))?;

    // --- sweep_report.md ---
    let md_path = sweep_dir.join("sweep_report.md");
    let mut md = String::from("# Sweep Summary\n\n");
    writeln!(md, "Total runs: {}\n", rows.len()).unwrap();
    writeln!(md, "| Rank | target_qty | slippage_bps | vol_mult_bps | return% | alpha% | dd% | fills | wins | halted |").unwrap();
    writeln!(md, "|------|-----------|-------------|--------------|---------|--------|-----|-------|------|--------|").unwrap();
    for r in rows {
        writeln!(
            md,
            "| {} | {} | {} | {} | {:.2} | {} | {:.2} | {} | {} | {} |",
            r.rank,
            r.target_qty,
            r.slippage_bps,
            r.volatility_mult_bps,
            r.total_return_pct,
            r.alpha_pct
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
            r.max_drawdown_pct,
            r.fill_count,
            r.win_rate_pct
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "n/a".to_string()),
            if r.halted { "YES" } else { "no" },
        )
        .unwrap();
    }
    md.push_str("\n> **Warning:** Sweep rankings reflect in-sample performance only. ");
    md.push_str("Higher alpha on training data does not predict live performance. ");
    md.push_str("Use sweeps to compare hypotheses, not to blindly select a configuration.\n");
    std::fs::write(&md_path, &md)
        .with_context(|| format!("write sweep_report.md failed: {}", md_path.display()))?;

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
