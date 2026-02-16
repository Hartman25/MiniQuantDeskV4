use anyhow::Result;
use clap::{Parser, Subcommand};
use chrono::Utc;
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
    }

    Ok(())
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
