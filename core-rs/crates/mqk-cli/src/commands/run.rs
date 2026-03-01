//! Run-lifecycle command handlers.
//!
//! Covers all subcommands of `mqk run`: start, arm, begin, stop, halt,
//! heartbeat, status, deadman-check, deadman-enforce, and loop.

use anyhow::{Context, Result};
use chrono::Utc;
use mqk_config::{report_unused_keys, UnusedKeyPolicy};
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorRunMeta};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use super::parse_config_mode;

// ---------------------------------------------------------------------------
// run start
// ---------------------------------------------------------------------------

pub async fn run_start(engine: String, mode: String, config_paths: Vec<String>) -> Result<()> {
    let path_refs: Vec<&str> = config_paths.iter().map(|s| s.as_str()).collect();
    let loaded = mqk_config::load_layered_yaml(&path_refs)?;

    let cfg_mode = parse_config_mode(&mode)?;
    let policy = match cfg_mode {
        mqk_config::ConfigMode::Live => UnusedKeyPolicy::Fail,
        mqk_config::ConfigMode::Paper | mqk_config::ConfigMode::Backtest => UnusedKeyPolicy::Warn,
    };

    let report = report_unused_keys(cfg_mode, &loaded.config_json, policy)?;
    if !report.is_clean() {
        eprintln!(
            "WARN: CONFIG_UNUSED_KEYS mode={} unused_leaf_keys={}",
            mode.to_uppercase(),
            report.unused_leaf_pointers.len()
        );
        for p in report.unused_leaf_pointers.iter().take(50) {
            eprintln!("  unused={}", p);
        }
        let extra = report.unused_leaf_pointers.len().saturating_sub(50);
        if extra > 0 {
            eprintln!("  ... and {} more", extra);
        }
    }

    let pool = mqk_db::connect_from_env().await?;

    let git_hash = get_git_hash().unwrap_or_else(|| "UNKNOWN".to_string());
    let run_id = derive_cli_run_id(&engine, &mode, &loaded.config_hash, &git_hash);
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

    Ok(())
}

// ---------------------------------------------------------------------------
// run arm
// ---------------------------------------------------------------------------

pub async fn run_arm(run_id: String, confirm: Option<String>) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;

    let r = mqk_db::fetch_run(&pool, run_uuid).await?;
    enforce_manual_confirmation_if_required(&r, confirm.as_deref())?;

    mqk_db::arm_preflight(&pool, run_uuid).await?;
    println!("armed=true run_id={} status=ARMED", run_uuid);

    Ok(())
}

// ---------------------------------------------------------------------------
// run begin
// ---------------------------------------------------------------------------

pub async fn run_begin(run_id: String) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    mqk_db::begin_run(&pool, run_uuid).await?;
    println!("begun=true run_id={} status=RUNNING", run_uuid);
    Ok(())
}

// ---------------------------------------------------------------------------
// run stop
// ---------------------------------------------------------------------------

pub async fn run_stop(run_id: String) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    mqk_db::stop_run(&pool, run_uuid).await?;
    println!("stopped=true run_id={} status=STOPPED", run_uuid);
    Ok(())
}

// ---------------------------------------------------------------------------
// run halt
// ---------------------------------------------------------------------------

pub async fn run_halt(run_id: String, reason: String) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    mqk_db::halt_run(&pool, run_uuid).await?;
    println!(
        "halted=true run_id={} status=HALTED reason={}",
        run_uuid, reason
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// run heartbeat
// ---------------------------------------------------------------------------

pub async fn run_heartbeat(run_id: String) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    mqk_db::heartbeat_run(&pool, run_uuid).await?;
    println!("heartbeat=true run_id={}", run_uuid);
    Ok(())
}

// ---------------------------------------------------------------------------
// run status
// ---------------------------------------------------------------------------

pub async fn run_status(run_id: String) -> Result<()> {
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
    Ok(())
}

// ---------------------------------------------------------------------------
// run deadman-check
// ---------------------------------------------------------------------------

pub async fn run_deadman_check(run_id: String, ttl_seconds: i64) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    let expired = mqk_db::deadman_expired(&pool, run_uuid, ttl_seconds, Utc::now()).await?;
    println!(
        "deadman_expired={} run_id={} ttl_seconds={}",
        expired, run_uuid, ttl_seconds
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// run deadman-enforce
// ---------------------------------------------------------------------------

pub async fn run_deadman_enforce(run_id: String, ttl_seconds: i64) -> Result<()> {
    let pool = mqk_db::connect_from_env().await?;
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;
    let halted = mqk_db::enforce_deadman_or_halt(&pool, run_uuid, ttl_seconds, Utc::now()).await?;
    println!(
        "deadman_halted={} run_id={} ttl_seconds={}",
        halted, run_uuid, ttl_seconds
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// run loop
// ---------------------------------------------------------------------------

pub fn run_loop(
    run_id: String,
    symbol: String,
    bars: usize,
    timeframe_secs: u64,
    exports_root: PathBuf,
    label: String,
) -> Result<()> {
    let run_uuid = Uuid::parse_str(&run_id).context("invalid run_id uuid")?;

    let mut cfg = OrchestratorConfig::test_defaults();
    cfg.timeframe_secs = timeframe_secs as i64;
    cfg.max_bars = bars;

    let meta = OrchestratorRunMeta {
        run_id: run_uuid,
        engine_id: "CLI".to_string(),
        mode: "BACKTEST".to_string(),
    };

    let mut orch = Orchestrator::new_with_meta(cfg, meta);

    let mut generated: Vec<OrchestratorBar> = Vec::with_capacity(bars);
    for i in 0..bars {
        let ts: u64 = 1_700_000_000u64 + (i as u64) * timeframe_secs;
        let price: i64 = 100_000_000i64 + (i as i64) * 100_000;

        generated.push(OrchestratorBar {
            symbol: symbol.clone(),
            end_ts: ts,
            open_micros: price,
            high_micros: price + 50_000,
            low_micros: price - 50_000,
            close_micros: price,
            volume: 1_000i64,
            day_id: (ts / 86_400) as u32,
        });
    }

    // currently unused (kept for CLI arg compatibility)
    let _ = exports_root;
    let _ = label;

    let report = orch.run(&generated).context("orchestrator run")?;

    println!("symbol={}", report.symbol);
    println!("bars_seen={}", report.bars_seen);
    println!("last_end_ts={:?}", report.last_end_ts);
    println!("last_close_micros={:?}", report.last_close_micros);

    Ok(())
}

// ---------------------------------------------------------------------------
// Run-ID derivation (D1-1)
// ---------------------------------------------------------------------------

/// Derive a deterministic run ID from the combination of engine identity and
/// loaded configuration. All inputs are caller-provided with no wall-clock
/// or RNG dependency.
///
/// **No RNG.** Uses `Uuid::new_v5` (SHA-1 over the DNS namespace).
///
/// Inputs:
///   `engine_id`   — engine identifier string (e.g. `"swing_v1"`)
///   `mode`        — `"live"` | `"paper"` | `"backtest"`
///   `config_hash` — SHA-256 hex of the loaded config JSON (from `mqk_config`)
///   `git_hash`    — short git commit hash of the running binary
///
/// Omitted from this version (reserved for future patches):
///   `asof_utc`      — wall-clock at run start; requires a `TimeSource`
///                     abstraction (D1-3) before it can be made deterministic.
///   `universe_hash` — symbol-set hash; not yet defined in schema.
///
/// The derivation prefix `"mqk-cli.run.v1"` scopes the hash within the DNS
/// namespace, preventing collisions with any other UUIDv5 uses in the system.
fn derive_cli_run_id(engine_id: &str, mode: &str, config_hash: &str, git_hash: &str) -> Uuid {
    let data = format!(
        "mqk-cli.run.v1|{}|{}|{}|{}",
        engine_id, mode, config_hash, git_hash
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, data.as_bytes())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn enforce_manual_confirmation_if_required(
    run: &mqk_db::RunRow,
    confirm: Option<&str>,
) -> Result<()> {
    if run.mode.to_uppercase() != "LIVE" {
        return Ok(());
    }

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
        .unwrap_or_default();

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

fn host_fingerprint() -> String {
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("{hostname}|{username}|{os}|{arch}")
}

fn opt_dt(dt: &Option<chrono::DateTime<Utc>>) -> String {
    dt.as_ref().map(|d| d.to_rfc3339()).unwrap_or_default()
}
