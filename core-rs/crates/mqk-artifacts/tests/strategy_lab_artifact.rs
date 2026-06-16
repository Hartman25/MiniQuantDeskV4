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
    write_manifest_with_metadata(dir, strategy_name, None, None);
}

fn write_rank_metrics(
    dir: &PathBuf,
    strategy_name: &str,
    symbol: &str,
    timeframe: &str,
    total_return_pct: f64,
) {
    fs::write(
        dir.join("metrics.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "strategy_name": "{strategy_name}",
  "symbol": "{symbol}",
  "timeframe": "{timeframe}",
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

fn write_manifest_with_metadata(
    dir: &PathBuf,
    strategy_name: &str,
    timeframe: Option<&str>,
    timeframe_secs: Option<i64>,
) {
    let timeframe_json = timeframe
        .map(|value| format!(r#"  "timeframe": "{value}","#))
        .unwrap_or_default();
    let timeframe_secs_json = timeframe_secs
        .map(|value| format!(r#"  "timeframe_secs": {value},"#))
        .unwrap_or_default();

    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "run_id": "00000000-0000-0000-0000-000000000001",
  "strategy_name": "{strategy_name}",
  "engine_id": "mqk-backtest",
  "mode": "backtest",
{timeframe_json}
{timeframe_secs_json}
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
fn strategy_lab_artifact_manifest_timeframe_is_used() {
    let dir = temp_artifact_dir("manifest_timeframe");
    write_manifest_with_metadata(&dir, "manifest_strategy", Some("5m"), None);
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "schema_version": 1,
  "symbols": ["SPY"],
  "timeframe": "1m",
  "total_return_pct": 8.0,
  "max_drawdown_pct": 4.0,
  "trade_count": 11
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate artifact");

    assert_eq!(evaluation.timeframe, "5m");
}

#[test]
fn strategy_lab_artifact_manifest_timeframe_secs_is_mapped() {
    let dir = temp_artifact_dir("manifest_timeframe_secs");
    write_manifest_with_metadata(&dir, "manifest_strategy", None, Some(300));
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "schema_version": 1,
  "symbols": ["SPY"],
  "total_return_pct": 8.0,
  "max_drawdown_pct": 4.0,
  "trade_count": 11
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate artifact");

    assert_eq!(evaluation.timeframe, "5m");
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
fn strategy_lab_artifact_old_shape_without_timeframe_evaluates_unknown() {
    let dir = temp_artifact_dir("old_shape_unknown_timeframe");
    fs::write(
        dir.join("metrics.json"),
        r#"{
  "strategy_name": "old_strategy",
  "symbol": "SPY",
  "total_return_pct": 12.0,
  "max_drawdown_pct": 6.0,
  "trade_count": 15
}
"#,
    )
    .expect("write metrics");

    let evaluation =
        mqk_artifacts::evaluate_strategy_lab_artifact_dir(&dir).expect("evaluate artifact");

    assert_eq!(evaluation.strategy_id, "old_strategy");
    assert_eq!(evaluation.symbol, "SPY");
    assert_eq!(evaluation.timeframe, "unknown");
    assert_ne!(evaluation.decision, StrategyLabDecision::InsufficientData);
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

#[test]
fn strategy_lab_artifact_tree_ranks_multiple_valid_folders_deterministically() {
    let root = temp_artifact_dir("rank_tree");
    let weak = root.join("b_weak");
    let strong = root.join("a_strong");
    fs::create_dir_all(&weak).expect("create weak artifact");
    fs::create_dir_all(&strong).expect("create strong artifact");
    write_rank_metrics(&weak, "weak_strategy", "MSFT", "5m", 15.0);
    write_rank_metrics(&strong, "strong_strategy", "AAPL", "1m", 40.0);

    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        &root,
        mqk_artifacts::StrategyLabArtifactRankOptions::default(),
    )
    .expect("rank artifact tree");

    assert_eq!(report.root_path, root);
    assert_eq!(report.candidates_scanned, 2);
    assert_eq!(report.evaluations_count, 2);
    assert_eq!(report.failures_count, 0);
    assert_eq!(report.ranked[0].strategy_id, "strong_strategy");
    assert_eq!(report.ranked[0].symbol, "AAPL");
    assert_eq!(report.ranked[0].timeframe, "1m");
    assert_eq!(report.ranked[0].decision, "research_pass");
    assert_eq!(report.ranked[1].strategy_id, "weak_strategy");
}

#[test]
fn strategy_lab_artifact_tree_top_n_limits_ranked_rows() {
    let root = temp_artifact_dir("rank_top_n");
    for (name, total_return_pct) in [("one", 40.0), ("two", 30.0), ("three", 20.0)] {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("create artifact");
        write_rank_metrics(&dir, name, "SPY", "1m", total_return_pct);
    }

    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        &root,
        mqk_artifacts::StrategyLabArtifactRankOptions { top_n: Some(2) },
    )
    .expect("rank artifact tree");

    assert_eq!(report.candidates_scanned, 3);
    assert_eq!(report.evaluations_count, 3);
    assert_eq!(report.failures_count, 0);
    assert_eq!(report.ranked.len(), 2);
    assert_eq!(report.ranked[0].rank, 1);
    assert_eq!(report.ranked[1].rank, 2);
}

#[test]
fn strategy_lab_artifact_tree_malformed_candidate_is_reported_with_valid_results() {
    let root = temp_artifact_dir("rank_mixed");
    let valid = root.join("valid");
    let malformed = root.join("malformed");
    fs::create_dir_all(&valid).expect("create valid artifact");
    fs::create_dir_all(&malformed).expect("create malformed artifact");
    write_rank_metrics(&valid, "valid_strategy", "SPY", "1m", 35.0);
    fs::write(malformed.join("metrics.json"), "{ not valid json").expect("write malformed");

    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        &root,
        mqk_artifacts::StrategyLabArtifactRankOptions::default(),
    )
    .expect("rank artifact tree");

    assert_eq!(report.candidates_scanned, 2);
    assert_eq!(report.evaluations_count, 1);
    assert_eq!(report.failures_count, 1);
    assert_eq!(report.ranked[0].strategy_id, "valid_strategy");
    assert_eq!(report.failures[0].artifact_path, malformed);
    assert!(
        report.failures[0]
            .error
            .contains("parse metrics.json failed"),
        "unexpected error: {}",
        report.failures[0].error
    );
}

#[test]
fn strategy_lab_artifact_tree_no_candidates_is_truthful() {
    let root = temp_artifact_dir("rank_empty");

    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        &root,
        mqk_artifacts::StrategyLabArtifactRankOptions::default(),
    )
    .expect("rank empty artifact tree");

    assert_eq!(report.candidates_scanned, 0);
    assert_eq!(report.evaluations_count, 0);
    assert_eq!(report.failures_count, 0);
    assert!(report.ranked.is_empty());
    assert!(report.failures.is_empty());
}

#[test]
fn strategy_lab_artifact_tree_stable_tie_breaker_uses_artifact_path() {
    let root = temp_artifact_dir("rank_tie_path");
    let first = root.join("a_first");
    let second = root.join("b_second");
    fs::create_dir_all(&first).expect("create first artifact");
    fs::create_dir_all(&second).expect("create second artifact");
    write_rank_metrics(&second, "same_strategy", "SPY", "1m", 25.0);
    write_rank_metrics(&first, "same_strategy", "SPY", "1m", 25.0);

    let report = mqk_artifacts::rank_strategy_lab_artifact_tree(
        &root,
        mqk_artifacts::StrategyLabArtifactRankOptions::default(),
    )
    .expect("rank artifact tree");

    assert_eq!(report.candidates_scanned, 2);
    assert_eq!(report.ranked[0].artifact_path, first);
    assert_eq!(report.ranked[1].artifact_path, second);
}
