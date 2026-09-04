use anyhow::{Context, Result};
use chrono::Utc;
use clap::ValueEnum;
use std::path::{Path, PathBuf};

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestInstrumentEconomics,
    MarketRegimeClassification, MarketRegimeFeatures, MarketRegimePolicy, StrategySizingConfig,
    SweepGrid, SweepRowResult, SWEEP_MAX_COMBINATIONS,
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
// Shared economics flag wiring (CSV + DB backtest CLI entry points)
// ---------------------------------------------------------------------------

/// BACKTEST-ECONOMICS-DB-CLI-ENTRY-01: shared opt-in economics builder for
/// every backtest CLI entry point (csv, db). If none of the three flags are
/// supplied, returns `None` and the caller keeps the engine's default equity
/// economics (multiplier=1, no margin) -- byte-identical to pre-flag
/// behavior. If `--contract-multiplier` is omitted but a margin flag is
/// supplied, the multiplier defaults to 1. A non-positive multiplier fails
/// closed here, before the caller does any further work.
fn build_backtest_economics_from_cli_flags(
    contract_multiplier: Option<i64>,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
) -> Result<Option<BacktestInstrumentEconomics>> {
    if contract_multiplier.is_none()
        && initial_margin_micros.is_none()
        && maintenance_margin_micros.is_none()
    {
        return Ok(None);
    }
    let multiplier = contract_multiplier.unwrap_or(1);
    let economics = BacktestInstrumentEconomics::new(
        multiplier,
        initial_margin_micros,
        maintenance_margin_micros,
    )
    .with_context(|| format!("invalid --contract-multiplier {}", multiplier))?;
    Ok(Some(economics))
}

// ---------------------------------------------------------------------------
// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: inline promotion evidence
// ---------------------------------------------------------------------------

/// Generate and persist the complete Backtest-side promotion evidence set
/// (stress suite + P9 robustness gauntlet) for a real backtest run, using
/// the SAME genuine `report`/`config`/`bars`/strategy factory the caller
/// just used for the actual run -- never reconstructed or replayed from a
/// derived artifact. Called immediately after `write_backtest_report`
/// while the real inputs are still in scope, from every CLI backtest entry
/// point that persists artifacts at all (`--out-dir` supplied).
///
/// `make_strategy` must return a FRESH strategy instance each call (the
/// same contract `run_backtest_stress_suite`/`run_robustness_gauntlet`
/// already require) -- a `PluginRegistry::instantiate(&self, ...)` closure
/// satisfies this since `instantiate` only borrows the registry.
fn write_inline_promotion_evidence(
    run_dir: &Path,
    report: &mqk_backtest::BacktestReport,
    config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: impl Fn() -> Box<dyn mqk_strategy::Strategy>,
) -> Result<()> {
    let stress_output =
        mqk_backtest::run_backtest_stress_suite(report, config, bars, &make_strategy);
    mqk_artifacts::write_canonical_stress_suite(run_dir, &stress_output)
        .context("write_canonical_stress_suite failed")?;

    let gauntlet_output = mqk_backtest::run_robustness_gauntlet(report, config, bars, &make_strategy);
    mqk_artifacts::write_canonical_robustness_gauntlet(run_dir, &gauntlet_output)
        .context("write_canonical_robustness_gauntlet failed")?;

    Ok(())
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
    contract_multiplier: Option<i64>,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
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

    // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: a clone taken before
    // `cfg` moves into the engine -- the stress/robustness re-runs need the
    // exact same base config the real run used.
    let cfg_for_evidence = cfg.clone();
    let mut engine = BacktestEngine::new(cfg);

    // BACKTEST-ECONOMICS-CLI-ENTRY-01: opt-in economics wiring, before any
    // artifact directory is created. See build_backtest_economics_from_cli_flags.
    if let Some(economics) = build_backtest_economics_from_cli_flags(
        contract_multiplier,
        initial_margin_micros,
        maintenance_margin_micros,
    )? {
        engine = engine.with_economics(economics);
    }

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
    println!(
        "economics_contract_multiplier={}",
        report.economics.contract_multiplier
    );
    println!(
        "economics_margin_enforced={}",
        report.economics.margin_enforced
    );

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

        // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: real stress suite
        // + P9 robustness gauntlet, using the SAME genuine cfg/bars/reg this
        // run just used -- reg is still owned here (instantiate only
        // borrows), never reconstructed.
        write_inline_promotion_evidence(&init_result.run_dir, &report, &cfg_for_evidence, &bars, || {
            reg.instantiate(&strategy).expect("strategy known valid: already instantiated once above")
        })
        .with_context(|| {
            format!(
                "write inline promotion evidence failed: {}",
                init_result.run_dir.display()
            )
        })?;

        println!("artifacts_written=true");
        println!("artifacts_dir={}", init_result.run_dir.display());
        println!("manifest={}", init_result.manifest_path.display());
        println!("promotion_evidence_written=true");
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
// Research-only regime detection report
// ---------------------------------------------------------------------------

pub fn run_regime_detect(
    csv_path: String,
    symbol: String,
    timeframe: String,
    json: bool,
) -> Result<()> {
    let bars = mqk_backtest::load_csv_file(&csv_path)
        .with_context(|| format!("load bars csv failed: {}", csv_path))?;
    let input = mqk_backtest::MarketRegimeInput::from_bars(bars, Some(symbol), Some(timeframe));
    let report =
        mqk_backtest::detect_market_regime(&input, &MarketRegimePolicy::conservative_defaults());

    if json {
        print_regime_json(&report)?;
    } else {
        print_regime_text(&report);
    }

    Ok(())
}

fn print_regime_text(report: &MarketRegimeClassification) {
    let reason_codes: Vec<&str> = report.reason_codes.iter().map(|r| r.code()).collect();

    println!("symbol={}", report.symbol.as_deref().unwrap_or(""));
    println!("timeframe={}", report.timeframe.as_deref().unwrap_or(""));
    println!("bar_count={}", report.bar_count);
    println!("valid_bar_count={}", report.features.valid_bar_count);
    println!("regime_kind={}", report.kind.code());
    println!("confidence={:.4}", report.confidence.score);
    println!(
        "return_pct={}",
        format_optional_f64(report.features.return_pct)
    );
    println!(
        "realized_volatility_pct={}",
        format_optional_f64(report.features.realized_volatility_pct)
    );
    println!(
        "average_range_pct={}",
        format_optional_f64(report.features.average_range_pct)
    );
    println!(
        "directional_consistency={}",
        format_optional_f64(report.features.directional_consistency)
    );
    println!(
        "max_drawdown_pct={}",
        format_optional_f64(report.features.max_drawdown_pct)
    );
    println!(
        "volume_trend_pct={}",
        format_optional_f64(report.features.volume_trend_pct)
    );
    println!("reason_codes={}", reason_codes.join(","));
}

fn print_regime_json(report: &MarketRegimeClassification) -> Result<()> {
    let reason_codes: Vec<&str> = report.reason_codes.iter().map(|r| r.code()).collect();
    let features = &report.features;
    let value = serde_json::json!({
        "symbol": report.symbol,
        "timeframe": report.timeframe,
        "bar_count": report.bar_count,
        "valid_bar_count": features.valid_bar_count,
        "regime_kind": report.kind.code(),
        "confidence": report.confidence.score,
        "features": regime_features_json(features),
        "reason_codes": reason_codes,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&value).context("serialize regime report failed")?
    );
    Ok(())
}

fn regime_features_json(features: &MarketRegimeFeatures) -> serde_json::Value {
    serde_json::json!({
        "input_bar_count": features.input_bar_count,
        "valid_bar_count": features.valid_bar_count,
        "return_pct": features.return_pct,
        "realized_volatility_pct": features.realized_volatility_pct,
        "average_range_pct": features.average_range_pct,
        "directional_consistency": features.directional_consistency,
        "max_drawdown_pct": features.max_drawdown_pct,
        "volume_trend_pct": features.volume_trend_pct,
    })
}

fn format_optional_f64(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.4}"))
        .unwrap_or_else(|| "n/a".to_string())
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
    contract_multiplier: Option<i64>,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
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

    // BACKTEST-ECONOMICS-DB-CLI-ENTRY-01: validate economics flags before
    // connecting to the DB or loading any bars, so an invalid
    // --contract-multiplier fails closed without a wasted DB round trip.
    let economics = build_backtest_economics_from_cli_flags(
        contract_multiplier,
        initial_margin_micros,
        maintenance_margin_micros,
    )?;

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

    // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: a clone taken before
    // `cfg` moves into the engine -- the stress/robustness re-runs need the
    // exact same base config the real run used.
    let cfg_for_evidence = cfg.clone();
    let mut engine = BacktestEngine::new(cfg);
    if let Some(economics) = economics {
        engine = engine.with_economics(economics);
    }
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
    println!(
        "economics_contract_multiplier={}",
        report.economics.contract_multiplier
    );
    println!(
        "economics_margin_enforced={}",
        report.economics.margin_enforced
    );

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

        // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: real stress suite
        // + P9 robustness gauntlet, using the SAME genuine cfg/bars/reg this
        // run just used -- reg is still owned here (instantiate only
        // borrows), never reconstructed.
        write_inline_promotion_evidence(&init_result.run_dir, &report, &cfg_for_evidence, &bars, || {
            reg.instantiate(&strategy).expect("strategy known valid: already instantiated once above")
        })
        .with_context(|| {
            format!(
                "write inline promotion evidence failed: {}",
                init_result.run_dir.display()
            )
        })?;

        println!("artifacts_written=true");
        println!("artifacts_dir={}", init_result.run_dir.display());
        println!("manifest={}", init_result.manifest_path.display());
        println!("promotion_evidence_written=true");
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
    max_participation_rate_bps_list: String,
    contract_multiplier: Option<i64>,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
    out_dir: Option<String>,
    max_combinations_override: Option<usize>,
) -> Result<()> {
    if timeframe_secs <= 0 {
        anyhow::bail!("--timeframe-secs must be > 0");
    }
    if initial_cash_micros <= 0 {
        anyhow::bail!("--initial-cash-micros must be > 0");
    }

    // BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01: validate economics
    // flags before loading bars, so an invalid --contract-multiplier fails
    // closed without any file I/O. Same opt-in helper already proven on
    // `mqk backtest csv`/`mqk backtest db`; applied identically to every
    // sweep combination.
    let economics = build_backtest_economics_from_cli_flags(
        contract_multiplier,
        initial_margin_micros,
        maintenance_margin_micros,
    )?;

    let bars = mqk_backtest::load_csv_file(&bars_path)
        .with_context(|| format!("load bars csv failed: {}", bars_path))?;

    let target_qty_vals = parse_i64_list(&target_qty_list)?;
    let slippage_bps_vals = parse_i64_list(&slippage_bps_list)?;
    let vol_mult_vals = parse_i64_list(&volatility_mult_bps_list)?;
    let max_participation_rate_bps_vals = parse_i64_list(&max_participation_rate_bps_list)?;

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
        max_participation_rate_bps: max_participation_rate_bps_vals,
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
            // BKT-BAR-VOLUME-IMPACT-STRESS-01: not a swept dimension --
            // carried through from the base config rather than silently
            // reset to 0.
            participation_impact_bps: base_cfg.stress.participation_impact_bps,
        };
        cfg.liquidity = mqk_backtest::LiquidityConfig {
            max_participation_rate_bps: pt.max_participation_rate_bps,
        };

        let mut engine = BacktestEngine::new(cfg);
        if let Some(ref econ) = economics {
            engine = engine.with_economics(econ.clone());
        }
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
            "sweep_run={}/{} tq={} slip={} vol={} liq_cap={} return={:.2}% liq_rejects={} halted={}",
            i + 1,
            combos.len(),
            pt.target_qty,
            pt.slippage_bps,
            pt.volatility_mult_bps,
            pt.max_participation_rate_bps,
            results.last().unwrap().total_return_pct,
            results.last().unwrap().rejected_liquidity_capacity_count,
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

    let effective_economics = economics.unwrap_or_else(BacktestInstrumentEconomics::equity);
    println!("sweep_ok=true");
    println!("sweep_total_runs={}", results.len());
    println!(
        "sweep_economics_contract_multiplier={}",
        effective_economics.contract_multiplier
    );
    if let Some(best) = results.first() {
        println!(
            "sweep_best_run_id={} tq={} slip={} vol={} liq_cap={} return={:.2}% alpha={:.2}% dd={:.2}%",
            best.run_id,
            best.target_qty,
            best.slippage_bps,
            best.volatility_mult_bps,
            best.max_participation_rate_bps,
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
         slippage_bps,volatility_mult_bps,max_participation_rate_bps,total_return_pct,\
         buy_and_hold_return_pct,alpha_pct,max_drawdown_pct,fill_count,\
         rejected_liquidity_capacity_count,trade_count,win_rate_pct,profit_factor,\
         halted,artifact_path\n",
    );
    for r in rows {
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{:.4},{},{},{:.4},{},{},{},{},{},{},{}",
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
            r.max_participation_rate_bps,
            r.total_return_pct,
            r.buy_and_hold_return_pct
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            r.alpha_pct.map(|v| format!("{v:.4}")).unwrap_or_default(),
            r.max_drawdown_pct,
            r.fill_count,
            r.rejected_liquidity_capacity_count,
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
                "max_participation_rate_bps": r.max_participation_rate_bps,
                "total_return_pct": r.total_return_pct,
                "buy_and_hold_return_pct": r.buy_and_hold_return_pct,
                "alpha_pct": r.alpha_pct,
                "max_drawdown_pct": r.max_drawdown_pct,
                "fill_count": r.fill_count,
                "rejected_liquidity_capacity_count": r.rejected_liquidity_capacity_count,
                "trade_count": r.trade_count,
                "win_rate_pct": r.win_rate_pct,
                "profit_factor": r.profit_factor,
                "halted": r.halted,
                "artifact_path": r.artifact_path,
            })
        })
        .collect();
    // BKT-BAR-VOLUME-CAPACITY-SWEEP-01: max_participation_rate_bps and
    // rejected_liquidity_capacity_count are purely additive row fields.
    // mqk-artifacts' BacktestReportArtifact establishes this repo's schema-
    // versioning convention (see PROMOTION-EVIDENCE-SEMANTIC-BINDING-01's
    // doc comment on `strategy_semantic_fingerprint`): a purely additive
    // field does not require a schema-version bump. No typed consumer reads
    // sweep_summary.json today, so schema_version stays "sweep-summary-v1"
    // -- proven by `sweep_summary_schema_version_unchanged_by_new_columns`.
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
    writeln!(md, "| Rank | target_qty | slippage_bps | vol_mult_bps | liq_cap_bps | return% | alpha% | dd% | fills | liq_rejects | wins | halted |").unwrap();
    writeln!(md, "|------|-----------|-------------|--------------|-------------|---------|--------|-----|-------|-------------|------|--------|").unwrap();
    for r in rows {
        writeln!(
            md,
            "| {} | {} | {} | {} | {} | {:.2} | {} | {:.2} | {} | {} | {} | {} |",
            r.rank,
            r.target_qty,
            r.slippage_bps,
            r.volatility_mult_bps,
            r.max_participation_rate_bps,
            r.total_return_pct,
            r.alpha_pct
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
            r.max_drawdown_pct,
            r.fill_count,
            r.rejected_liquidity_capacity_count,
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
// STRATEGY-LAB-SCANNER-01C: local-data-only strategy/symbol scanner runner
//
// STRATEGY-SCANNER-JOBS-GUI-01B: the scan-execution and artifact-writing
// logic now lives in `mqk_backtest::{execute_strategy_scan,
// write_scan_artifacts}` so the daemon's `POST /api/v1/strategy-scans/jobs`
// runs the identical scan without shelling out to this CLI binary. This
// function is now a thin CLI wrapper: parse/validate flags, call the shared
// functions, print CLI-formatted output. Artifact schema and scan_id
// derivation are unchanged.
// ---------------------------------------------------------------------------

/// Scan the enabled-equity registry universe against local bar CSVs only.
///
/// No provider call, no broker call, no live/paper order, no DB
/// connection. The only IO is: read `registry`, read `{bars_root}/
/// {timeframe}/{symbol}_{timeframe}.csv` files, and (unless `dry_run`)
/// write the artifact directory under `out_dir`.
#[allow(clippy::too_many_arguments)]
pub fn run_strategy_scan(
    registry_path: String,
    bars_root: String,
    timeframe: String,
    strategy_ids: String,
    top: usize,
    limit_symbols: Option<usize>,
    out_dir: String,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let strategies: Vec<String> = strategy_ids
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if strategies.is_empty() {
        anyhow::bail!("--strategy must name at least one strategy_id");
    }
    if top == 0 {
        anyhow::bail!("--top must be > 0");
    }

    let req = mqk_backtest::ScanRunRequest {
        registry_path: registry_path.clone(),
        bars_root: bars_root.clone(),
        timeframe: timeframe.clone(),
        strategies: strategies.clone(),
        top,
        limit_symbols,
        git_hash: bkt_git_hash(),
        created_at_utc: Utc::now().to_rfc3339(), // allow: operational manifest timestamp
    };
    let output = mqk_backtest::execute_strategy_scan(&req)
        .map_err(|e| anyhow::anyhow!("strategy scan failed: {e}"))?;

    let mut artifacts_written = false;
    let mut artifacts_dir: Option<PathBuf> = None;
    if !dry_run {
        let run_dir = mqk_backtest::write_scan_artifacts(Path::new(&out_dir), &output)
            .map_err(|e| anyhow::anyhow!("write scan artifacts failed: {e}"))?;
        artifacts_written = true;
        artifacts_dir = Some(run_dir);
    }

    let manifest = &output.manifest;
    let candidates = &output.candidates;
    let summary = &output.summary;

    if json {
        // JSON mode: stdout carries exactly one JSON value (the summary),
        // nothing else -- callers can pipe stdout straight into a parser.
        println!(
            "{}",
            serde_json::to_string_pretty(summary).context("serialize scan summary failed")?
        );
    } else {
        println!("scan_id={}", manifest.scan_id);
        println!("registry_path={}", manifest.registry_path);
        println!("bars_root={}", manifest.bars_root);
        println!("timeframe={}", manifest.timeframe);
        println!("strategies={}", manifest.strategies.join(","));
        println!("universe_count={}", manifest.universe_count);
        println!("ranked_count={}", manifest.ranked_count);
        println!("skipped_count={}", manifest.skipped_count);
        println!("artifacts_written={artifacts_written}");
        if let Some(dir) = &artifacts_dir {
            println!("artifacts_dir={}", dir.display());
        }
        for w in &manifest.warnings {
            println!("warning={w}");
        }
        for c in candidates.iter().filter(|c| c.rank.is_some()).take(top) {
            println!(
                "rank={} symbol={} timeframe={} strategy_id={} score={} total_return_pct={} truth_state={} reason_code={}",
                c.rank.unwrap_or(0),
                c.symbol,
                c.timeframe,
                c.strategy_id,
                c.score.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".to_string()),
                c.metrics.total_return_pct.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".to_string()),
                c.truth_state.code(),
                c.reason_code.code(),
            );
        }
        for skip in &summary.top_skip_reasons {
            println!("skip_reason={} count={}", skip.reason_code, skip.count);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01C: research-review classification over an
// existing scanner artifact. Thin CLI wrapper: parse/validate flags, call
// the shared `mqk_backtest::{execute_strategy_scan_review,
// write_review_artifacts}` functions, print CLI-formatted output. No
// provider call, no broker call, no live/paper order, no DB connection --
// the only IO is reading the named scanner artifact files and (always)
// writing the review artifact directory.
// ---------------------------------------------------------------------------

/// Classify every candidate in an existing `mqk backtest scan-strategies`
/// artifact directory into a research-review state, and write a review
/// artifact directory alongside it.
pub fn run_review_scan(
    artifact_dir: String,
    out_dir: String,
    top: usize,
    json: bool,
) -> Result<()> {
    if top == 0 {
        anyhow::bail!("--top must be > 0");
    }

    let req = mqk_backtest::ReviewRunRequest {
        artifact_dir: artifact_dir.clone(),
        top,
        policy: mqk_backtest::StrategyScanReviewPolicy::default(),
        git_hash: bkt_git_hash(),
        created_at_utc: Utc::now().to_rfc3339(), // allow: operational manifest timestamp
    };
    let output = mqk_backtest::execute_strategy_scan_review(&req)
        .map_err(|e| anyhow::anyhow!("strategy scan review failed: {e}"))?;

    let run_dir = mqk_backtest::write_review_artifacts(Path::new(&out_dir), &output)
        .map_err(|e| anyhow::anyhow!("write review artifacts failed: {e}"))?;

    let manifest = &output.manifest;
    let summary = &output.summary;

    if json {
        // JSON mode: stdout carries exactly one JSON value (the summary),
        // nothing else -- callers can pipe stdout straight into a parser.
        println!(
            "{}",
            serde_json::to_string_pretty(summary).context("serialize review summary failed")?
        );
    } else {
        println!("review_id={}", manifest.review_id);
        println!("scanner_scan_id={}", manifest.scanner_scan_id);
        println!("source_artifact_dir={}", manifest.source_artifact_dir);
        println!("review_artifact_dir={}", run_dir.display());
        println!("candidate_count={}", manifest.candidate_count);
        println!("blocked_count={}", manifest.blocked_count);
        println!("needs_review_count={}", manifest.needs_review_count);
        println!(
            "watchlist_candidate_count={}",
            manifest.watchlist_candidate_count
        );
        println!("paper_candidate_count={}", manifest.paper_candidate_count);
        println!("rejected_count={}", manifest.rejected_count);
        for w in &manifest.warnings {
            println!("warning={w}");
        }
        for d in &summary.top_paper_candidates {
            println!(
                "paper_candidate symbol={} timeframe={} strategy_id={} scanner_rank={} scanner_score={}",
                d.symbol,
                d.timeframe,
                d.strategy_id,
                d.scanner_rank.map(|r| r.to_string()).unwrap_or_else(|| "n/a".to_string()),
                d.scanner_score.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".to_string()),
            );
        }
        for d in &summary.top_watchlist_candidates {
            println!(
                "watchlist_candidate symbol={} timeframe={} strategy_id={}",
                d.symbol, d.timeframe, d.strategy_id,
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: DSR/PBO sensitivity
// finalization
// ---------------------------------------------------------------------------

/// Merge the real DSR/PBO sensitivity result for a Research trial into an
/// existing candidate's `robustness_gauntlet.json`, produced earlier by the
/// SAME candidate's real backtest execution (`mqk backtest csv`/`db`).
///
/// This is a genuinely SEPARATE production phase from backtest execution --
/// Research trial identity is a Python/research-py concept established by
/// the Research pipeline, never known to the Rust backtest engine at
/// execution time -- so it cannot run inline with the backtest the way the
/// stress suite / pure P9 scenarios do. `artifact_root`/`run_id` identify
/// the candidate's EXISTING evidence directory (the same
/// `artifact_root/<run_id>/` convention `resolve_backtest_evidence` uses);
/// this command loads it, cross-checks `--trial-id`'s own registered
/// `strategy_id` against that candidate's real `strategy_name` (refusing a
/// Research-trial mismatch), and merges the result via
/// `mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity`.
#[allow(clippy::too_many_arguments)]
pub fn run_finalize_robustness_sensitivity(
    artifact_root: String,
    run_id: String,
    registry_db: String,
    trial_id: String,
    judge_artifact_sha256: String,
    research_py_root: String,
    python: String,
    block_counts: String,
    dsr_max_sensitivity_range: f64,
    pbo_max_sensitivity_range: f64,
) -> Result<()> {
    let run_id: uuid::Uuid = run_id.parse().context("--run-id must be a valid UUID")?;
    let run_dir = Path::new(&artifact_root).join(run_id.to_string());

    let existing = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir).with_context(|| {
        format!(
            "existing robustness_gauntlet.json must already be real and structurally valid \
             at {} -- run the real backtest (mqk backtest csv/db with --out-dir) first",
            run_dir.display()
        )
    })?;

    let block_counts: Vec<u32> = parse_u32_list(&block_counts)?;
    if block_counts.is_empty() {
        anyhow::bail!("--block-counts must supply at least one value");
    }

    let sensitivity = mqk_backtest::dsr_pbo_sensitivity_scenario(
        &python,
        Path::new(&research_py_root),
        Path::new(&registry_db),
        &trial_id,
        &existing.strategy_name,
        &judge_artifact_sha256,
        &block_counts,
        dsr_max_sensitivity_range,
        pbo_max_sensitivity_range,
    );

    println!("scenario_name={}", sensitivity.name);
    println!("applicable={}", sensitivity.applicable);
    println!("passed={}", sensitivity.passed);
    if let Some(reason) = &sensitivity.reason {
        println!("reason={reason}");
    }

    let path = mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &run_dir,
        &sensitivity,
    )
    .with_context(|| {
        format!(
            "finalize_canonical_robustness_gauntlet_with_sensitivity failed for {}",
            run_dir.display()
        )
    })?;

    let finalized = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir)
        .context("re-loading the finalized artifact failed")?;

    println!("finalized_artifact={}", path.display());
    println!("scenarios_run={}", finalized.scenarios_run());
    println!("is_complete={}", finalized.is_complete());
    println!("all_applicable_passed={}", finalized.all_applicable_passed());

    Ok(())
}

// ---------------------------------------------------------------------------
// P7A-P7B-ECONOMIC-REPLAY-STRESS-01: genuine P7A/P7B stress finalization
// ---------------------------------------------------------------------------

/// Merge the real, genuine P7A/P7B economic replay stress result for a
/// Research trial into an existing candidate's `robustness_gauntlet.json`,
/// produced earlier by the SAME candidate's real backtest execution (`mqk
/// backtest csv`/`db`). Mirrors [`run_finalize_robustness_sensitivity`]
/// exactly -- a genuinely separate production phase, same cross-candidate
/// authority check, same merge seam
/// (`mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity`,
/// generalized to accept either deferred scenario).
#[allow(clippy::too_many_arguments)]
pub fn run_finalize_p7a_p7b_replay_stress(
    artifact_root: String,
    run_id: String,
    registry_db: String,
    trial_id: String,
    economic_eval_id: String,
    research_py_root: String,
    python: String,
    stress_out_dir: String,
    stress_execution_slippage_bps: u32,
    stress_execution_volatility_mult_bps: u32,
    stress_max_target_qty: Option<u32>,
    stress_max_position_notional_usd: Option<f64>,
    max_drawdown_ceiling: f64,
) -> Result<()> {
    let run_id: uuid::Uuid = run_id.parse().context("--run-id must be a valid UUID")?;
    let run_dir = Path::new(&artifact_root).join(run_id.to_string());

    let existing = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir).with_context(|| {
        format!(
            "existing robustness_gauntlet.json must already be real and structurally valid \
             at {} -- run the real backtest (mqk backtest csv/db with --out-dir) first",
            run_dir.display()
        )
    })?;

    let stress = mqk_backtest::p7a_p7b_economic_replay_stress_scenario(
        &python,
        Path::new(&research_py_root),
        Path::new(&registry_db),
        &trial_id,
        &economic_eval_id,
        &existing.strategy_name,
        Path::new(&stress_out_dir),
        stress_execution_slippage_bps,
        stress_execution_volatility_mult_bps,
        stress_max_target_qty,
        stress_max_position_notional_usd,
        max_drawdown_ceiling,
    );

    println!("scenario_name={}", stress.name);
    println!("applicable={}", stress.applicable);
    println!("passed={}", stress.passed);
    if let Some(reason) = &stress.reason {
        println!("reason={reason}");
    }

    let path =
        mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &stress)
            .with_context(|| {
                format!(
                    "finalize_canonical_robustness_gauntlet_with_sensitivity failed for {}",
                    run_dir.display()
                )
            })?;

    let finalized = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir)
        .context("re-loading the finalized artifact failed")?;

    println!("finalized_artifact={}", path.display());
    println!("scenarios_run={}", finalized.scenarios_run());
    println!("is_complete={}", finalized.is_complete());
    println!("all_applicable_passed={}", finalized.all_applicable_passed());

    Ok(())
}

// ---------------------------------------------------------------------------
// FINAL-P9-ROBUSTNESS-SEMANTICS-01: genuine shuffled placebo finalization
// ---------------------------------------------------------------------------

/// Merge the real genuine shuffled placebo result for a Research trial into
/// an existing candidate's `robustness_gauntlet.json`, produced earlier by
/// the SAME candidate's real backtest execution (`mqk backtest csv`/`db`).
/// Mirrors [`run_finalize_p7a_p7b_replay_stress`] exactly -- a genuinely
/// separate production phase, same cross-candidate authority check, same
/// merge seam (generalized to accept any of the three deferred scenarios).
pub fn run_finalize_genuine_shuffled_placebo(
    artifact_root: String,
    run_id: String,
    registry_db: String,
    trial_id: String,
    economic_eval_id: String,
    research_py_root: String,
    python: String,
    placebo_out_dir: String,
) -> Result<()> {
    let run_id: uuid::Uuid = run_id.parse().context("--run-id must be a valid UUID")?;
    let run_dir = Path::new(&artifact_root).join(run_id.to_string());

    let existing = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir).with_context(|| {
        format!(
            "existing robustness_gauntlet.json must already be real and structurally valid \
             at {} -- run the real backtest (mqk backtest csv/db with --out-dir) first",
            run_dir.display()
        )
    })?;

    let placebo = mqk_backtest::genuine_shuffled_placebo_scenario(
        &python,
        Path::new(&research_py_root),
        Path::new(&registry_db),
        &trial_id,
        &economic_eval_id,
        &existing.strategy_name,
        Path::new(&placebo_out_dir),
    );

    println!("scenario_name={}", placebo.name);
    println!("applicable={}", placebo.applicable);
    println!("passed={}", placebo.passed);
    if let Some(reason) = &placebo.reason {
        println!("reason={reason}");
    }

    let path =
        mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &placebo)
            .with_context(|| {
                format!(
                    "finalize_canonical_robustness_gauntlet_with_sensitivity failed for {}",
                    run_dir.display()
                )
            })?;

    let finalized = mqk_artifacts::load_canonical_robustness_gauntlet(&run_dir)
        .context("re-loading the finalized artifact failed")?;

    println!("finalized_artifact={}", path.display());
    println!("scenarios_run={}", finalized.scenarios_run());
    println!("is_complete={}", finalized.is_complete());
    println!("all_applicable_passed={}", finalized.all_applicable_passed());

    Ok(())
}

fn parse_u32_list(s: &str) -> Result<Vec<u32>> {
    s.split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.parse::<u32>().with_context(|| format!("invalid integer: {v}")))
        .collect()
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
