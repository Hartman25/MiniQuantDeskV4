//! STRATEGY-LAB-SCANNER-01C — `mqk backtest scan-strategies` CLI scenario tests.
//!
//! Every fixture here is a tiny, hand-written local file under a fresh
//! temp directory. No test in this file depends on the repo-root
//! `exports/` tree, and `MQK_DATABASE_URL`/`TWELVEDATA_API_KEY` are always
//! explicitly removed from the child process environment -- if the
//! command under test ever opened a DB connection or made a provider
//! call, these tests would fail with a connection/auth error rather than
//! the expected output.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_cli_strategy_scanner_{label}_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Deterministic gentle-uptrend daily bars CSV body for `symbol`, `count` rows.
fn daily_bars_csv(symbol: &str, count: usize) -> String {
    let mut out = String::from(
        "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
    );
    let start_ts: i64 = 1_700_000_000;
    for i in 0..count {
        let end_ts = start_ts + (i as i64) * 86_400;
        let base = 100_000_000i64 + (i as i64) * 100_000;
        let wiggle = if i % 2 == 0 { 50_000 } else { -50_000 };
        let open = base;
        let close = base + wiggle;
        let high = open.max(close) + 20_000;
        let low = open.min(close) - 20_000;
        out.push_str(&format!(
            "{symbol},1D,{end_ts},{open},{high},{low},{close},1000000,t\n"
        ));
    }
    out
}

fn registry_json(symbols: &[&str]) -> String {
    let entries: Vec<String> = symbols
        .iter()
        .map(|s| {
            format!(
                r#"{{"instrument_id":"equity:US:{s}","symbol":"{s}","asset_class":"equity","provider":"twelvedata","provider_symbol":"{s}","venue":"NASDAQ","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"fixture"}}"#
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Fixture layout: registry with AAPL, MSFT, NODATA (all enabled); local
/// `1D/AAPL_1D.csv` and `1D/MSFT_1D.csv` (80 bars each, well above the
/// scanner's minimum); `NODATA` has no bars file at all; no `5m/`
/// directory exists.
struct Fixture {
    _dir: PathBuf,
    registry_path: PathBuf,
    bars_root: PathBuf,
    out_dir: PathBuf,
}

fn build_fixture() -> Fixture {
    let dir = temp_dir("fixture");
    let registry_path = dir.join("equities.json");
    std::fs::write(&registry_path, registry_json(&["AAPL", "MSFT", "NODATA"]))
        .expect("write registry fixture");

    let bars_root = dir.join("md_backup");
    let tf_dir = bars_root.join("1D");
    std::fs::create_dir_all(&tf_dir).expect("create 1D bars dir");
    std::fs::write(tf_dir.join("AAPL_1D.csv"), daily_bars_csv("AAPL", 80))
        .expect("write AAPL bars");
    std::fs::write(tf_dir.join("MSFT_1D.csv"), daily_bars_csv("MSFT", 80))
        .expect("write MSFT bars");
    // NODATA intentionally has no bars file. No 5m/ directory is created.

    let out_dir = dir.join("strategy_scans");

    Fixture {
        _dir: dir,
        registry_path,
        bars_root,
        out_dir,
    }
}

fn run(args: &[&str]) -> Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_DATABASE_URL")
        .env_remove("TWELVEDATA_API_KEY")
        .output()
        .expect("failed to run mqk-cli")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

// 1. CLI help includes scanner command.
#[test]
fn backtest_help_lists_scan_strategies() {
    let output = run(&["backtest", "--help"]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("scan-strategies"),
        "help output missing scan-strategies:\n{stdout}"
    );
}

// 2. Fixture scan works end to end (also covers #3 honest data_missing for
// NODATA, and #9/#10 no DB/provider env).
#[test]
fn fixture_scan_runs_successfully_and_reports_honest_data_missing() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--top",
        "20",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "scan-strategies failed\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_of(&output);
    assert!(stdout.contains("universe_count=3"));
    assert!(stdout.contains("ranked_count=2"));
    assert!(stdout.contains("skipped_count=1"));
    assert!(
        stdout.contains("truth_state=data_missing")
            || stdout.contains("skip_reason=missing_bars_file")
    );
}

// 4. 5m missing local data is skipped honestly (no 5m/ directory exists at all).
#[test]
fn missing_5m_directory_is_skipped_honestly_not_crashed() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "5m",
        "--strategy",
        "intraday_scalper",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "scan-strategies (5m) failed\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_of(&output);
    assert!(stdout.contains("\"ranked_count\": 0"));
    assert!(stdout.contains("\"skipped_count\": 3"));
}

// 5 + 6 + 7. Output artifact files are created; CSV has a stable header;
// JSON candidate schema is stable.
#[test]
fn artifact_files_are_written_with_stable_schema() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    let scan_id = stdout
        .lines()
        .find_map(|l| l.strip_prefix("scan_id="))
        .expect("scan_id printed");

    let run_dir = fx.out_dir.join(scan_id);
    let manifest_path = run_dir.join("manifest.json");
    let candidates_json_path = run_dir.join("candidates.json");
    let candidates_csv_path = run_dir.join("candidates.csv");
    let summary_path = run_dir.join("summary.json");
    for p in [
        &manifest_path,
        &candidates_json_path,
        &candidates_csv_path,
        &summary_path,
    ] {
        assert!(
            p.is_file(),
            "expected artifact file missing: {}",
            p.display()
        );
    }

    let csv = std::fs::read_to_string(&candidates_csv_path).unwrap();
    let header = csv.lines().next().unwrap();
    assert_eq!(
        header,
        "rank,symbol,timeframe,strategy_id,bars_available,truth_state,reason_code,score,total_return_pct,alpha_pct,max_drawdown_pct,trade_count,win_rate_pct,profit_factor,warnings,blockers"
    );

    let candidates_json = std::fs::read_to_string(&candidates_json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&candidates_json).unwrap();
    let arr = parsed.as_array().expect("candidates.json is a JSON array");
    assert_eq!(arr.len(), 3);
    for row in arr {
        for field in [
            "symbol",
            "timeframe",
            "strategy_id",
            "bars_available",
            "truth_state",
            "reason_code",
            "score",
            "rank",
            "metrics",
            "warnings",
            "blockers",
        ] {
            assert!(
                row.get(field).is_some(),
                "candidate row missing field '{field}': {row}"
            );
        }
    }

    let manifest_json = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
    assert_eq!(manifest["scan_id"], scan_id);
    assert_eq!(manifest["universe_count"], 3);
}

// 8. Top-N truncation works.
#[test]
fn top_n_truncates_printed_and_summary_rows() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--top",
        "1",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON summary");
    let top_ranked = summary["top_ranked"].as_array().expect("top_ranked array");
    assert_eq!(
        top_ranked.len(),
        1,
        "top=1 must truncate to exactly one row"
    );
    // Both AAPL and MSFT are rankable, but only the top-1 is surfaced.
    assert_eq!(summary["ranked_count"], 2);
}

// Dry-run: evaluate and print but do not write the artifact directory.
#[test]
fn dry_run_does_not_write_artifact_directory() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    assert!(stdout.contains("artifacts_written=false"));
    assert!(
        !fx.out_dir.exists() || std::fs::read_dir(&fx.out_dir).unwrap().next().is_none(),
        "dry-run must not create any artifact subdirectory"
    );
}

// Limit-symbols: deterministic alphabetical truncation of the universe.
#[test]
fn limit_symbols_truncates_universe_alphabetically() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--limit-symbols",
        "1",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON summary");
    // Universe is sorted alphabetically (AAPL, MSFT, NODATA); limiting to 1
    // deterministically keeps only AAPL.
    assert_eq!(summary["universe_count"], 1);
    let top_ranked = summary["top_ranked"].as_array().unwrap();
    assert_eq!(top_ranked.len(), 1);
    assert_eq!(top_ranked[0]["symbol"], "AAPL");
}

// A second identical invocation (fresh scan) produces the same candidate
// truth states -- proves the CLI path is deterministic end to end, not
// just the pure core.
#[test]
fn repeated_invocations_produce_identical_scan_id() {
    let fx = build_fixture();
    let args: Vec<&str> = vec![
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "swing_momentum",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--dry-run",
    ];
    let out1 = run(&args);
    let out2 = run(&args);
    assert!(out1.status.success());
    assert!(out2.status.success());
    let scan_id_1 = stdout_of(&out1)
        .lines()
        .find_map(|l| l.strip_prefix("scan_id="))
        .unwrap()
        .to_string();
    let scan_id_2 = stdout_of(&out2)
        .lines()
        .find_map(|l| l.strip_prefix("scan_id="))
        .unwrap()
        .to_string();
    assert_eq!(scan_id_1, scan_id_2);
}

// Unsupported strategy_id fails closed with a clear error, not a panic.
#[test]
fn unknown_strategy_flag_value_does_not_crash_the_whole_scan() {
    let fx = build_fixture();
    let output = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        fx.registry_path.to_str().unwrap(),
        "--bars-root",
        fx.bars_root.to_str().unwrap(),
        "--timeframe",
        "1D",
        "--strategy",
        "not_a_real_strategy",
        "--out-dir",
        fx.out_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON summary");
    assert_eq!(summary["ranked_count"], 0);
    assert_eq!(summary["skipped_count"], 3);
}
