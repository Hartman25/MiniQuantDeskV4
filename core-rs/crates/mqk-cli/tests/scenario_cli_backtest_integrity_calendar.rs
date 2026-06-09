use std::path::PathBuf;

use uuid::Uuid;

fn write_weekend_gap_csv() -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_backtest_integrity_calendar_{}.csv",
        Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        concat!(
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
            "AAPL,1772225700,100000000,101000000,99000000,100500000,1000,1\n",
            "AAPL,1772461800,100500000,101500000,100000000,101000000,1200,1\n",
        ),
    )?;
    Ok(path)
}

fn run_backtest_csv(args: &[&str]) -> anyhow::Result<String> {
    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn backtest_csv_default_integrity_calendar_remains_always_on() -> anyhow::Result<()> {
    let bars = write_weekend_gap_csv()?;
    let bars_s = bars.to_string_lossy().to_string();

    let stdout = run_backtest_csv(&[
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
        "--integrity-stale-threshold-ticks",
        "0",
        "--integrity-gap-tolerance-bars",
        "0",
    ])?;

    assert!(stdout.contains("bars_loaded=2"));
    assert!(
        stdout.contains("execution_blocked=true"),
        "AlwaysOn default must still block the cross-weekend gap; stdout:\n{stdout}"
    );

    Ok(())
}

#[test]
fn backtest_csv_parses_us_equity_regular_calendar_and_ignores_weekend_gap() -> anyhow::Result<()> {
    let bars = write_weekend_gap_csv()?;
    let bars_s = bars.to_string_lossy().to_string();

    let stdout = run_backtest_csv(&[
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
        "--integrity-stale-threshold-ticks",
        "0",
        "--integrity-gap-tolerance-bars",
        "0",
        "--integrity-calendar",
        "us-equity-regular",
    ])?;

    assert!(stdout.contains("bars_loaded=2"));
    assert!(
        stdout.contains("execution_blocked=false"),
        "us-equity-regular must ignore expected weekend gap; stdout:\n{stdout}"
    );

    Ok(())
}
