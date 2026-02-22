//! Command handler modules for mqk-cli.
//!
//! Shared utilities used by multiple command paths live here.
//! Command-specific logic lives in the submodules.

pub mod backtest;
pub mod run;

use anyhow::{Context, Result};
use mqk_config::ConfigMode;
use serde_json::Value;
use std::fs;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse a CLI `--mode` string into a [`ConfigMode`].
pub fn parse_config_mode(mode: &str) -> Result<ConfigMode> {
    match mode.trim().to_uppercase().as_str() {
        "BACKTEST" => Ok(ConfigMode::Backtest),
        "PAPER" => Ok(ConfigMode::Paper),
        "LIVE" => Ok(ConfigMode::Live),
        other => anyhow::bail!(
            "invalid --mode '{}'. expected one of: BACKTEST | PAPER | LIVE",
            other
        ),
    }
}

/// Load an audit payload from either an inline JSON string or a file path.
pub fn load_payload(payload: Option<String>, payload_file: Option<String>) -> Result<Value> {
    if let Some(p) = payload_file {
        let bytes = fs::read(&p).with_context(|| format!("read payload-file failed: {}", p))?;
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
