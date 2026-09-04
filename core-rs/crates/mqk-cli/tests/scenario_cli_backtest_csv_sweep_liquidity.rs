//! BKT-BAR-VOLUME-CAPACITY-SWEEP-01: proves `mqk backtest csv-sweep`'s
//! `--max-participation-rate-bps` flag reaches `bkt.rs`'s manual per-point
//! config loop -- a separate code path from the canonical `run_sweep` seam,
//! and the one that was found missing this exact wire-up (the loop built
//! each point's `BacktestConfig` by hand and never set `cfg.liquidity`,
//! so an enabled cap would have silently never reached the engine) -- and
//! that the new sweep_summary columns are purely additive (schema_version
//! stays "sweep-summary-v1").
//!
//! `csv_sweep_liquidity_rate_above_10000_bps_fails_closed` is the
//! load-bearing negative control for the wire-up itself: with `cfg.liquidity`
//! disconnected, an out-of-range rate no longer reaches
//! `BacktestEngine::run`'s own `validate_liquidity_config`, so the sweep
//! would wrongly succeed instead of failing closed (confirmed by temporarily
//! commenting out the `cfg.liquidity = ...` assignment and re-running this
//! suite). None of the built-in registered strategies (swing_momentum,
//! mean_reversion, volatility_breakout, intraday_scalper) reliably fire a
//! trade on a small hand-written 3-bar CSV fixture, so
//! `csv_sweep_liquidity_flag_reaches_every_sweep_row` proves the narrower
//! (but still real) invariant that `SweepPoint.max_participation_rate_bps`
//! is faithfully carried into each row's own field, not that a rejection is
//! actually observed -- the engine-level enforcement itself (cap causing a
//! real fill-vs-reject split as target_qty crosses the ceiling) is proven
//! directly against `run_sweep` in
//! `mqk-backtest/tests/scenario_bar_volume_capacity_sweep_01.rs`, which
//! controls its own deterministic strategy rather than depending on a
//! production strategy's signal thresholds.
//!
//! No DB, no provider, no broker: every test runs a CSV sweep against a
//! temp-file bars fixture and a temp output directory, matching the
//! existing `scenario_cli_backtest_csv_sweep_economics.rs` style.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

fn write_bars_csv(tag: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_backtest_sweep_liquidity_{}_{}.csv",
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

fn fresh_out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_cli_bkt_sweep_liq_out_{}_{}",
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

fn read_sweep_summary(stdout: &str) -> serde_json::Value {
    let sweep_dir = stdout
        .lines()
        .find_map(|line| line.strip_prefix("sweep_dir="))
        .map(PathBuf::from)
        .expect("stdout did not contain sweep_dir=...");
    serde_json::from_str(
        &std::fs::read_to_string(sweep_dir.join("sweep_summary.json"))
            .expect("failed to read sweep_summary.json"),
    )
    .expect("failed to parse sweep_summary.json")
}

/// No `--max-participation-rate-bps` flag -> every row's
/// `max_participation_rate_bps` is the base config default (0 = disabled),
/// byte-identical to pre-patch behavior.
#[test]
fn csv_sweep_omitted_liquidity_flag_defaults_to_disabled() -> anyhow::Result<()> {
    let bars = write_bars_csv("liq01")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("liq01");
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

    let summary = read_sweep_summary(&stdout);
    let rows = summary["rows"].as_array().expect("rows must be an array");
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(
            row["max_participation_rate_bps"], 0,
            "omitted flag must default to disabled (0): {row}"
        );
        assert_eq!(row["rejected_liquidity_capacity_count"], 0);
    }

    Ok(())
}

/// `--max-participation-rate-bps 500` must reach every sweep row's own
/// `max_participation_rate_bps` field. See the module doc comment for why
/// this test proves point-to-row passthrough rather than an observed
/// rejection, and where the stronger engine-enforcement proof lives.
#[test]
fn csv_sweep_liquidity_flag_reaches_every_sweep_row() -> anyhow::Result<()> {
    let bars = write_bars_csv("liq02")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("liq02");
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
        "--max-participation-rate-bps",
        "500",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(
        stdout.contains("liq_cap=500"),
        "sweep_run progress line must report the configured cap: {stdout}"
    );

    let summary = read_sweep_summary(&stdout);
    let rows = summary["rows"].as_array().expect("rows must be an array");
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(
            row["max_participation_rate_bps"], 500,
            "the CLI flag must reach every sweep row, not just be parsed and dropped: {row}"
        );
    }

    let run_dir = PathBuf::from(
        rows[0]["artifact_path"]
            .as_str()
            .expect("sweep row missing artifact_path"),
    );
    assert!(
        run_dir.join("metrics.json").is_file(),
        "per-point metrics.json must still be written for the liquidity-configured run"
    );

    Ok(())
}

/// The new `max_participation_rate_bps` / `rejected_liquidity_capacity_count`
/// columns are purely additive: sweep_summary.json's schema_version stays
/// "sweep-summary-v1" (this repo's established convention -- see
/// `BacktestReportArtifact`'s own additive-field precedent -- is that a
/// purely additive field does not require a schema-version bump), and both
/// new columns are present in the CSV header and the JSON rows.
#[test]
fn sweep_summary_schema_version_unchanged_by_new_columns() -> anyhow::Result<()> {
    let bars = write_bars_csv("liq03")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("liq03");
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
        "--max-participation-rate-bps",
        "250",
        "--out-dir",
        &out_dir_s,
    ])?;

    let summary = read_sweep_summary(&stdout);
    assert_eq!(
        summary["schema_version"], "sweep-summary-v1",
        "additive row fields must not force a schema-version bump: {summary}"
    );
    let row = &summary["rows"][0];
    assert!(row.get("max_participation_rate_bps").is_some());
    assert!(row.get("rejected_liquidity_capacity_count").is_some());

    let sweep_dir = stdout
        .lines()
        .find_map(|line| line.strip_prefix("sweep_dir="))
        .map(PathBuf::from)
        .expect("stdout did not contain sweep_dir=...");
    let csv = std::fs::read_to_string(sweep_dir.join("sweep_summary.csv"))?;
    let header = csv.lines().next().expect("csv must have a header row");
    assert!(header.contains("max_participation_rate_bps"));
    assert!(header.contains("rejected_liquidity_capacity_count"));

    Ok(())
}

/// A `max-participation-rate-bps` value above 10_000 (>100%) must fail the
/// sweep closed at run start, matching the engine-level
/// `InvalidLiquidityConfig` bound -- not silently clamp or accept it.
#[test]
fn csv_sweep_liquidity_rate_above_10000_bps_fails_closed() -> anyhow::Result<()> {
    let bars = write_bars_csv("liq04")?;
    let bars_s = bars.to_string_lossy().to_string();
    let out_dir = fresh_out_dir("liq04");
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
        "--max-participation-rate-bps",
        "10001",
        "--out-dir",
        &out_dir_s,
    ])?;

    assert!(
        !output.status.success(),
        "an invalid (>10_000 bps) liquidity cap must fail the sweep, not silently proceed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("InvalidLiquidityConfig") || stderr.contains("10001"),
        "error must identify the invalid liquidity config: {stderr}"
    );

    Ok(())
}
