use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String,
    pub git_hash: String,
    pub config_hash: String,
    pub host_fingerprint: String,
    pub created_at_utc: DateTime<Utc>,
    pub artifacts: ArtifactList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactList {
    pub audit_jsonl: String,
    pub manifest_json: String,
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
}

pub struct InitRunArtifactsArgs<'a> {
    pub exports_root: &'a Path, // e.g. ../exports
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: &'a str,
    pub mode: &'a str,
    pub git_hash: &'a str,
    pub config_hash: &'a str,
    pub host_fingerprint: &'a str,
}

pub struct InitRunArtifactsResult {
    pub run_dir: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn init_run_artifacts(args: InitRunArtifactsArgs<'_>) -> Result<InitRunArtifactsResult> {
    // exports/<run_id>/
    let run_dir = args.exports_root.join(args.run_id.to_string());
    fs::create_dir_all(&run_dir).with_context(|| format!("create exports dir failed: {}", run_dir.display()))?;

    // Create placeholder files if missing (do not overwrite existing).
    ensure_file_exists_with(&run_dir.join("audit.jsonl"), "")?;
    ensure_file_exists_with(&run_dir.join("orders.csv"), "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n")?;
    ensure_file_exists_with(&run_dir.join("fills.csv"), "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n")?;
    ensure_file_exists_with(&run_dir.join("equity_curve.csv"), "ts_utc,equity\n")?;
    ensure_file_exists_with(&run_dir.join("metrics.json"), "{}\n")?;

    // Write manifest.json (overwrite is OK; it’s deterministic for a run start).
    let manifest = RunManifest {
        schema_version: args.schema_version,
        run_id: args.run_id,
        engine_id: args.engine_id.to_string(),
        mode: args.mode.to_string(),
        git_hash: args.git_hash.to_string(),
        config_hash: args.config_hash.to_string(),
        host_fingerprint: args.host_fingerprint.to_string(),
        created_at_utc: Utc::now(),
        artifacts: ArtifactList {
            audit_jsonl: "audit.jsonl".to_string(),
            manifest_json: "manifest.json".to_string(),
            orders_csv: "orders.csv".to_string(),
            fills_csv: "fills.csv".to_string(),
            equity_curve_csv: "equity_curve.csv".to_string(),
            metrics_json: "metrics.json".to_string(),
        },
    };

    let manifest_path = run_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest).context("serialize manifest failed")?;
    fs::write(&manifest_path, format!("{json}\n")).with_context(|| format!("write manifest failed: {}", manifest_path.display()))?;

    Ok(InitRunArtifactsResult { run_dir, manifest_path })
}

fn ensure_file_exists_with(path: &Path, contents_if_create: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents_if_create).with_context(|| format!("create placeholder failed: {}", path.display()))?;
    Ok(())
}
