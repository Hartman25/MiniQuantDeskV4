//! Proves `mqk backtest native-signals` additively writes a
//! `benchmark_v2.json` sibling artifact (capital-matched exact-target
//! passive benchmark) alongside the existing `native_signals.csv` /
//! `native_signals_meta.json` outputs, without changing either of those
//! existing files' schema.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

/// Five 300s bars rising 25bps over the lookback -- enough to trip
/// `intraday_scalper`'s long-entry threshold on the last bar (matches the
/// fixture already proven in `mqk-runtime`'s
/// `scenario_crypto_sizing_bootstrap_a3c.rs::rising_window`).
fn write_bullish_bars_csv(tag: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_native_signals_bench_v2_{}_{}.csv",
        tag,
        Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        concat!(
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
            "AAPL,300,100000000,100000000,100000000,100000000,1000,1\n",
            "AAPL,600,100050000,100050000,100050000,100050000,1000,1\n",
            "AAPL,900,100100000,100100000,100100000,100100000,1000,1\n",
            "AAPL,1200,100150000,100150000,100150000,100150000,1000,1\n",
            "AAPL,1500,100250000,100250000,100250000,100250000,1000,1\n",
        ),
    )?;
    Ok(path)
}

/// Five flat 300s bars -- never trips a long signal, so Benchmark V2 has no
/// positive quantity to match and must be legitimately absent (not fabricated).
fn write_flat_bars_csv(tag: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_native_signals_bench_v2_flat_{}_{}.csv",
        tag,
        Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        concat!(
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
            "AAPL,300,100000000,100000000,100000000,100000000,1000,1\n",
            "AAPL,600,100000000,100000000,100000000,100000000,1000,1\n",
            "AAPL,900,100000000,100000000,100000000,100000000,1000,1\n",
            "AAPL,1200,100000000,100000000,100000000,100000000,1000,1\n",
            "AAPL,1500,100000000,100000000,100000000,100000000,1000,1\n",
        ),
    )?;
    Ok(path)
}

fn fresh_out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_cli_native_signals_bench_v2_out_{}_{}",
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

/// REQUIRED 1/2: a triggered long signal produces a `benchmark_v2.json`
/// carrying the exact (not resized) target quantity and the versioned
/// policy id, and the command's existing stdout/exit-success contract is
/// unchanged.
#[test]
fn bullish_run_writes_benchmark_v2_json_with_exact_quantity_and_policy_id() -> anyhow::Result<()> {
    let bars = write_bullish_bars_csv("bullish")?;
    let out_dir = fresh_out_dir("bullish");

    let stdout = run_cli_ok(&[
        "backtest",
        "native-signals",
        "--bars-path",
        bars.to_str().unwrap(),
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--out-dir",
        out_dir.to_str().unwrap(),
    ])?;
    assert!(stdout.contains("native_signals_csv="));

    // Existing artifacts are unchanged.
    assert!(out_dir.join("native_signals.csv").is_file());
    assert!(out_dir.join("native_signals_meta.json").is_file());

    // New, additive artifact.
    let bench_path = out_dir.join("benchmark_v2.json");
    assert!(
        bench_path.is_file(),
        "benchmark_v2.json must be written for a triggered long signal"
    );
    let bench: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&bench_path)?)?;
    assert_eq!(
        bench["policy_id"],
        serde_json::json!("capital_matched_exact_target_buy_hold_v1")
    );
    assert_eq!(bench["symbol"], serde_json::json!("AAPL"));
    assert_eq!(
        bench["target_qty_micros"],
        serde_json::json!(1_000_000),
        "intraday_scalper default sizing is one whole share -- never resized"
    );
    assert!(bench["account_return_pct"].is_number());
    assert!(bench["alpha_pct"].is_number());

    std::fs::remove_file(&bars).ok();
    std::fs::remove_dir_all(&out_dir).ok();
    Ok(())
}

/// REQUIRED: a strategy/window combination that never goes long has no
/// comparable Benchmark V2 result -- the command still succeeds (the
/// signal-emission job it already had is unaffected), but no
/// `benchmark_v2.json` is fabricated.
#[test]
fn flat_run_succeeds_without_fabricating_a_benchmark_v2_result() -> anyhow::Result<()> {
    let bars = write_flat_bars_csv("flat")?;
    let out_dir = fresh_out_dir("flat");

    run_cli_ok(&[
        "backtest",
        "native-signals",
        "--bars-path",
        bars.to_str().unwrap(),
        "--strategy",
        "intraday_scalper",
        "--symbol",
        "AAPL",
        "--timeframe-secs",
        "300",
        "--out-dir",
        out_dir.to_str().unwrap(),
    ])?;

    assert!(out_dir.join("native_signals.csv").is_file());
    assert!(out_dir.join("native_signals_meta.json").is_file());
    assert!(
        !out_dir.join("benchmark_v2.json").is_file(),
        "a never-triggered strategy must not get a fabricated benchmark result"
    );

    std::fs::remove_file(&bars).ok();
    std::fs::remove_dir_all(&out_dir).ok();
    Ok(())
}
