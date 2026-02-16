use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use chrono::Utc;
use serde_json::Value;
use std::fs;
use std::process::Command;
use uuid::Uuid;

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
}

#[derive(Subcommand)]
enum DbCmd {
    Status,
    Migrate,
}

#[derive(Subcommand)]
enum RunCmd {
    /// Create a new run row in DB and print run_id + hashes.
    Start {
        /// Engine ID (e.g. MAIN, EXP)
        #[arg(long)]
        engine: String,

        /// Mode (PAPER | LIVE)
        #[arg(long)]
        mode: String,

        /// Layered config paths in merge order
        #[arg(long = "config", required = true)]
        config_paths: Vec<String>,
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
                DbCmd::Migrate => {
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

        Commands::Run { cmd } => match cmd {
            RunCmd::Start { engine, mode, config_paths } => {
                let pool = mqk_db::connect_from_env().await?;

                let path_refs: Vec<&str> = config_paths.iter().map(|s| s.as_str()).collect();
                let loaded = mqk_config::load_layered_yaml(&path_refs)?;

                let run_id = Uuid::new_v4();
                let git_hash = get_git_hash().unwrap_or_else(|| "UNKNOWN".to_string());
                let host_fp = host_fingerprint();

                let new_run = mqk_db::NewRun {
                    run_id,
                    engine_id: engine.clone(),
                    mode: mode.clone(),
                    started_at_utc: Utc::now(),
                    git_hash: git_hash.clone(),
                    config_hash: loaded.config_hash.clone(),
                    config_json: loaded.config_json.clone(),
                    host_fingerprint: host_fp.clone(),
                };

                mqk_db::insert_run(&pool, &new_run).await?;

                println!("run_id={}", run_id);
                println!("engine_id={}", engine);
                println!("mode={}", mode);
                println!("git_hash={}", git_hash);
                println!("config_hash={}", loaded.config_hash);
                println!("host_fingerprint={}", host_fp);
            }
        },

        Commands::Audit { cmd } => match cmd {
            AuditCmd::Emit { run_id, topic, event_type, payload, payload_file, hash_chain, .. } => {
                let pool = mqk_db::connect_from_env().await?;

                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                let payload_json: Value = load_payload(payload, payload_file)?;

                // JSONL path: exports/<run_id>/audit.jsonl (repo-root exports folder)
                let path = format!("../exports/{}/audit.jsonl", run_id);
                let mut writer = mqk_audit::AuditWriter::new(&path, hash_chain)?;

                // Append to file (generates event_id + ts + optional hashes)
                let ev = writer.append(run_uuid, &topic, &event_type, payload_json)?;

                // Insert to DB
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

fn load_payload(payload: Option<String>, payload_file: Option<String>) -> Result<Value> {
    if let Some(p) = payload_file {
        // Read raw bytes to handle UTF-8 BOM cleanly on Windows.
        let bytes = fs::read(&p).with_context(|| format!("read payload-file failed: {}", p))?;

        // Strip UTF-8 BOM if present.
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);

        let raw = String::from_utf8(bytes.to_vec()).context("payload-file must be UTF-8 text")?;
        let raw = raw.trim();

        let v: Value = serde_json::from_str(raw).context("payload-file must contain valid JSON")?;
        return Ok(v);
    }

    let raw = payload.context("must provide --payload or --payload-file")?;
    let raw = raw.trim();

    let v: Value = serde_json::from_str(raw).context("--payload must be valid JSON")?;
    Ok(v)
}

/// Best-effort git hash (short).
fn get_git_hash() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

/// Stable-ish, non-sensitive host fingerprint for run attribution.
/// This is *not* a hardware id. It’s just enough to distinguish machines in logs.
fn host_fingerprint() -> String {
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("{hostname}|{username}|{os}|{arch}")
}
