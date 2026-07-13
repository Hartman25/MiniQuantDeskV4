#![recursion_limit = "256"]

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

use commands::{
    bkt::{
        run_backtest_csv, run_backtest_db, run_regime_detect, run_review_scan,
        run_strategy_lab_evaluate, run_strategy_lab_rank, run_strategy_scan, run_sweep_csv,
        IntegrityCalendarArg,
    },
    load_payload,
    md::{
        md_coinlore_latest_mark, md_crypto_registry_readiness, md_ingest_csv, md_ingest_provider,
        md_kraken_ohlc_dry_run, md_kraken_ohlc_ingest, md_kraken_ohlc_sync,
        md_kraken_scheduler_readiness, md_registry_v2_status, md_registry_v2_translation_check,
        md_sync_provider,
    },
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

    /// Read-only autonomous-paper operator reports.
    Autonomous {
        #[command(subcommand)]
        cmd: AutonomousCmd,
    },
}

#[derive(Subcommand)]
enum AutonomousCmd {
    /// AUTON-NO-TRADE-OFFHOURS-01D: print the most recent durable
    /// autonomous no-trade diagnostic rows (`autonomous_no_trade_diagnostics`).
    /// Read-only: no DB write, no runtime start, no broker/provider call.
    /// Mirrors `GET /api/v1/autonomous/no-trade-diagnostics`.
    NoTradeDiagnostics {
        /// Maximum rows to print, newest first.
        #[arg(long, default_value_t = 20)]
        limit: i64,
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

        /// Contract multiplier for multiplier-bearing instruments (futures/options-style
        /// metadata; e.g. ES futures = 50, standard equity options = 100). Omitted = equity
        /// default (multiplier=1), identical to current behavior. Metadata only -- does
        /// not enable non-equity execution or change order routing. Must be > 0 if supplied.
        #[arg(long)]
        contract_multiplier: Option<i64>,

        /// Optional initial margin metadata in micros, carried into report/artifact
        /// economics. Metadata only -- never enforced.
        #[arg(long)]
        initial_margin_micros: Option<i64>,

        /// Optional maintenance margin metadata in micros, carried into report/artifact
        /// economics. Metadata only -- never enforced.
        #[arg(long)]
        maintenance_margin_micros: Option<i64>,

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

        /// Contract multiplier for multiplier-bearing instruments (futures/options-style
        /// metadata; e.g. ES futures = 50, standard equity options = 100). Omitted = equity
        /// default (multiplier=1), identical to current behavior. Metadata only -- does
        /// not enable non-equity execution or change order routing. Must be > 0 if supplied.
        /// Applied identically to every sweep combination.
        #[arg(long)]
        contract_multiplier: Option<i64>,

        /// Optional initial margin metadata in micros, carried into report/artifact
        /// economics for every sweep combination. Metadata only -- never enforced.
        #[arg(long)]
        initial_margin_micros: Option<i64>,

        /// Optional maintenance margin metadata in micros, carried into report/artifact
        /// economics for every sweep combination. Metadata only -- never enforced.
        #[arg(long)]
        maintenance_margin_micros: Option<i64>,

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

        /// Contract multiplier for multiplier-bearing instruments (futures/options-style
        /// metadata; e.g. ES futures = 50, standard equity options = 100). Omitted = equity
        /// default (multiplier=1), identical to current behavior. Metadata only -- does
        /// not enable non-equity execution or change order routing. Must be > 0 if supplied.
        #[arg(long)]
        contract_multiplier: Option<i64>,

        /// Optional initial margin metadata in micros, carried into report/artifact
        /// economics. Metadata only -- never enforced.
        #[arg(long)]
        initial_margin_micros: Option<i64>,

        /// Optional maintenance margin metadata in micros, carried into report/artifact
        /// economics. Metadata only -- never enforced.
        #[arg(long)]
        maintenance_margin_micros: Option<i64>,

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

    /// Rank existing completed backtest artifact folders with Strategy Lab.
    StrategyLabRank {
        /// Root directory to scan for artifact folders containing metrics.json.
        #[arg(long)]
        artifacts_root: String,

        /// Limit ranked rows.
        #[arg(long)]
        top: Option<usize>,

        /// Print a deterministic JSON report instead of key=value lines.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Research-only market regime report over a CSV bars file.
    RegimeDetect {
        /// Path to bars CSV file.
        #[arg(long)]
        csv: String,

        /// Symbol label to include in the report.
        #[arg(long)]
        symbol: String,

        /// Timeframe label to include in the report.
        #[arg(long)]
        timeframe: String,

        /// Print deterministic JSON instead of key=value lines.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// STRATEGY-LAB-SCANNER-01C: local-data-only strategy/symbol/timeframe
    /// scanner. Resolves the enabled-equity universe from the instrument
    /// registry, evaluates each `(symbol, strategy)` pair against local
    /// bar CSVs only (no provider/broker/network call, no live/paper
    /// order), and writes a ranked-candidate artifact directory.
    ScanStrategies {
        /// Instrument registry JSON path.
        #[arg(long, default_value = "config/instruments/equities.json")]
        registry: String,

        /// Root directory containing `{timeframe}/{symbol}_{timeframe}.csv` bar files.
        #[arg(long, default_value = "exports/md_backup")]
        bars_root: String,

        /// Timeframe label to scan (e.g. "1D", "1H", "5m", "15m", "1m").
        #[arg(long, default_value = "1D")]
        timeframe: String,

        /// Comma-separated strategy_id list (see `mqk backtest csv --help` for
        /// available strategies). Default: swing_momentum.
        #[arg(long, default_value = "swing_momentum")]
        strategy: String,

        /// Limit the printed/artifact top-ranked rows.
        #[arg(long, default_value_t = 20)]
        top: usize,

        /// Optional cap on the number of universe symbols scanned
        /// (deterministic: takes the first N in alphabetical order).
        #[arg(long)]
        limit_symbols: Option<usize>,

        /// Output directory for the scan artifact tree.
        #[arg(long, default_value = "exports/strategy_scans")]
        out_dir: String,

        /// Evaluate and print results but do not write the artifact directory.
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Print a deterministic JSON report instead of key=value lines.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// STRATEGY-SCANNER-PROMOTION-01C: research-review classification over
    /// an existing `scan-strategies` artifact directory. Reads the scanner
    /// artifact, classifies every candidate into a deterministic research
    /// review state (never a trading approval), and writes a review
    /// artifact directory. No provider/broker/network call, no live/paper
    /// order, no DB connection.
    ReviewScan {
        /// Path to an existing scanner artifact directory (the
        /// `{scan_id}` directory written by `scan-strategies`).
        #[arg(long)]
        artifact_dir: String,

        /// Output directory for the review artifact tree.
        #[arg(long, default_value = "exports/strategy_reviews")]
        out_dir: String,

        /// Limit the printed/artifact top paper/watchlist candidate rows.
        #[arg(long, default_value_t = 50)]
        top: usize,

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

    /// ASSET-CORE-01C: read-only v1->v2 instrument-registry conversion/validation
    /// status probe. Loads `--registry`, converts to InstrumentRegistryV2 in
    /// memory, validates it, and prints a status report. No DB connection, no
    /// provider/broker calls, no writes. Exits nonzero on v2 validation failure.
    RegistryV2Status {
        /// Path to the v1 instrument registry JSON (e.g. config/instruments/equities.json).
        #[arg(long)]
        registry: PathBuf,
    },

    /// CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED:
    /// read-only CoinLore latest-mark evidence surface. Resolves CoinLore
    /// aliases for --symbols from a registry-v2 fixture, parses a ticker
    /// response (from --input-file by default; --input-file is required
    /// unless MQK_ALLOW_COINLORE_NETWORK_SMOKE=1 is set), and prints
    /// LatestMark values. Never opens a DB connection, never writes
    /// md_bars, never claims a completed OHLCV bar.
    CoinloreLatestMark {
        /// Path to a registry-v2 JSON file carrying provider_symbols.coinlore_id
        /// / provider_symbols.coinlore_symbol aliases (e.g.
        /// config/instruments/instruments_v2.crypto_local_marks.example.json).
        #[arg(long)]
        registry: PathBuf,

        /// Comma-separated canonical symbols (e.g. BTC/USD,ETH/USD).
        #[arg(long)]
        symbols: String,

        /// Path to a local file containing a CoinLore /api/ticker/?id=...
        /// response body (bare JSON array). When omitted, a live network
        /// call is attempted only if MQK_ALLOW_COINLORE_NETWORK_SMOKE=1.
        #[arg(long)]
        input_file: Option<PathBuf>,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Path to the provider registry JSON (used only to report
        /// provider_enabled in stdout/evidence; never gates parsing or the
        /// network call, which remain controlled solely by --input-file /
        /// MQK_ALLOW_COINLORE_NETWORK_SMOKE).
        #[arg(long, default_value = "config/providers/providers.json")]
        provider_registry: PathBuf,
    },

    /// CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-
    /// COMBINED: read-only Kraken OHLC parser evidence surface. Resolves the
    /// Kraken alias for --symbol from a registry-v2 fixture, parses a
    /// Kraken /0/public/OHLC response (from --input-file by default;
    /// --input-file is required unless MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1 is
    /// set), and prints completed-bar evidence. Never opens a DB
    /// connection, never writes md_bars, never claims the forming candle is
    /// complete. Only --timeframe 1D is supported.
    KrakenOhlcDryRun {
        /// Path to a registry-v2 JSON file carrying provider_symbols.kraken_pair
        /// (e.g. config/instruments/instruments_v2.crypto_local_marks.example.json).
        #[arg(long)]
        registry: PathBuf,

        /// Canonical symbol (e.g. BTC/USD or ETH/USD).
        #[arg(long)]
        symbol: String,

        /// Timeframe. Only 1D is supported by this adapter.
        #[arg(long, default_value = "1D")]
        timeframe: String,

        /// Path to a local file containing a Kraken /0/public/OHLC response
        /// body. When omitted, a live network call is attempted only if
        /// MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1.
        #[arg(long)]
        input_file: Option<PathBuf>,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED:
    /// fixture-first Kraken provider-ingest path. Resolves the Kraken alias
    /// for --symbol, parses a Kraken /0/public/OHLC response (from
    /// --input-file by default; --input-file is required unless
    /// MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1 is set), and ingests only the
    /// completed (non-forming) bars into md_bars with truthful
    /// provider_id="kraken" metadata. Only --timeframe 1D is supported. This
    /// is a single explicit operator invocation, not recurring ingestion:
    /// no scheduler, daemon, or GUI wiring is added by this command.
    KrakenOhlcIngest {
        /// Path to a registry-v2 JSON file carrying provider_symbols.kraken_pair
        /// (e.g. config/instruments/instruments_v2.crypto_local_marks.example.json).
        #[arg(long)]
        registry: PathBuf,

        /// Canonical symbol (e.g. BTC/USD or ETH/USD).
        #[arg(long)]
        symbol: String,

        /// Timeframe. Only 1D is supported by this adapter.
        #[arg(long, default_value = "1D")]
        timeframe: String,

        /// Path to a local file containing a Kraken /0/public/OHLC response
        /// body. When omitted, a live network call is attempted only if
        /// MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1.
        #[arg(long)]
        input_file: Option<PathBuf>,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED:
    /// content-diff-aware Kraken sync. Reads existing md_bars rows for the
    /// exact candidate end_ts keys, classifies each completed (non-forming)
    /// bar as missing/changed/unchanged by comparing OHLCV + is_complete +
    /// provider provenance, then upserts only missing and (unless
    /// --no-update-existing) changed bars via the same provider-metadata-
    /// aware helper `kraken-ohlc-ingest` uses, stamped
    /// `ingest_mode="provider_sync"` (distinct from `kraken-ohlc-ingest`'s
    /// `"provider_ingest"`) so DB rows can be traced to which command wrote
    /// them. Same fail-closed fixture/network gate as `kraken-ohlc-ingest`.
    /// Only --timeframe 1D is supported. Not recurring sync: no scheduler,
    /// daemon, or GUI wiring is added by this command.
    KrakenOhlcSync {
        /// Path to a registry-v2 JSON file carrying provider_symbols.kraken_pair
        /// (e.g. config/instruments/instruments_v2.crypto_local_marks.example.json).
        #[arg(long)]
        registry: PathBuf,

        /// Canonical symbol (e.g. BTC/USD or ETH/USD).
        #[arg(long)]
        symbol: String,

        /// Timeframe. Only 1D is supported by this adapter.
        #[arg(long, default_value = "1D")]
        timeframe: String,

        /// Path to a local file containing a Kraken /0/public/OHLC response
        /// body. When omitted, a live network call is attempted only if
        /// MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1 (manual operator smoke) or
        /// MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 (a future, separately-
        /// registered scheduled task); either also requires
        /// MQK_DATABASE_URL to already be set.
        #[arg(long)]
        input_file: Option<PathBuf>,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Conservative alternate policy: never write an existing row, even
        /// if its content is classified as changed (still reports it as
        /// rows_changed_skipped_due_to_no_update_existing, distinct from a
        /// truly-unchanged row). Missing/new rows are still inserted.
        /// Default (absent): missing rows are inserted, changed rows are
        /// updated, unchanged rows are always skipped.
        #[arg(long, default_value_t = false, action = clap::ArgAction::SetTrue)]
        no_update_existing: bool,
    },

    /// CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01:
    /// read-only operator readiness surface. Reads `--registry` and
    /// `--providers` (neither is ever mutated) and classifies whether the
    /// current, unmodified configs are ready for data-only `--provider`
    /// OHLCV operations for `--symbols`. `provider_enabled=false` and
    /// per-symbol `enabled=false` are expected, correct states here
    /// (`data_ready_manual_only`), not a failure. Never implies trading,
    /// scheduler, or production-cutover readiness. Never opens a DB
    /// connection, never calls a provider/network endpoint, never writes
    /// `md_bars`, never registers a scheduler.
    CryptoRegistryReadiness {
        /// Path to a registry-v2 JSON file carrying provider_symbols.kraken_pair
        /// / provider_symbols.kraken_result_key aliases (e.g.
        /// config/instruments/instruments_v2.crypto_local_marks.example.json).
        #[arg(long)]
        registry: PathBuf,

        /// Path to the provider registry JSON.
        #[arg(long, default_value = "config/providers/providers.json")]
        providers: PathBuf,

        /// Provider id to check readiness for.
        #[arg(long, default_value = "kraken")]
        provider: String,

        /// Comma-separated canonical symbols (e.g. BTC/USD,ETH/USD).
        #[arg(long, default_value = "BTC/USD,ETH/USD")]
        symbols: String,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01: read-only operator
    /// readiness surface proving whether a *future*, not-yet-authorized
    /// Kraken scheduled sync is currently allowed by the `CRYPTO-DATA-02A`
    /// rate-limit/cadence policy, the current provider/registry config, and
    /// (optionally) the latest Kraken OHLC evidence. `active` does not mean
    /// a scheduler is registered -- it means prerequisites are satisfied for
    /// a future, separately authorized registration patch to be considered.
    /// Never registers a Windows Scheduled Task, never adds a daemon job,
    /// never calls Kraken or any provider/network endpoint, never opens a
    /// DB connection, never mutates any config file.
    KrakenSchedulerReadiness {
        /// Path to the CRYPTO-DATA-02A policy JSON artifact.
        #[arg(
            long,
            default_value = "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json"
        )]
        policy: PathBuf,

        /// Path to a registry-v2 JSON file carrying provider_symbols.kraken_pair
        /// / provider_symbols.kraken_result_key aliases.
        #[arg(long)]
        registry: PathBuf,

        /// Path to the provider registry JSON.
        #[arg(long, default_value = "config/providers/providers.json")]
        providers: PathBuf,

        /// Provider id to check readiness for.
        #[arg(long, default_value = "kraken")]
        provider: String,

        /// Comma-separated canonical symbols (e.g. BTC/USD,ETH/USD).
        #[arg(long, default_value = "BTC/USD,ETH/USD")]
        symbols: String,

        /// Optional directory to inspect for the latest Kraken OHLC
        /// ingest/sync evidence file. When omitted, evidence is not checked
        /// at all (evidence_readiness_state=not_required).
        #[arg(long)]
        evidence_dir: Option<PathBuf>,

        /// When set, missing/unsafe/stale evidence in --evidence-dir fails
        /// closed (evidence_unsafe). When absent (default), missing/unsafe
        /// evidence is only a warning.
        #[arg(long, default_value_t = false, action = clap::ArgAction::SetTrue)]
        require_fresh_evidence: bool,

        /// Maximum evidence age (seconds) before it is considered stale,
        /// only enforced when --require-fresh-evidence is set.
        #[arg(long, default_value_t = 172_800)]
        evidence_max_age_secs: i64,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// REGISTRY-V2-TRANSLATION-01C: read-only proof that
    /// `RegistryV2SymbolTranslationIndex` (mqk-md) builds collision-free and
    /// round-trips for the current registry universe. Loads --registry-v1
    /// (v1 equities.json), converts it to InstrumentRegistryV2 in memory via
    /// the existing v1->v2 conversion, builds a translation index, and
    /// round-trip-checks every converted instrument. Optionally loads
    /// --registry-v2 (a separate v2 fixture, e.g. the disabled crypto local-
    /// marks fixture) and builds a second, independent translation index for
    /// it, reporting any enabled non-equity row as a failure. No DB
    /// connection, no provider/broker calls, no writes, no mutation of
    /// either input file. Evidence is written only when --output-dir is
    /// supplied.
    RegistryV2TranslationCheck {
        /// Path to the v1 instrument registry JSON (e.g. config/instruments/equities.json).
        #[arg(long)]
        registry_v1: Option<PathBuf>,

        /// Path to a standalone registry-v2 JSON fixture (e.g.
        /// config/instruments/instruments_v2.crypto_local_marks.example.json).
        /// Built and checked independently of --registry-v1; never merged
        /// with it.
        #[arg(long)]
        registry_v2: Option<PathBuf>,

        /// Directory to write a JSON evidence artifact. Not staged/committed.
        /// When omitted, no evidence file is written.
        #[arg(long)]
        output_dir: Option<PathBuf>,
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
            MdCmd::RegistryV2Status { registry } => {
                md_registry_v2_status(registry)?;
            }
            MdCmd::CoinloreLatestMark {
                registry,
                symbols,
                input_file,
                output_dir,
                provider_registry,
            } => {
                md_coinlore_latest_mark(
                    registry,
                    symbols,
                    input_file,
                    output_dir,
                    provider_registry,
                )
                .await?;
            }
            MdCmd::KrakenOhlcDryRun {
                registry,
                symbol,
                timeframe,
                input_file,
                output_dir,
            } => {
                md_kraken_ohlc_dry_run(registry, symbol, timeframe, input_file, output_dir)
                    .await?;
            }
            MdCmd::KrakenOhlcIngest {
                registry,
                symbol,
                timeframe,
                input_file,
                output_dir,
            } => {
                md_kraken_ohlc_ingest(registry, symbol, timeframe, input_file, output_dir)
                    .await?;
            }
            MdCmd::KrakenOhlcSync {
                registry,
                symbol,
                timeframe,
                input_file,
                output_dir,
                no_update_existing,
            } => {
                md_kraken_ohlc_sync(
                    registry,
                    symbol,
                    timeframe,
                    input_file,
                    output_dir,
                    no_update_existing,
                )
                .await?;
            }
            MdCmd::CryptoRegistryReadiness {
                registry,
                providers,
                provider,
                symbols,
                output_dir,
            } => {
                md_crypto_registry_readiness(registry, providers, provider, symbols, output_dir)?;
            }
            MdCmd::KrakenSchedulerReadiness {
                policy,
                registry,
                providers,
                provider,
                symbols,
                evidence_dir,
                require_fresh_evidence,
                evidence_max_age_secs,
                output_dir,
            } => {
                md_kraken_scheduler_readiness(
                    policy,
                    registry,
                    providers,
                    provider,
                    symbols,
                    evidence_dir,
                    require_fresh_evidence,
                    evidence_max_age_secs,
                    output_dir,
                )?;
            }
            MdCmd::RegistryV2TranslationCheck {
                registry_v1,
                registry_v2,
                output_dir,
            } => {
                md_registry_v2_translation_check(registry_v1, registry_v2, output_dir)?;
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
                contract_multiplier,
                initial_margin_micros,
                maintenance_margin_micros,
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
                    contract_multiplier,
                    initial_margin_micros,
                    maintenance_margin_micros,
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
                contract_multiplier,
                initial_margin_micros,
                maintenance_margin_micros,
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
                    contract_multiplier,
                    initial_margin_micros,
                    maintenance_margin_micros,
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
                contract_multiplier,
                initial_margin_micros,
                maintenance_margin_micros,
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
                    contract_multiplier,
                    initial_margin_micros,
                    maintenance_margin_micros,
                    out_dir,
                )
                .await?;
            }
            BacktestCmd::StrategyLabEvaluate { artifact_dir, json } => {
                run_strategy_lab_evaluate(artifact_dir, json)?;
            }
            BacktestCmd::StrategyLabRank {
                artifacts_root,
                top,
                json,
            } => {
                run_strategy_lab_rank(artifacts_root, top, json)?;
            }
            BacktestCmd::RegimeDetect {
                csv,
                symbol,
                timeframe,
                json,
            } => {
                run_regime_detect(csv, symbol, timeframe, json)?;
            }
            BacktestCmd::ScanStrategies {
                registry,
                bars_root,
                timeframe,
                strategy,
                top,
                limit_symbols,
                out_dir,
                dry_run,
                json,
            } => {
                run_strategy_scan(
                    registry,
                    bars_root,
                    timeframe,
                    strategy,
                    top,
                    limit_symbols,
                    out_dir,
                    dry_run,
                    json,
                )?;
            }
            BacktestCmd::ReviewScan {
                artifact_dir,
                out_dir,
                top,
                json,
            } => {
                run_review_scan(artifact_dir, out_dir, top, json)?;
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

        Commands::Autonomous { cmd } => match cmd {
            AutonomousCmd::NoTradeDiagnostics { limit } => {
                let pool = mqk_db::connect_from_env().await?;
                let rows = mqk_db::fetch_recent_autonomous_no_trade_diagnostics(&pool, limit).await?;
                if rows.is_empty() {
                    println!("truth_state=no_rows");
                } else {
                    println!("truth_state=active");
                    for r in &rows {
                        println!(
                            "diagnostic_id={} observed_at_utc={} run_id={} mode={} session_window_state={} runtime_start_allowed={} arm_state={} overall_ready={} reason_code={} stage={} paper_order_attempted={} live_order_attempted={} reason=\"{}\"",
                            r.diagnostic_id,
                            r.observed_at_utc.to_rfc3339(),
                            r.run_id
                                .map(|u: uuid::Uuid| u.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                            r.mode,
                            r.session_window_state,
                            r.runtime_start_allowed,
                            r.arm_state,
                            r.overall_ready,
                            r.reason_code,
                            r.stage,
                            r.paper_order_attempted,
                            r.live_order_attempted,
                            r.reason,
                        );
                    }
                }
            }
        },
    }

    Ok(())
}
