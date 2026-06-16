use std::fs;
use std::path::PathBuf;

use mqk_backtest::{StrategyLabDecision, StrategyLabReasonCode};

fn temp_artifact_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_strategy_lab_artifact_{}_{}",
        label,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp artifact dir");
    dir
}

fn write_manifest(dir: &PathBuf, strategy_name: &str) {
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "run_id": "00000000-0000-0000-0000-000000000001",
  "strategy_name": "{strategy_name}",
  "engine_id": "mqk-backtest",
  "mode": "backtest",
  "git_hash": "test",
  "config_hash": "cfg",
  "host_fingerprint": "test-host",
  "created_at_utc": "2023-11-14T22:13:20Z",
  "artifacts": {{
    "audit_jsonl": "audit.jsonl",
    "manifest_json": "manifest.json",
    "orders_csv": "orders.csv",
    "fills_csv": "fills.csv",
    "equity_curve_csv": "equity_curve.csv",
    "metrics_json": "metrics.json"
  }}
}}
"#
        ),
    )
    .expect("write manifest");
}

#[test]
fn strategy_lab_artifact_valid_folder_evaluates_successfully() {
    let dir = temp_artifact_dir("valid");
    write_manifest(&dir, "manifest_strategy");
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "schema_version": 1,
  "symbols": ["SPY"],
  "timeframe": "1m",
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
  },
  "ignored_future_field": "ignored"
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate artifact");

    assert_eq!(evaluation.strategy_id, "manifest_strategy");
    assert_eq!(evaluation.symbol, "SPY");
    assert_eq!(evaluation.timeframe, "1m");
    assert_eq!(evaluation.decision, StrategyLabDecision::ResearchPass);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::ResearchPassMinimumsMet));
}

#[test]
fn strategy_lab_artifact_missing_metrics_json_fails_truthfully() {
    let dir = temp_artifact_dir("missing_metrics");
    write_manifest(&dir, "manifest_strategy");

    let err = mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir)
        .expect_err("missing metrics.json must fail");

    let message = format!("{err:#}");
    assert!(
        message.contains("read metrics.json failed"),
        "unexpected error: {message}"
    );
}

#[test]
fn strategy_lab_artifact_malformed_metrics_json_fails_truthfully() {
    let dir = temp_artifact_dir("malformed_metrics");
    fs::write(dir.join("metrics.json"), "{ not valid json").expect("write metrics");

    let err = mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir)
        .expect_err("malformed metrics.json must fail");

    let message = format!("{err:#}");
    assert!(
        message.contains("parse metrics.json failed"),
        "unexpected error: {message}"
    );
}

#[test]
fn strategy_lab_artifact_missing_optional_metrics_does_not_crash() {
    let dir = temp_artifact_dir("missing_optional");
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "strategy_name": "partial_strategy",
  "symbols": ["SPY"],
  "total_return_pct": 10.0,
  "max_drawdown_pct": 10.0,
  "trade_count": 12
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate partial artifact");

    assert_eq!(evaluation.strategy_id, "partial_strategy");
    assert_eq!(evaluation.symbol, "SPY");
    assert_eq!(evaluation.timeframe, "unknown");
    assert_ne!(evaluation.decision, StrategyLabDecision::InsufficientData);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::WinRateMissing));
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::ExpectancyMissing));
}

#[test]
fn strategy_lab_artifact_missing_required_metrics_returns_insufficient_data() {
    let dir = temp_artifact_dir("missing_required");
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "strategy_name": "incomplete_strategy",
  "symbols": ["SPY"],
  "win_rate_pct": 50.0
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate artifact");

    assert_eq!(evaluation.decision, StrategyLabDecision::InsufficientData);
    assert!(evaluation
        .reason_codes
        .contains(&StrategyLabReasonCode::MetricsMissing));
}
