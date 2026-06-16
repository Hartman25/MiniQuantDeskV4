//! BT-E2E-01 — Deterministic fixture-driven end-to-end backtest proof.
//!
//! Runs the full backtest pipeline (load CSV → engine → write artifacts) using
//! the embedded bkt4_bars.csv fixture and asserts:
//!
//! 1. All expected artifact files are written.
//! 2. metrics.json contains benchmark fields (buy_and_hold_return_pct, alpha_pct).
//! 3. report.md contains required operator-readable sections.
//! 4. Repeated runs over the same fixture produce byte-identical metrics.json and report.md.

use mqk_backtest::{BacktestConfig, BacktestEngine};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec};
use std::fs;
use uuid::Uuid;

// Embed the fixture so this test is self-contained and compile-time verified.
const BKT4_BARS_CSV: &str = include_str!("fixtures/bkt4_bars.csv");

/// Minimal hold-nothing strategy: never emits any trade intents.
/// Used to satisfy the engine's requirement for a registered strategy.
struct PassThroughStrategy;

impl Strategy for PassThroughStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("bt_e2e_01_passthrough", 60)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

/// Parse the embedded CSV string, run the engine, write artifacts to a temp dir.
/// Returns (run_dir, metrics_json_content, report_md_content).
fn run_e2e_fixture(label: &str) -> (std::path::PathBuf, String, String) {
    let bars = mqk_backtest::parse_csv_bars(BKT4_BARS_CSV).expect("parse fixture CSV");
    assert!(!bars.is_empty(), "fixture must have at least one bar");

    let cfg = BacktestConfig::test_defaults();
    let initial_cash = cfg.initial_cash_micros;

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(PassThroughStrategy))
        .expect("add_strategy");
    let report = engine.run(&bars).expect("engine.run must succeed");

    let tmp = std::env::temp_dir().join(format!("mqk_bt_e2e_01_{}_{}", label, std::process::id()));
    fs::create_dir_all(&tmp).unwrap();

    // Build a minimal init_run_artifacts call (manifest + run dir).
    let now_utc: chrono::DateTime<chrono::Utc> =
        "2023-11-14T22:13:20Z".parse().expect("fixed timestamp");
    let init = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: &tmp,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: report.strategy_name.as_str(),
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "test",
        config_hash: &report.config_id.to_string(),
        host_fingerprint: "e2e-test",
        now_utc,
    })
    .expect("init_run_artifacts");

    mqk_artifacts::write_backtest_report(&init.run_dir, &report, initial_cash)
        .expect("write_backtest_report");

    let metrics_json =
        fs::read_to_string(init.run_dir.join("metrics.json")).expect("metrics.json must exist");
    let report_md =
        fs::read_to_string(init.run_dir.join("report.md")).expect("report.md must exist");

    (init.run_dir, metrics_json, report_md)
}

/// E2E-01: All expected artifact files are written.
#[test]
fn e2e_01_all_artifact_files_written() {
    let (run_dir, _, _) = run_e2e_fixture("e2e01");
    for name in &[
        "orders.csv",
        "fills.csv",
        "equity_curve.csv",
        "metrics.json",
        "report.md",
    ] {
        assert!(
            run_dir.join(name).exists(),
            "{} must exist in run_dir",
            name
        );
    }
}

/// E2E-02: metrics.json contains benchmark fields.
#[test]
fn e2e_02_metrics_json_has_benchmark_fields() {
    let (_, metrics_json, _) = run_e2e_fixture("e2e02");
    let v: serde_json::Value =
        serde_json::from_str(&metrics_json).expect("metrics.json must be valid JSON");

    // bkt4_bars.csv has 3 bars with valid prices — benchmark must be present.
    let bm = &v["benchmark"];
    assert!(
        !bm.is_null(),
        "benchmark must be present for 3-bar fixture; got: {}",
        metrics_json
    );
    assert!(
        bm["buy_and_hold_return_pct"].is_number(),
        "buy_and_hold_return_pct must be a number"
    );
    assert!(bm["alpha_pct"].is_number(), "alpha_pct must be a number");
    assert!(
        bm["strategy_total_return_pct"].is_number(),
        "strategy_total_return_pct must be a number"
    );
    assert!(
        bm["first_bar_open_micros"].is_number(),
        "first_bar_open_micros must be a number"
    );
    assert!(
        bm["last_bar_close_micros"].is_number(),
        "last_bar_close_micros must be a number"
    );
}

/// E2E-03: Benchmark calculation is correct for bkt4_bars.csv.
///
/// bkt4_bars.csv:
///   row 1: open=995000, close=1000000
///   row 2: open=1000000, close=1020000
///   row 3: open=1020000, close=1015000
///
/// first_open=995000, last_close=1015000
/// buy_and_hold = (1015000 / 995000 - 1) * 100 ≈ 2.0101%
#[test]
fn e2e_03_benchmark_calculation_correct() {
    let (_, metrics_json, _) = run_e2e_fixture("e2e03");
    let v: serde_json::Value = serde_json::from_str(&metrics_json).unwrap();
    let bm = &v["benchmark"];

    let first_open = bm["first_bar_open_micros"].as_i64().unwrap();
    let last_close = bm["last_bar_close_micros"].as_i64().unwrap();
    assert_eq!(
        first_open, 995_000,
        "first_open must be 995000 (bkt4 row1 open)"
    );
    assert_eq!(
        last_close, 1_015_000,
        "last_close must be 1015000 (bkt4 row3 close)"
    );

    let bah = bm["buy_and_hold_return_pct"].as_f64().unwrap();
    let expected_bah = (1_015_000.0f64 / 995_000.0 - 1.0) * 100.0;
    let diff = (bah - expected_bah).abs();
    assert!(
        diff < 1e-6,
        "buy_and_hold_return_pct={} expected≈{}",
        bah,
        expected_bah
    );

    // Alpha = strategy_return - buy_and_hold_return
    let alpha = bm["alpha_pct"].as_f64().unwrap();
    let strategy_return = bm["strategy_total_return_pct"].as_f64().unwrap();
    let alpha_check = (alpha - (strategy_return - bah)).abs();
    assert!(alpha_check < 1e-9, "alpha must equal strategy_return - bah");
}

/// E2E-04: report.md contains required operator sections.
#[test]
fn e2e_04_report_md_contains_required_sections() {
    let (_, _, report_md) = run_e2e_fixture("e2e04");
    for needle in &[
        "# Backtest Report",
        "## Identity",
        "Strategy",
        "## Equity Performance",
        "Total Return",
        "Max Drawdown",
        "## Benchmark Comparison",
        "Buy-and-Hold Return",
        "Alpha vs Benchmark",
        "## Trade Statistics",
        "## Assumptions",
        "Backtest results do not guarantee future profitability",
    ] {
        assert!(
            report_md.contains(needle),
            "report.md missing section/text: '{}'",
            needle
        );
    }
}

/// E2E-05: Repeated runs over the same fixture produce byte-identical metrics.json and report.md.
#[test]
fn e2e_05_repeated_runs_are_deterministic() {
    let (_, metrics_a, report_a) = run_e2e_fixture("e2e05a");
    let (_, metrics_b, report_b) = run_e2e_fixture("e2e05b");

    assert_eq!(
        metrics_a, metrics_b,
        "metrics.json must be byte-identical across repeated runs"
    );
    assert_eq!(
        report_a, report_b,
        "report.md must be byte-identical across repeated runs"
    );
}

/// E2E-06: Benchmark is None when no bars are processed.
#[test]
fn e2e_06_benchmark_none_for_empty_bars() {
    let cfg = BacktestConfig::test_defaults();
    let initial_cash = cfg.initial_cash_micros;
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(PassThroughStrategy))
        .expect("add_strategy");
    let report = engine.run(&[]).expect("empty run");

    let tmp = std::env::temp_dir().join(format!("mqk_bt_e2e_06_{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    let now_utc: chrono::DateTime<chrono::Utc> =
        "2023-11-14T22:13:20Z".parse().expect("fixed timestamp");
    let init = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: &tmp,
        schema_version: 1,
        run_id: Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"e2e06"),
        strategy_name: "",
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "test",
        config_hash: "empty",
        host_fingerprint: "e2e-test",
        now_utc,
    })
    .unwrap();

    mqk_artifacts::write_backtest_report(&init.run_dir, &report, initial_cash).unwrap();
    let metrics_json = fs::read_to_string(init.run_dir.join("metrics.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&metrics_json).unwrap();

    // benchmark field must be absent (skipped) when no bars ran.
    assert!(
        v["benchmark"].is_null(),
        "benchmark must be null/absent for empty bar sequence"
    );
}
