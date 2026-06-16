use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

fn temp_artifact_dir(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mqk_cli_strategy_lab_{}_{}", label, Uuid::new_v4()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp artifact dir");
    dir
}

fn write_manifest(dir: &PathBuf) {
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

fn write_valid_metrics(dir: &PathBuf) {
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
