use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

use commands::{
    backtest::{md_ingest_csv, md_ingest_provider},
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
                timeframe,
                start,
                end,
            } => {
                md_ingest_provider(source, symbols, timeframe, start, end).await?;
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
