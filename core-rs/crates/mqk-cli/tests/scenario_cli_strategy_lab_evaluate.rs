use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

fn temp_artifact_dir(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mqk_cli_strategy_lab_{}_{}", label, Uuid::new_v4()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp artifact dir");
    dir
}

fn write_manifest(dir: &Path) {
    fs::write(
        dir.join("manifest.json"),
        r#"{
  "schema_version": 1,
  "run_id": "00000000-0000-0000-0000-000000000001",
  "strategy_name": "manifest_strategy",
  "engine_id": "mqk-backtest",
  "mode": "backtest",
  "timeframe_secs": 300,
  "git_hash": "test",
  "config_hash": "cfg",
  "host_fingerprint": "test-host",
  "created_at_utc": "2023-11-14T22:13:20Z",
  "artifacts": {
    "audit_jsonl": "audit.jsonl",
    "manifest_json": "manifest.json",
    "orders_csv": "orders.csv",
    "fills_csv": "fills.csv",
    "equity_curve_csv": "equity_curve.csv",
    "metrics_json": "metrics.json"
  }
}
"#,
    )
    .expect("write manifest");
}

fn write_valid_metrics(dir: &Path) {
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "schema_version": 1,
  "symbols": ["SPY"],
  "total_return_pct": 40.0,
  "max_drawdown_pct": 3.0,
  "trade_count": 20,
  "win_rate_pct": 65.0,
  "profit_factor": 2.4,
  "expectancy": 1.0,
  "trade_frequency": 3.5,
  "exposure_time_pct": 45.0,
  "sharpe_ratio": 1.2,
  "benchmark": {
    "buy_and_hold_return_pct": 5.0,
    "alpha_pct": 35.0
  }
}
"#,
    )
    .expect("write metrics");
}

fn write_rank_metrics(dir: &Path, strategy_name: &str, symbol: &str, total_return_pct: f64) {
    fs::write(
        dir.join("metrics.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "strategy_name": "{strategy_name}",
  "symbol": "{symbol}",
  "timeframe": "1m",
  "total_return_pct": {total_return_pct},
  "max_drawdown_pct": 3.0,
  "trade_count": 20,
  "win_rate_pct": 65.0,
  "profit_factor": 2.4,
  "expectancy": 1.0,
  "exposure_time_pct": 45.0,
  "sharpe_ratio": 1.2,
  "benchmark": {{
    "buy_and_hold_return_pct": 5.0,
    "alpha_pct": 35.0
  }}
}}
"#
        ),
    )
    .expect("write metrics");
}

#[test]
fn strategy_lab_evaluate_valid_artifact_prints_report() -> anyhow::Result<()> {
    let dir = temp_artifact_dir("valid");
    write_manifest(&dir);
    write_valid_metrics(&dir);
    let dir_s = dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "strategy-lab-evaluate",
            "--artifact-dir",
            &dir_s,
        ])
        .output()?;

    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;

    assert!(stdout.contains("strategy_id=manifest_strategy"));
    assert!(stdout.contains("symbol=SPY"));
    assert!(stdout.contains("timeframe=5m"));
    assert!(stdout.contains("decision=research_pass"));
    assert!(stdout.contains("reason_codes=research_pass_minimums_met"));

    Ok(())
}

#[test]
fn strategy_lab_rank_valid_root_prints_deterministic_report() -> anyhow::Result<()> {
    let root = temp_artifact_dir("rank_valid");
    let strong = root.join("strong");
    let weak = root.join("weak");
    fs::create_dir_all(&strong).expect("create strong artifact");
    fs::create_dir_all(&weak).expect("create weak artifact");
    write_rank_metrics(&strong, "strong_strategy", "AAPL", 40.0);
    write_rank_metrics(&weak, "weak_strategy", "MSFT", 15.0);
    let root_s = root.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "strategy-lab-rank",
            "--artifacts-root",
            &root_s,
            "--top",
            "1",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;

    assert!(stdout.contains("candidates_scanned=2"));
    assert!(stdout.contains("evaluations_count=2"));
    assert!(stdout.contains("failures_count=0"));
    assert!(stdout.contains("rank=1"));
    assert!(stdout.contains("strategy_id=strong_strategy"));
    assert!(stdout.contains("symbol=AAPL"));
    assert!(stdout.contains("decision=research_pass"));
    assert!(!stdout.contains("strategy_id=weak_strategy"));

    Ok(())
}

#[test]
fn strategy_lab_rank_json_is_parseable() -> anyhow::Result<()> {
    let root = temp_artifact_dir("rank_json");
    let artifact = root.join("artifact");
    fs::create_dir_all(&artifact).expect("create artifact");
    write_rank_metrics(&artifact, "json_strategy", "SPY", 30.0);
    let root_s = root.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "strategy-lab-rank",
            "--artifacts-root",
            &root_s,
            "--json",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert_eq!(value["candidates_scanned"], 1);
    assert_eq!(value["evaluations_count"], 1);
    assert_eq!(value["failures_count"], 0);
    assert_eq!(value["ranked"][0]["strategy_id"], "json_strategy");
    assert_eq!(value["ranked"][0]["decision"], "research_pass");

    Ok(())
}

#[test]
fn strategy_lab_rank_missing_root_fails_truthfully() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "mqk_cli_strategy_lab_rank_missing_{}",
        Uuid::new_v4()
    ));
    let dir_s = dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(["backtest", "strategy-lab-rank", "--artifacts-root", &dir_s])
        .output()?;

    assert!(!output.status.success(), "missing root must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("artifact root does not exist or is not a directory"),
        "unexpected stderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn strategy_lab_rank_mixed_root_reports_evaluated_and_failed_counts() -> anyhow::Result<()> {
    let root = temp_artifact_dir("rank_mixed");
    let valid = root.join("valid");
    let malformed = root.join("malformed");
    fs::create_dir_all(&valid).expect("create valid artifact");
    fs::create_dir_all(&malformed).expect("create malformed artifact");
    write_rank_metrics(&valid, "valid_strategy", "SPY", 35.0);
    fs::write(malformed.join("metrics.json"), "{ not valid json").expect("write malformed");
    let root_s = root.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(["backtest", "strategy-lab-rank", "--artifacts-root", &root_s])
        .output()?;

    assert!(
        output.status.success(),
        "mixed valid/malformed root should report both without failing\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;

    assert!(stdout.contains("candidates_scanned=2"));
    assert!(stdout.contains("evaluations_count=1"));
    assert!(stdout.contains("failures_count=1"));
    assert!(stdout.contains("strategy_id=valid_strategy"));
    assert!(stdout.contains("failure_artifact_path="));
    assert!(stdout.contains("parse metrics.json failed"));

    Ok(())
}

#[test]
fn strategy_lab_evaluate_missing_artifact_folder_fails_truthfully() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("mqk_cli_strategy_lab_missing_{}", Uuid::new_v4()));
    let dir_s = dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "strategy-lab-evaluate",
            "--artifact-dir",
            &dir_s,
        ])
        .output()?;

    assert!(!output.status.success(), "missing folder must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("artifact directory does not exist or is not a directory"),
        "unexpected stderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn strategy_lab_evaluate_malformed_metrics_fails_truthfully() -> anyhow::Result<()> {
    let dir = temp_artifact_dir("malformed");
    write_manifest(&dir);
    fs::write(dir.join("metrics.json"), "{ not valid json").expect("write malformed metrics");
    let dir_s = dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "strategy-lab-evaluate",
            "--artifact-dir",
            &dir_s,
        ])
        .output()?;

    assert!(!output.status.success(), "malformed metrics must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parse metrics.json failed"),
        "unexpected stderr:\n{stderr}"
    );

    Ok(())
}
