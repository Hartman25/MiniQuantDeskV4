use std::path::PathBuf;

use uuid::Uuid;

fn write_regime_csv(label: &str, body: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("mqk_cli_regime_{label}_{}.csv", Uuid::new_v4()));
    std::fs::write(
        &path,
        format!(
            "{}{}",
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
            body
        ),
    )?;
    Ok(path)
}

fn run_regime_detect(args: &[&str]) -> anyhow::Result<std::process::Output> {
    Ok(assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .output()?)
}

#[test]
fn regime_detect_valid_csv_text_report_works() -> anyhow::Result<()> {
    let csv = write_regime_csv(
        "valid_text",
        concat!(
            "SPY,1,100000000,100500000,99500000,100000000,1000,1\n",
            "SPY,2,101000000,101500000,100500000,101000000,1000,1\n",
            "SPY,3,102000000,102500000,101500000,102000000,1000,1\n",
            "SPY,4,103000000,103500000,102500000,103000000,1000,1\n",
            "SPY,5,104000000,104500000,103500000,104000000,1000,1\n",
            "SPY,6,105000000,105500000,104500000,105000000,1000,1\n",
            "SPY,7,106000000,106500000,105500000,106000000,1000,1\n",
            "SPY,8,107000000,107500000,106500000,107000000,1000,1\n",
        ),
    )?;
    let csv_s = csv.to_string_lossy().to_string();

    let output = run_regime_detect(&[
        "backtest",
        "regime-detect",
        "--csv",
        &csv_s,
        "--symbol",
        "SPY",
        "--timeframe",
        "1m",
    ])?;

    assert!(
        output.status.success(),
        "regime-detect failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("symbol=SPY"));
    assert!(stdout.contains("timeframe=1m"));
    assert!(stdout.contains("bar_count=8"));
    assert!(stdout.contains("regime_kind=bull_trend"));
    assert!(stdout.contains("reason_codes=research_only,positive_return,directional_consistency"));

    Ok(())
}

#[test]
fn regime_detect_json_output_is_parseable() -> anyhow::Result<()> {
    let csv = write_regime_csv(
        "json",
        concat!(
            "SPY,1,100000000,102000000,98000000,100000000,1000,1\n",
            "SPY,2,101000000,103000000,99000000,101000000,1000,1\n",
            "SPY,3,99000000,101000000,97000000,99000000,1000,1\n",
            "SPY,4,100500000,102500000,98500000,100500000,1000,1\n",
            "SPY,5,99500000,101500000,97500000,99500000,1000,1\n",
            "SPY,6,100250000,102250000,98250000,100250000,1000,1\n",
            "SPY,7,99750000,101750000,97750000,99750000,1000,1\n",
            "SPY,8,100000000,102000000,98000000,100000000,1000,1\n",
        ),
    )?;
    let csv_s = csv.to_string_lossy().to_string();

    let output = run_regime_detect(&[
        "backtest",
        "regime-detect",
        "--csv",
        &csv_s,
        "--symbol",
        "SPY",
        "--timeframe",
        "1m",
        "--json",
    ])?;

    assert!(
        output.status.success(),
        "regime-detect failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["symbol"], "SPY");
    assert_eq!(value["timeframe"], "1m");
    assert_eq!(value["bar_count"], 8);
    assert_eq!(value["regime_kind"], "sideways");
    assert_eq!(value["features"]["valid_bar_count"], 8);

    Ok(())
}

#[test]
fn regime_detect_missing_csv_fails_truthfully() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join(format!("mqk_cli_regime_missing_{}.csv", Uuid::new_v4()));
    let path_s = path.to_string_lossy().to_string();

    let output = run_regime_detect(&[
        "backtest",
        "regime-detect",
        "--csv",
        &path_s,
        "--symbol",
        "SPY",
        "--timeframe",
        "1m",
    ])?;

    assert!(!output.status.success(), "missing CSV must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("load bars csv failed"),
        "unexpected stderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn regime_detect_too_few_bars_reports_insufficient_data() -> anyhow::Result<()> {
    let csv = write_regime_csv(
        "too_few",
        concat!(
            "SPY,1,100000000,101000000,99000000,100000000,1000,1\n",
            "SPY,2,101000000,102000000,100000000,101000000,1000,1\n",
        ),
    )?;
    let csv_s = csv.to_string_lossy().to_string();

    let output = run_regime_detect(&[
        "backtest",
        "regime-detect",
        "--csv",
        &csv_s,
        "--symbol",
        "SPY",
        "--timeframe",
        "1m",
    ])?;

    assert!(
        output.status.success(),
        "too few bars should report successfully\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("regime_kind=insufficient_data"));
    assert!(stdout.contains("reason_codes=research_only,insufficient_data"));

    Ok(())
}
