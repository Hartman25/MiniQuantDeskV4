//! BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01: proves the same
//! opt-in economics flags already proven on `mqk backtest csv`/`mqk backtest
//! db` also reach `mqk backtest csv-sweep`, applied identically to every
//! sweep combination's engine, artifacts, and manifest.
//!
//! No DB, no provider, no broker: every test runs a CSV sweep against a
//! temp-file bars fixture and a temp output directory, matching the
//! existing `scenario_cli_backtest_economics.rs` style.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

/// Same shape as `scenario_cli_backtest_economics.rs`'s fixture: three
/// same-day, evenly-spaced (300s) bars matching `intraday_scalper`'s
/// required `timeframe_secs=300`.
fn write_bars_csv(tag: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_backtest_sweep_economics_{}_{}.csv",
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
    std::env::temp_dir().join(format!(
        "mqk_cli_bkt_sweep_econ_out_{}_{}",
        tag,
        Uuid::new_v4()
    ))
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
/// byte-identical to pre-patch behavior; every per-point manifest.json
/// carries the truthful default.
#[test]
fn backtest_csv_sweep_economics_default_preserves_equity() -> anyhow::Result<()> {
    let bars = write_bars_csv("besw01")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("besw01");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv-sweep",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--target-qty",
        "1",
        "--slippage-bps",
        "0",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("sweep_economics_contract_multiplier=1"));

    let run_dir = extract_first_run_dir(&stdout);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json"))?)?;
    let econ = &manifest["economics"];
    assert_eq!(econ["contract_multiplier"], 1);
    assert!(econ["initial_margin_micros"].is_null());
    assert_eq!(econ["margin_enforced"], false);
    assert_eq!(econ["source"], "default_equity");

    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 1);

    Ok(())
}

/// --contract-multiplier 50 reaches every sweep point's manifest.json and
/// metrics.json, not just a single run.
#[test]
fn backtest_csv_sweep_economics_multiplier_50_reaches_every_point() -> anyhow::Result<()> {
    let bars = write_bars_csv("besw02")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("besw02");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv-sweep",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--target-qty",
        "1,2",
        "--slippage-bps",
        "0",
        "--contract-multiplier",
        "50",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(stdout.contains("sweep_economics_contract_multiplier=50"));

    let run_dirs = extract_all_run_dirs(&stdout);
    assert_eq!(run_dirs.len(), 2, "expected one run dir per sweep point");
    for run_dir in run_dirs {
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json"))?)?;
        assert_eq!(manifest["economics"]["contract_multiplier"], 50);
        assert_eq!(manifest["economics"]["source"], "explicit_request");

        let metrics: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
        assert_eq!(metrics["economics"]["contract_multiplier"], 50);
    }

    Ok(())
}

/// Margin flags are recorded as metadata in every sweep point's
/// manifest.json/metrics.json, and margin_enforced stays false.
#[test]
fn backtest_csv_sweep_economics_margin_metadata_not_enforced() -> anyhow::Result<()> {
    let bars = write_bars_csv("besw03")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("besw03");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let stdout = run_cli_ok(&[
        "backtest",
        "csv-sweep",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--target-qty",
        "1",
        "--slippage-bps",
        "0",
        "--contract-multiplier",
        "50",
        "--initial-margin-micros",
        "10000000000",
        "--maintenance-margin-micros",
        "5000000000",
        "--out-dir",
        &out_dir_s,
    ])?;

    let run_dir = extract_first_run_dir(&stdout);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("manifest.json"))?)?;
    let econ = &manifest["economics"];
    assert_eq!(econ["initial_margin_micros"], 10_000_000_000i64);
    assert_eq!(econ["maintenance_margin_micros"], 5_000_000_000i64);
    assert_eq!(
        econ["margin_enforced"], false,
        "margin metadata must never claim enforcement"
    );

    Ok(())
}

/// --contract-multiplier 0 fails closed before any bars are loaded or any
/// artifact directory is created.
#[test]
fn backtest_csv_sweep_economics_zero_multiplier_fails_closed() -> anyhow::Result<()> {
    let bars = write_bars_csv("besw04")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("besw04");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = run_cli(&[
        "backtest",
        "csv-sweep",
        "--bars",
        &bars_s,
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--target-qty",
        "1",
        "--slippage-bps",
        "0",
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

/// Parses the first `artifact_path=<path>` value out of `sweep_summary.json`
/// referenced by `sweep_dir=<path>` in stdout.
fn extract_first_run_dir(stdout: &str) -> PathBuf {
    extract_all_run_dirs(stdout)
        .into_iter()
        .next()
        .expect("expected at least one sweep run dir")
}

fn extract_all_run_dirs(stdout: &str) -> Vec<PathBuf> {
    let sweep_dir = stdout
        .lines()
        .find_map(|line| line.strip_prefix("sweep_dir="))
        .map(PathBuf::from)
        .expect("stdout did not contain sweep_dir=...");

    let summary: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(sweep_dir.join("sweep_summary.json"))
            .expect("failed to read sweep_summary.json"),
    )
    .expect("failed to parse sweep_summary.json");

    summary["rows"]
        .as_array()
        .expect("sweep_summary.json rows must be an array")
        .iter()
        .map(|row| {
            PathBuf::from(
                row["artifact_path"]
                    .as_str()
                    .expect("sweep row missing artifact_path"),
            )
        })
        .collect()
}
