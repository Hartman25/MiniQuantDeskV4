//! BACKTEST-ECONOMICS-CLI-ENTRY-01: proves the first real operator entry point
//! for backtest economics -- CLI-only opt-in flags on `mqk backtest csv` that
//! reach `BacktestEngine::with_economics`, `BacktestReport.economics`,
//! `metrics.json`, and `report.md`.
//!
//! No DB, no provider, no broker: every test runs a CSV backtest against a
//! temp-file bars fixture and a temp output directory, matching the existing
//! `scenario_cli_backtest_integrity_calendar.rs` style.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

/// Three same-day, evenly-spaced (300s) bars: matches `intraday_scalper`'s
/// required `timeframe_secs=300` (see `StrategySpec`), so the strategy host
/// never rejects the run with `TimeframeMismatch` and default integrity
/// settings never block execution -- keeping these tests focused purely on
/// economics wiring.
fn write_bars_csv(tag: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_backtest_economics_{}_{}.csv",
        tag,
        Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        concat!(
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
            "AAPL,1700000300,100000000,101000000,99000000,100500000,1000,1\n",
            "AAPL,1700000600,100500000,101500000,100000000,101000000,1200,1\n",
            "AAPL,1700000900,101000000,102000000,100500000,101500000,1300,1\n",
        ),
    )?;
    Ok(path)
}

/// Fresh, never-created output directory path for a single test run.
fn fresh_out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mqk_cli_bkt_econ_out_{}_{}", tag, Uuid::new_v4()))
}

fn run_cli(args: &[&str]) -> anyhow::Result<Output> {
    Ok(assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .output()?)
}

fn run_cli_ok(args: &[&str]) -> anyhow::Result<String> {
    let output = run_cli(args)?;
    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// no economics flags -> default equity economics (multiplier=1, no margin),
/// byte-identical to pre-patch behavior.
#[test]
fn backtest_csv_economics_default_preserves_equity() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli01")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli01");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("economics_contract_multiplier=1"));
    assert!(stdout.contains("economics_margin_enforced=false"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    let econ = &metrics["economics"];
    assert_eq!(econ["contract_multiplier"], 1);
    assert!(econ["initial_margin_micros"].is_null());
    assert!(econ["maintenance_margin_micros"].is_null());
    assert_eq!(econ["margin_enforced"], false);

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 1"));

    Ok(())
}

/// --contract-multiplier 50 reaches report.economics, metrics.json, and report.md.
#[test]
fn backtest_csv_economics_multiplier_50_appears_in_artifacts() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli02")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli02");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--contract-multiplier",
        "50",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("economics_contract_multiplier=50"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 50);

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 50"));

    Ok(())
}

/// --contract-multiplier 100 works the same way as 50 (no special-casing of 50).
#[test]
fn backtest_csv_economics_multiplier_100_appears_in_artifacts() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli03")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli03");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--contract-multiplier",
        "100",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("economics_contract_multiplier=100"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 100);

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 100"));

    Ok(())
}

/// Margin flags are recorded as metadata in metrics.json/report.md, and
/// margin_enforced stays false -- the margin scaffold is never enforced.
#[test]
fn backtest_csv_economics_margin_metadata_not_enforced() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli04")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli04");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--contract-multiplier",
        "50",
        "--initial-margin-micros",
        "10000000000",
        "--maintenance-margin-micros",
        "5000000000",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("economics_margin_enforced=false"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    let econ = &metrics["economics"];
    assert_eq!(econ["initial_margin_micros"], 10_000_000_000i64);
    assert_eq!(econ["maintenance_margin_micros"], 5_000_000_000i64);
    assert_eq!(
        econ["margin_enforced"], false,
        "margin metadata must never claim enforcement"
    );

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Margin Enforced | false"));

    Ok(())
}

/// --contract-multiplier 0 fails closed before any artifact directory exists.
#[test]
fn backtest_csv_economics_zero_multiplier_fails_closed() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli05")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli05");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = run_cli(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--contract-multiplier",
        "0",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(
        !output.status.success(),
        "CLI must fail closed on --contract-multiplier 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_multiplier must be positive"),
        "stderr must explain the rejection; stderr:\n{stderr}"
    );
    assert!(
        !out_dir.exists(),
        "no artifact directory must be created on fail-closed rejection"
    );

    Ok(())
}

/// A negative --contract-multiplier fails closed the same way as zero.
#[test]
fn backtest_csv_economics_negative_multiplier_fails_closed() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli06")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli06");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = run_cli(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        // clap parses a bare `-5` as a possible flag; `=` form is unambiguous.
        "--contract-multiplier=-5",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(
        !output.status.success(),
        "CLI must fail closed on a negative --contract-multiplier"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_multiplier must be positive"),
        "stderr must explain the rejection; stderr:\n{stderr}"
    );
    assert!(
        !out_dir.exists(),
        "no artifact directory must be created on fail-closed rejection"
    );

    Ok(())
}

/// Omitting --contract-multiplier while supplying margin flags defaults the
/// multiplier to 1 (still positive, so the run succeeds with equity-style
/// multiplier but caller-supplied margin metadata).
#[test]
fn backtest_csv_economics_margin_only_defaults_multiplier_one() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli07")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli07");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--initial-margin-micros",
        "1000000",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("economics_contract_multiplier=1"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 1);
    assert_eq!(metrics["economics"]["initial_margin_micros"], 1_000_000);

    Ok(())
}

/// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: no economics flags -> manifest.json
/// carries truthful default_equity economics (mirrors metrics.json).
#[test]
fn backtest_csv_economics_default_manifest_is_truthful() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli08")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli08");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--out-dir",
        &out_dir_s,
    ])?;

    let run_dir = extract_artifacts_dir(&stdout);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json"))?)?;
    let econ = &manifest["economics"];
    assert_eq!(econ["contract_multiplier"], 1);
    assert!(econ["initial_margin_micros"].is_null());
    assert!(econ["maintenance_margin_micros"].is_null());
    assert_eq!(econ["margin_enforced"], false);
    assert_eq!(econ["source"], "default_equity");

    Ok(())
}

/// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: --contract-multiplier 50 reaches
/// manifest.json (not just metrics.json/report.md).
#[test]
fn backtest_csv_economics_multiplier_50_reaches_manifest() -> anyhow::Result<()> {
    let bars = write_bars_csv("becli09")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("becli09");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--contract-multiplier",
        "50",
        "--initial-margin-micros",
        "10000000000",
        "--maintenance-margin-micros",
        "5000000000",
        "--out-dir",
        &out_dir_s,
    ])?;

    let run_dir = extract_artifacts_dir(&stdout);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json"))?)?;
    let econ = &manifest["economics"];
    assert_eq!(econ["contract_multiplier"], 50);
    assert_eq!(econ["initial_margin_micros"], 10_000_000_000i64);
    assert_eq!(econ["maintenance_margin_micros"], 5_000_000_000i64);
    assert_eq!(
        econ["margin_enforced"], false,
        "margin metadata must never claim enforcement"
    );
    assert_eq!(econ["source"], "explicit_request");

    // metrics.json/report.md must carry the same economics, unregressed.
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 50);
    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 50"));

    Ok(())
}

/// Parses the `artifacts_dir=<path>` line that `run_backtest_csv` prints to stdout.
fn extract_artifacts_dir(stdout: &str) -> PathBuf {
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("artifacts_dir=") {
            return PathBuf::from(path);
        }
    }
    panic!("stdout did not contain artifacts_dir=...; stdout:\n{stdout}");
}
