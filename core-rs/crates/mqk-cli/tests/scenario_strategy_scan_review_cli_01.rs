//! STRATEGY-SCANNER-PROMOTION-01C — `mqk backtest review-scan` CLI scenario
//! tests.
//!
//! Every test here writes a small synthetic scanner artifact fixture to a
//! fresh OS temp directory and invokes the real `mqk-cli` binary as a
//! subprocess (same pattern as `scenario_cli_strategy_lab_evaluate.rs`). No
//! network call, no DB connection, no broker/provider environment variable
//! is required by any test in this file.

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mqk_cli_scan_review_{}_{}", label, Uuid::new_v4()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Two-candidate scanner artifact fixture: AAPL is a clean positive
/// candidate (eligible for `paper_candidate`); MSFT has a positive alpha
/// but a *negative* absolute total return (must never become
/// `paper_candidate`, regardless of its alpha).
fn candidates_json() -> String {
    r#"[
  {
    "symbol": "AAPL",
    "timeframe": "1D",
    "strategy_id": "swing_momentum",
    "bars_available": 400,
    "truth_state": "candidate_ranked",
    "reason_code": "ranked",
    "score": 8.5,
    "rank": 1,
    "metrics": {
      "total_return_pct": 12.0,
      "benchmark_return_pct": 4.0,
      "alpha_pct": 8.0,
      "max_drawdown_pct": 10.0,
      "trade_count": 20,
      "win_rate_pct": 55.0,
      "profit_factor": 1.8,
      "fill_count": 40,
      "bars_used": 400,
      "data_start_ts": 1700000000,
      "data_end_ts": 1735000000,
      "halted": false
    },
    "warnings": [],
    "blockers": []
  },
  {
    "symbol": "MSFT",
    "timeframe": "1D",
    "strategy_id": "swing_momentum",
    "bars_available": 400,
    "truth_state": "candidate_ranked",
    "reason_code": "ranked",
    "score": 6.0,
    "rank": 2,
    "metrics": {
      "total_return_pct": -3.0,
      "benchmark_return_pct": -9.0,
      "alpha_pct": 6.0,
      "max_drawdown_pct": 12.0,
      "trade_count": 15,
      "win_rate_pct": 50.0,
      "profit_factor": 1.4,
      "fill_count": 30,
      "bars_used": 400,
      "data_start_ts": 1700000000,
      "data_end_ts": 1735000000,
      "halted": false
    },
    "warnings": [],
    "blockers": []
  }
]
"#
    .to_string()
}

fn write_valid_scanner_artifact(dir: &Path, scan_id: &str) {
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "scan_id": "{scan_id}",
  "created_at_utc": "2026-07-01T00:00:00Z",
  "git_hash": "test",
  "registry_path": "config/instruments/equities.json",
  "bars_root": "exports/md_backup",
  "timeframe": "1D",
  "strategies": ["swing_momentum"],
  "universe_count": 2,
  "ranked_count": 2,
  "skipped_count": 0,
  "blockers": [],
  "warnings": []
}}
"#
        ),
    )
    .expect("write manifest.json");

    fs::write(dir.join("candidates.json"), candidates_json()).expect("write candidates.json");

    fs::write(
        dir.join("summary.json"),
        format!(
            r#"{{
  "scan_id": "{scan_id}",
  "universe_count": 2,
  "ranked_count": 2,
  "skipped_count": 0,
  "top_ranked": {},
  "top_skip_reasons": []
}}
"#,
            candidates_json()
        ),
    )
    .expect("write summary.json");
}

fn run_review_scan(artifact_dir: &Path, out_dir: &Path) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "review-scan",
            "--artifact-dir",
            &artifact_dir.to_string_lossy(),
            "--out-dir",
            &out_dir.to_string_lossy(),
            "--top",
            "50",
        ])
        // No DB, no provider, no broker environment variable is read by
        // this command -- proven by running with the common suspects
        // explicitly stripped and still succeeding.
        .env_remove("DATABASE_URL")
        .env_remove("MQK_DATABASE_URL")
        .env_remove("ALPACA_API_KEY")
        .env_remove("ALPACA_API_SECRET")
        .env_remove("ALPACA_BASE_URL")
        .env_remove("TWELVEDATA_API_KEY")
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .output()
        .expect("run mqk-cli backtest review-scan")
}

fn one_review_subdir(out_dir: &Path) -> PathBuf {
    let mut entries: Vec<PathBuf> = fs::read_dir(out_dir)
        .expect("read out_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one review subdirectory");
    entries.remove(0)
}

// 1 + 4. Reads a tiny scanner fixture artifact; positive candidate becomes
// `paper_candidate` under policy.
#[test]
fn review_scan_positive_candidate_becomes_paper_candidate() {
    let scan_dir = temp_dir("scan_valid");
    let scan_id = "11111111-1111-1111-1111-111111111111";
    write_valid_scanner_artifact(&scan_dir, scan_id);
    let out_dir = temp_dir("review_out_valid");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");

    assert!(stdout.contains(&format!("scanner_scan_id={scan_id}")));
    assert!(stdout.contains("candidate_count=2"));
    assert!(stdout.contains("paper_candidate_count=1"));
    assert!(stdout.contains("paper_candidate symbol=AAPL"));
    assert!(!stdout.contains("paper_candidate symbol=MSFT"));
}

// 2. Writes all four review files.
#[test]
fn review_scan_writes_all_four_artifact_files() {
    let scan_dir = temp_dir("scan_files");
    write_valid_scanner_artifact(&scan_dir, "22222222-2222-2222-2222-222222222222");
    let out_dir = temp_dir("review_out_files");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(output.status.success());

    let review_dir = one_review_subdir(&out_dir);
    assert!(review_dir.join("manifest.json").is_file());
    assert!(review_dir.join("review_decisions.json").is_file());
    assert!(review_dir.join("review_decisions.csv").is_file());
    assert!(review_dir.join("summary.json").is_file());
}

// 3. Negative-return positive-alpha candidate is blocked, not `paper_candidate`.
#[test]
fn review_scan_negative_return_candidate_not_paper_candidate() {
    let scan_dir = temp_dir("scan_negative");
    write_valid_scanner_artifact(&scan_dir, "33333333-3333-3333-3333-333333333333");
    let out_dir = temp_dir("review_out_negative");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(output.status.success());

    let review_dir = one_review_subdir(&out_dir);
    let decisions = fs::read_to_string(review_dir.join("review_decisions.json")).expect("read decisions");
    assert!(decisions.contains("\"symbol\": \"MSFT\""));
    // MSFT has alpha_pct=6.0 (positive) but total_return_pct=-3.0 (negative) --
    // must never be classified as paper_candidate.
    let msft_block_start = decisions.find("\"MSFT\"").expect("MSFT present");
    let msft_block = &decisions[msft_block_start..(msft_block_start + 400).min(decisions.len())];
    assert!(!msft_block.contains("\"paper_candidate\""));
    assert!(msft_block.contains("\"rejected\""));
}

// 5. Missing scanner files fail truthfully.
#[test]
fn review_scan_missing_scanner_files_fails_truthfully() {
    let empty_scan_dir = temp_dir("scan_missing");
    let out_dir = temp_dir("review_out_missing");

    let output = run_review_scan(&empty_scan_dir, &out_dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("manifest.json") || stderr.contains("read failed"));
}

// 6. Invalid scanner JSON fails truthfully.
#[test]
fn review_scan_invalid_scanner_json_fails_truthfully() {
    let scan_dir = temp_dir("scan_invalid");
    fs::write(scan_dir.join("manifest.json"), "{ not valid json").expect("write bad manifest");
    fs::write(scan_dir.join("summary.json"), "{}").expect("write summary");
    fs::write(scan_dir.join("candidates.json"), "[]").expect("write candidates");
    let out_dir = temp_dir("review_out_invalid");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("parse failed") || stderr.contains("manifest.json"));
}

// 7. CSV headers stable.
#[test]
fn review_scan_csv_header_is_stable() {
    let scan_dir = temp_dir("scan_csv");
    write_valid_scanner_artifact(&scan_dir, "44444444-4444-4444-4444-444444444444");
    let out_dir = temp_dir("review_out_csv");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(output.status.success());

    let review_dir = one_review_subdir(&out_dir);
    let csv = fs::read_to_string(review_dir.join("review_decisions.csv")).expect("read csv");
    let header = csv.lines().next().expect("header line");
    assert_eq!(
        header,
        "symbol,timeframe,strategy_id,scanner_rank,scanner_score,review_state,reason_codes,blockers,warnings"
    );
    assert!(csv.contains("paper_candidate"));
    assert!(csv.contains("rejected"));
}

// 8. Review id deterministic.
#[test]
fn review_scan_review_id_is_deterministic() {
    let scan_dir = temp_dir("scan_deterministic");
    write_valid_scanner_artifact(&scan_dir, "55555555-5555-5555-5555-555555555555");
    let out_dir_1 = temp_dir("review_out_det_1");
    let out_dir_2 = temp_dir("review_out_det_2");

    let output_1 = run_review_scan(&scan_dir, &out_dir_1);
    let output_2 = run_review_scan(&scan_dir, &out_dir_2);
    assert!(output_1.status.success());
    assert!(output_2.status.success());

    let stdout_1 = String::from_utf8(output_1.stdout).expect("utf8");
    let stdout_2 = String::from_utf8(output_2.stdout).expect("utf8");

    let review_id_line = |s: &str| {
        s.lines()
            .find(|l| l.starts_with("review_id="))
            .expect("review_id line present")
            .to_string()
    };
    assert_eq!(review_id_line(&stdout_1), review_id_line(&stdout_2));

    // The review subdirectory name (the review_id itself) must also match.
    let review_dir_1 = one_review_subdir(&out_dir_1);
    let review_dir_2 = one_review_subdir(&out_dir_2);
    assert_eq!(
        review_dir_1.file_name().unwrap(),
        review_dir_2.file_name().unwrap()
    );
}

// 9 + 10. No DB or provider/broker environment variable is required --
// proven by run_review_scan() above explicitly stripping the common
// suspects (DATABASE_URL / ALPACA_* / TWELVEDATA_* / KRAKEN_*) from every
// invocation in this file and every test still succeeding.
#[test]
fn review_scan_succeeds_without_db_or_provider_broker_env() {
    let scan_dir = temp_dir("scan_no_env");
    write_valid_scanner_artifact(&scan_dir, "66666666-6666-6666-6666-666666666666");
    let out_dir = temp_dir("review_out_no_env");

    let output = run_review_scan(&scan_dir, &out_dir);
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
