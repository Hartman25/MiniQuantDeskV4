use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use chrono::Utc;
use serde_json::Value;
use std::fs;
use std::path::Path;
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

        /// Mode (PAPER | LIVE)
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
                DbCmd::Migrate { yes } => {
                    // Guardrail: refuse migrations if there is any LIVE run in ARMED/RUNNING
                    // unless the operator explicitly acknowledges with --yes.
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

        Commands::Run { cmd } => match cmd {
            RunCmd::Start {
                engine,
                mode,
                config_paths,
            } => {
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

                // Initialize run artifacts directory + manifest + placeholders.
                // NOTE: CLI is run from core-rs/, so exports root is ../exports.
                let exports_root = Path::new("../exports");
                let _art = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
                    exports_root,
                    schema_version: 1,
                    run_id,
                    engine_id: &engine,
                    mode: &mode,
                    git_hash: &git_hash,
                    config_hash: &loaded.config_hash,
                    host_fingerprint: &host_fp,
                })?;

                println!("run_id={}", run_id);
                println!("engine_id={}", engine);
                println!("mode={}", mode);
                println!("git_hash={}", git_hash);
                println!("config_hash={}", loaded.config_hash);
                println!("host_fingerprint={}", host_fp);
            }

            RunCmd::Arm { run_id, confirm } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;

                // Fetch run so we can enforce LIVE confirmation rules using stored config_json.
                let r = mqk_db::fetch_run(&pool, run_uuid).await?;
                enforce_manual_confirmation_if_required(&r, confirm.as_deref())?;

                mqk_db::arm_run(&pool, run_uuid).await?;
                println!("armed=true run_id={} status=ARMED", run_uuid);
            }

            RunCmd::Begin { run_id } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                mqk_db::begin_run(&pool, run_uuid).await?;
                println!("begun=true run_id={} status=RUNNING", run_uuid);
            }

            RunCmd::Stop { run_id } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                mqk_db::stop_run(&pool, run_uuid).await?;
                println!("stopped=true run_id={} status=STOPPED", run_uuid);
            }

            RunCmd::Halt { run_id, reason } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                mqk_db::halt_run(&pool, run_uuid).await?;
                println!(
                    "halted=true run_id={} status=HALTED reason={}",
                    run_uuid, reason
                );
            }

            RunCmd::Heartbeat { run_id } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                mqk_db::heartbeat_run(&pool, run_uuid).await?;
                println!("heartbeat=true run_id={}", run_uuid);
            }

            RunCmd::Status { run_id } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                let r = mqk_db::fetch_run(&pool, run_uuid).await?;
                println!("run_id={}", r.run_id);
                println!("engine_id={}", r.engine_id);
                println!("mode={}", r.mode);
                println!("status={}", r.status.as_str());
                println!("started_at_utc={}", r.started_at_utc.to_rfc3339());
                println!("armed_at_utc={}", opt_dt(&r.armed_at_utc));
                println!("running_at_utc={}", opt_dt(&r.running_at_utc));
                println!("stopped_at_utc={}", opt_dt(&r.stopped_at_utc));
                println!("halted_at_utc={}", opt_dt(&r.halted_at_utc));
                println!("last_heartbeat_utc={}", opt_dt(&r.last_heartbeat_utc));
                println!("git_hash={}", r.git_hash);
                println!("config_hash={}", r.config_hash);
                println!("host_fingerprint={}", r.host_fingerprint);
            }

            RunCmd::DeadmanCheck { run_id, ttl_seconds } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                let expired = mqk_db::deadman_expired(&pool, run_uuid, ttl_seconds).await?;
                println!(
                    "deadman_expired={} run_id={} ttl_seconds={}",
                    expired, run_uuid, ttl_seconds
                );
            }

            RunCmd::DeadmanEnforce { run_id, ttl_seconds } => {
                let pool = mqk_db::connect_from_env().await?;
                let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
                let halted =
                    mqk_db::enforce_deadman_or_halt(&pool, run_uuid, ttl_seconds).await?;
                println!(
                    "deadman_halted={} run_id={} ttl_seconds={}",
                    halted, run_uuid, ttl_seconds
                );
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

fn opt_dt(dt: &Option<chrono::DateTime<Utc>>) -> String {
    dt.as_ref()
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "".to_string())
}

/// Enforce manual confirmation for LIVE arming when configured.
///
/// NOTE: This is intentionally a CLI-layer gate. Phase-1 DB schema does not store
/// operator confirmations, and `mqk_db::arm_run()` does not enforce it.
fn enforce_manual_confirmation_if_required(
    run: &mqk_db::RunRow,
    confirm: Option<&str>,
) -> Result<()> {
    if run.mode.to_uppercase() != "LIVE" {
        return Ok(());
    }

    // Default: require confirmation unless explicitly disabled in config.
    let require = run
        .config_json
        .pointer("/arming/require_manual_confirmation")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    if !require {
        return Ok(());
    }

    let fmt = run
        .config_json
        .pointer("/arming/confirmation_format")
        .and_then(|v| v.as_str())
        .unwrap_or("ARM LIVE {account_last4} {daily_loss_limit}");

    let account_last4 = run
        .config_json
        .pointer("/broker/account_last4")
        .and_then(|v| v.as_str())
        .unwrap_or("0000");

    let daily_loss_limit = run
        .config_json
        .pointer("/risk/daily_loss_limit")
        .map(|v| match v {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => "".to_string(),
        })
        .unwrap_or_else(|| "".to_string());

    let expected = fmt
        .replace("{account_last4}", account_last4)
        .replace("{daily_loss_limit}", daily_loss_limit.trim());

    let confirm = confirm
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "manual confirmation required for LIVE arming. expected: \"{}\" (use --confirm)",
                expected
            )
        })?;

    if confirm != expected {
        return Err(anyhow::anyhow!(
            "manual confirmation mismatch. expected: \"{}\" got: \"{}\"",
            expected,
            confirm
        ));
    }

    Ok(())
}
