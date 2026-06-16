use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

use commands::{
    bkt::{
        run_backtest_csv, run_backtest_db, run_strategy_lab_evaluate, run_sweep_csv,
        IntegrityCalendarArg,
    },
    load_payload,
    md::{md_ingest_csv, md_ingest_provider, md_sync_provider},
    run::{
        run_arm, run_begin, run_deadman_check, run_deadman_enforce, run_halt, run_heartbeat,
        run_start, run_status, run_stop,
    },
};
// RT-2: run_execute uses BrokerGateway::for_test (stub wiring); gated in production.
// RT-8: run_loop uses mqk_testkit::Orchestrator; gated in production.
#[cfg(feature = "testkit")]
use commands::run::{run_execute, run_loop};

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

        /// Strategy name to run (see `mqk backtest list-strategies`).
        /// Available: swing_momentum, mean_reversion, volatility_breakout, intraday_scalper.
        #[arg(long, default_value = "swing_momentum")]
        strategy: String,

        /// Primary symbol for the strategy.
        #[arg(long, default_value = "SPY")]
        symbol: String,

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

        /// Integrity stale threshold in ticks (seconds for time-indexed bar feeds).
        /// Default 120 is correct for intraday data (1m/5m bars with ~60-300 s gaps).
        /// For daily bars (timeframe_secs=86400), use at least 172800: daily gaps are
        /// 86400 s and a threshold of 120 would immediately set execution_blocked=true.
        /// Weekend gaps in daily data can reach 259200 s (3 days); 172800 covers
        /// datasets that store only trading-day timestamps with no weekend entries.
        #[arg(long, default_value_t = 120)]
        integrity_stale_threshold_ticks: u64,

        /// Integrity gap tolerance (missing bars).
        #[arg(long, default_value_t = 0)]
        integrity_gap_tolerance_bars: u32,

        /// Integrity calendar for gap detection.
        #[arg(long, value_enum, default_value = "always-on")]
        integrity_calendar: IntegrityCalendarArg,

        /// Target share count for intraday_scalper strategy (default: 1).
        /// Captured in config_id — different values produce different run identities.
        #[arg(long, default_value_t = 1)]
        target_qty: i64,

        /// Hard cap on target share count for intraday_scalper (optional; absent = no cap).
        #[arg(long)]
        max_target_qty: Option<i64>,

        /// Hard cap on position notional in whole USD for intraday_scalper (optional; absent = no cap).
        #[arg(long)]
        max_position_notional_usd: Option<i64>,

        /// Optional output directory for deterministic artifacts (fills/equity/metrics).
        #[arg(long)]
        out_dir: Option<String>,
    },

    /// Run a deterministic parameter sweep over a CSV bars file.
    ///
    /// Generates the Cartesian product of target-qty × slippage-bps × volatility-mult-bps,
    /// runs each combination, and writes sweep_summary.csv + sweep_summary.json + sweep_report.md.
    CsvSweep {
        /// Path to bars CSV file.
        #[arg(long)]
        bars: String,

        /// Strategy name to run.
        #[arg(long, default_value = "swing_momentum")]
        strategy: String,

        /// Primary symbol for the strategy.
        #[arg(long, default_value = "SPY")]
        symbol: String,

        /// Timeframe seconds (must match strategy spec).
        #[arg(long, default_value_t = 60)]
        timeframe_secs: i64,

        /// Initial cash in micros.
        #[arg(long, default_value_t = 100_000_000_000)]
        initial_cash_micros: i64,

        /// Enable integrity checks.
        #[arg(long, default_value_t = true)]
        integrity_enabled: bool,

        /// Integrity stale threshold in ticks.
        #[arg(long, default_value_t = 120)]
        integrity_stale_threshold_ticks: u64,

        /// Integrity gap tolerance (missing bars).
        #[arg(long, default_value_t = 0)]
        integrity_gap_tolerance_bars: u32,

        /// Comma-separated target_qty values to sweep (e.g. "1,3,5").
        #[arg(long)]
        target_qty: String,

        /// Comma-separated slippage_bps values to sweep (e.g. "5,10").
        #[arg(long)]
        slippage_bps: String,

        /// Comma-separated volatility_mult_bps values to sweep.
        /// If omitted, uses the base config default (5000).
        #[arg(long, default_value = "")]
        volatility_mult_bps: String,

        /// Output directory for individual run artifacts and sweep summary.
        #[arg(long)]
        out_dir: Option<String>,

        /// Override the maximum combinations limit (default: 100).
        /// Use with caution: large sweeps take proportionally longer.
        #[arg(long)]
        max_combinations: Option<usize>,
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

        /// Strategy name to run.
        #[arg(long, default_value = "swing_momentum")]
        strategy: String,

        /// Primary symbol for the strategy.
        #[arg(long, default_value = "SPY")]
        symbol: String,

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

        /// Integrity stale threshold in ticks (seconds for time-indexed bar feeds).
        /// Default 120 is correct for intraday data (1m/5m bars with ~60-300 s gaps).
        /// For daily bars (timeframe_secs=86400), use at least 172800.
        #[arg(long, default_value_t = 120)]
        integrity_stale_threshold_ticks: u64,

        /// Target share count for intraday_scalper strategy (default: 1).
        /// Captured in config_id — different values produce different run identities.
        #[arg(long, default_value_t = 1)]
        target_qty: i64,

        /// Hard cap on target share count for intraday_scalper (optional; absent = no cap).
        #[arg(long)]
        max_target_qty: Option<i64>,

        /// Hard cap on position notional in whole USD for intraday_scalper (optional; absent = no cap).
        #[arg(long)]
        max_position_notional_usd: Option<i64>,

        /// Optional output directory for deterministic artifacts (fills/equity/metrics/manifest).
        #[arg(long)]
        out_dir: Option<String>,
    },

    /// Evaluate an existing completed backtest artifact folder with Strategy Lab.
    StrategyLabEvaluate {
        /// Existing artifact run directory containing metrics.json.
        #[arg(long)]
        artifact_dir: String,

        /// Print a deterministic JSON report instead of key=value lines.
        #[arg(long, default_value_t = false)]
        json: bool,
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

        /// Comma-separated symbols. Mutually exclusive with --symbols-from-registry.
        #[arg(long)]
        symbols: Option<String>,

        /// Path to instrument registry JSON (e.g. config/instruments/equities.json).
        /// Loads all enabled equity symbols. Mutually exclusive with --symbols.
        #[arg(long)]
        symbols_from_registry: Option<PathBuf>,

        /// Timeframe (1D | 1h | 1m | 5m)
        #[arg(long)]
        timeframe: String,

        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: String,

        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: String,
    },

    /// Incremental historical market-data sync from a provider into canonical md_bars.
    ///
    /// For each symbol, detects the latest stored bar end_ts and requests only the bars
    /// needed to extend coverage.  Requires --full-start when no bars exist for a symbol.
    /// Overlap is subtracted from the latest stored bar's date to re-ingest recent bars
    /// and handle late completions.
    SyncProvider {
        /// Provider source name (only: twelvedata)
        #[arg(long)]
        source: String,

        /// Comma-separated symbols. Mutually exclusive with --symbols-from-registry.
        #[arg(long)]
        symbols: Option<String>,

        /// Path to instrument registry JSON (e.g. config/instruments/equities.json).
        /// Loads all enabled equity symbols. Mutually exclusive with --symbols.
        #[arg(long)]
        symbols_from_registry: Option<PathBuf>,

        /// Timeframe (1D | 1h | 1m | 5m)
        #[arg(long)]
        timeframe: String,

        /// Initial backfill start date (YYYY-MM-DD).
        /// Required when no bars exist yet for a symbol; ignored for symbols that already
        /// have stored bars (incremental start is computed from the latest bar + overlap).
        #[arg(long)]
        full_start: Option<String>,

        /// End date (YYYY-MM-DD). Defaults to today (wall clock, operator command only).
        #[arg(long)]
        end: Option<String>,

        /// Overlap in calendar days subtracted from the latest stored bar date.
        /// Defaults: 5 for 1D, 2 for 1h, 2 for 5m, 1 for 1m.
        #[arg(long)]
        overlap_days: Option<u32>,
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

    /// FD-2: Run the authoritative ExecutionOrchestrator tick loop against a live DB.
    /// RT-2: only available when built with --features testkit.
    #[cfg(feature = "testkit")]
    Execute {
        /// Run id (must be RUNNING)
        #[arg(long)]
        run_id: String,

        /// Number of tick iterations to execute
        #[arg(long, default_value_t = 1)]
        ticks: u32,
    },

    /// Execute a deterministic orchestrator loop (testkit) with synthetic bars.
    /// RT-8: only available when built with --features testkit.
    #[cfg(feature = "testkit")]
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
                            "REFUSING MIGRATE: detected {} active LIVE run(s) in ARMED/RUNNING. Re-run with: mqk db migrate --yes",
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
            MdCmd::IngestCsv {
                path,
                timeframe,
                source,
            } => {
                md_ingest_csv(path, timeframe, source).await?;
            }
            MdCmd::IngestProvider {
                source,
                symbols,
                symbols_from_registry,
                timeframe,
                start,
                end,
            } => {
                md_ingest_provider(
                    source,
                    symbols,
                    symbols_from_registry,
                    timeframe,
                    start,
                    end,
                )
                .await?;
            }
            MdCmd::SyncProvider {
                source,
                symbols,
                symbols_from_registry,
                timeframe,
                full_start,
                end,
                overlap_days,
            } => {
                md_sync_provider(
                    source,
                    symbols,
                    symbols_from_registry,
                    timeframe,
                    full_start,
                    end,
                    overlap_days,
                )
                .await?;
            }
        },

        Commands::Backtest { cmd } => match cmd {
            BacktestCmd::Csv {
                bars,
                strategy,
                symbol,
                timeframe_secs,
                initial_cash_micros,
                shadow,
                integrity_enabled,
                integrity_stale_threshold_ticks,
                integrity_gap_tolerance_bars,
                integrity_calendar,
                target_qty,
                max_target_qty,
                max_position_notional_usd,
                out_dir,
            } => {
                run_backtest_csv(
                    bars,
                    strategy,
                    symbol,
                    timeframe_secs,
                    initial_cash_micros,
                    shadow,
                    integrity_enabled,
                    integrity_stale_threshold_ticks,
                    integrity_gap_tolerance_bars,
                    integrity_calendar,
                    target_qty,
                    max_target_qty,
                    max_position_notional_usd,
                    out_dir,
                )
                .await?;
            }
            BacktestCmd::CsvSweep {
                bars,
                strategy,
                symbol,
                timeframe_secs,
                initial_cash_micros,
                integrity_enabled,
                integrity_stale_threshold_ticks,
                integrity_gap_tolerance_bars,
                target_qty,
                slippage_bps,
                volatility_mult_bps,
                out_dir,
                max_combinations,
            } => {
                run_sweep_csv(
                    bars,
                    strategy,
                    symbol,
                    timeframe_secs,
                    initial_cash_micros,
                    integrity_enabled,
                    integrity_stale_threshold_ticks,
                    integrity_gap_tolerance_bars,
                    target_qty,
                    slippage_bps,
                    volatility_mult_bps,
                    out_dir,
                    max_combinations,
                )
                .await?;
            }
            BacktestCmd::Db {
                timeframe,
                start_end_ts,
                end_end_ts,
                symbols,
                strategy,
                symbol,
                timeframe_secs,
                initial_cash_micros,
                shadow,
                integrity_enabled,
                integrity_stale_threshold_ticks,
                target_qty,
                max_target_qty,
                max_position_notional_usd,
                out_dir,
            } => {
                run_backtest_db(
                    timeframe,
                    start_end_ts,
                    end_end_ts,
                    symbols,
                    strategy,
                    symbol,
                    timeframe_secs,
                    initial_cash_micros,
                    shadow,
                    integrity_enabled,
                    integrity_stale_threshold_ticks,
                    target_qty,
                    max_target_qty,
                    max_position_notional_usd,
                    out_dir,
                )
                .await?;
            }
            BacktestCmd::StrategyLabEvaluate { artifact_dir, json } => {
                run_strategy_lab_evaluate(artifact_dir, json)?;
            }
        },

        Commands::Run { cmd } => match cmd {
            RunCmd::Start {
                engine,
                mode,
                config_paths,
            } => {
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
            RunCmd::DeadmanCheck {
                run_id,
                ttl_seconds,
            } => {
                run_deadman_check(run_id, ttl_seconds).await?;
            }
            RunCmd::DeadmanEnforce {
                run_id,
                ttl_seconds,
            } => {
                run_deadman_enforce(run_id, ttl_seconds).await?;
            }
            #[cfg(feature = "testkit")]
            RunCmd::Execute { run_id, ticks } => {
                run_execute(run_id, ticks).await?;
            }
            #[cfg(feature = "testkit")]
            RunCmd::Loop {
                run_id,
                symbol,
                bars,
                timeframe_secs,
                exports_root,
                label,
            } => {
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
