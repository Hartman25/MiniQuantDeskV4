//! IR-BV2-01 end to end through the real CLI: `scan-strategies
//! --benchmark-policy <V2>` run with the same config flags as a canonical
//! `backtest csv` must emit candidate evidence carrying that Backtest run's
//! exact `run_id` and `config_hash` -- and without those flags it must not.

use std::path::{Path, PathBuf};

use uuid::Uuid;

const V2_POLICY: &str = "capital_matched_exact_target_buy_hold_v1";
const STRATEGY: &str = "absolute_momentum_252";
const DAY: i64 = 86_400;

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> anyhow::Result<TmpDir> {
    let root = std::env::temp_dir().join(format!("mqk_cli_scan_v2_cfg_{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join("bars").join("1D"))?;
    std::fs::write(
        root.join("registry.json"),
        r#"[{"instrument_id":"equity:US:SPY","symbol":"SPY","asset_class":"equity","provider":"alpaca","provider_symbol":"SPY","venue":"NYSE","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"fixture"}]"#,
    )?;
    let mut csv =
        String::from("symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume\n");
    for i in 0..320i64 {
        let c = 100_000_000 + i * 50_000;
        csv.push_str(&format!(
            "SPY,{},{},{},{},{},1000\n",
            DAY * (i + 1),
            c,
            c + 1_000_000,
            c - 1_000_000,
            c
        ));
    }
    std::fs::write(root.join("bars").join("1D").join("SPY_1D.csv"), csv)?;
    Ok(TmpDir(root))
}

fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .output()
        .unwrap()
}

fn run_ok(args: &[&str]) -> String {
    let out = run(args);
    assert!(
        out.status.success(),
        "mqk-cli {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn kv(stdout: &str, key: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key}= in:\n{stdout}"))
        .trim()
        .to_string()
}

/// Run the scan, return (candidate_run_id, candidate_config_id).
fn scan_identity(root: &Path, extra: &[&str]) -> (String, String) {
    let registry = root.join("registry.json").display().to_string();
    let bars_root = root.join("bars").display().to_string();
    let out_dir = root
        .join(format!("scans_{}", Uuid::new_v4()))
        .display()
        .to_string();
    let mut args = vec![
        "backtest",
        "scan-strategies",
        "--registry",
        &registry,
        "--bars-root",
        &bars_root,
        "--timeframe",
        "1D",
        "--strategy",
        STRATEGY,
        "--out-dir",
        &out_dir,
        "--benchmark-policy",
        V2_POLICY,
    ];
    args.extend_from_slice(extra);
    let stdout = run_ok(&args);
    let dir = kv(&stdout, "artifacts_dir");
    let cands: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&dir).join("candidates.json")).unwrap(),
    )
    .unwrap();
    let ev = &cands[0]["metrics"]["benchmark_v2"];
    assert!(ev.is_object(), "V2 evidence expected: {}", cands[0]);
    (
        ev["candidate_run_id"].as_str().unwrap().to_string(),
        ev["candidate_config_id"].as_str().unwrap().to_string(),
    )
}

const CANONICAL_FLAGS: [&str; 8] = [
    "--integrity-calendar",
    "us-equity-regular",
    "--integrity-stale-threshold-ticks",
    "259200",
    "--integrity-gap-tolerance-bars",
    "3",
    "--initial-cash-micros",
    "100000000000",
];

#[test]
fn scan_with_the_backtest_csv_flags_carries_the_backtest_run_identity() -> anyhow::Result<()> {
    let fx = fixture()?;
    let bars =
        fx.0.join("bars")
            .join("1D")
            .join("SPY_1D.csv")
            .display()
            .to_string();
    let mut csv_args = vec![
        "backtest",
        "csv",
        "--bars",
        &bars,
        "--strategy",
        STRATEGY,
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
    ];
    csv_args.extend_from_slice(&CANONICAL_FLAGS);
    let stdout = run_ok(&csv_args);
    let (run_id, config_hash) = (kv(&stdout, "run_id"), kv(&stdout, "config_hash"));
    // The corrected Batch 01 canonical Backtest config (a694f4bb...): these
    // flags, via `backtest csv`, are exactly what produced it.
    assert_eq!(config_hash, "a694f4bb-a933-5efe-b7ce-706a757febb5");

    // Same flags -> same run identity.
    let (cand_run, cand_cfg) = scan_identity(&fx.0, &CANONICAL_FLAGS);
    assert_eq!(cand_cfg, config_hash);
    assert_eq!(cand_run, run_id);

    // Flag defaults mirror `backtest csv`'s defaults (no integrity flags on
    // either side): still the same identity.
    let stdout = run_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars,
        "--strategy",
        STRATEGY,
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
    ]);
    let (cand_run, cand_cfg) = scan_identity(&fx.0, &[]);
    assert_eq!(cand_cfg, kv(&stdout, "config_hash"));
    assert_eq!(cand_run, kv(&stdout, "run_id"));
    Ok(())
}

#[test]
fn scan_under_different_flags_does_not_carry_the_backtest_run_identity() -> anyhow::Result<()> {
    let fx = fixture()?;
    let bars =
        fx.0.join("bars")
            .join("1D")
            .join("SPY_1D.csv")
            .display()
            .to_string();
    let mut csv_args = vec![
        "backtest",
        "csv",
        "--bars",
        &bars,
        "--strategy",
        STRATEGY,
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
    ];
    csv_args.extend_from_slice(&CANONICAL_FLAGS);
    let stdout = run_ok(&csv_args);
    let (run_id, config_hash) = (kv(&stdout, "run_id"), kv(&stdout, "config_hash"));

    // Scanner run under the (default) integrity posture instead.
    let (cand_run, cand_cfg) = scan_identity(&fx.0, &[]);
    assert_ne!(cand_cfg, config_hash);
    assert_ne!(cand_run, run_id);

    // A different capital is a different identity too.
    let (cand_run, cand_cfg) = scan_identity(
        &fx.0,
        &[
            "--integrity-calendar",
            "us-equity-regular",
            "--integrity-stale-threshold-ticks",
            "259200",
            "--integrity-gap-tolerance-bars",
            "3",
            "--initial-cash-micros",
            "50000000000",
        ],
    );
    assert_ne!(cand_cfg, config_hash);
    assert_ne!(cand_run, run_id);
    Ok(())
}

#[test]
fn config_flags_without_the_v2_policy_are_refused() -> anyhow::Result<()> {
    let fx = fixture()?;
    let registry = fx.0.join("registry.json").display().to_string();
    let bars_root = fx.0.join("bars").display().to_string();
    let out_dir = fx.0.join("scans_legacy").display().to_string();
    let out = run(&[
        "backtest",
        "scan-strategies",
        "--registry",
        &registry,
        "--bars-root",
        &bars_root,
        "--timeframe",
        "1D",
        "--strategy",
        STRATEGY,
        "--out-dir",
        &out_dir,
        "--integrity-gap-tolerance-bars",
        "3",
    ]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("accepted only with --benchmark-policy"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!Path::new(&out_dir).exists(), "no artifacts on refusal");
    Ok(())
}
