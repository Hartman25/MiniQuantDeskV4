//! FixedInitialCapitalFractionV1 through the real CLI: explicit selection only,
//! strict flag validation, csv/scan identity parity, benchmark correspondence,
//! and unchanged legacy identity.

use std::path::{Path, PathBuf};

use uuid::Uuid;

const CF: &str = "fixed_initial_capital_fraction_v1";
const FIXED: &str = "fixed_quantity_v1";
const V2_POLICY: &str = "capital_matched_exact_target_buy_hold_v1";
const CF_BENCH: &str = "capital_fraction_matched_passive_buy_hold_v1";
const STRATEGY: &str = "absolute_momentum_252";
const DAY: i64 = 86_400;
const CANONICAL_LEGACY_CONFIG: &str = "a694f4bb-a933-5efe-b7ce-706a757febb5";

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> anyhow::Result<TmpDir> {
    let root = std::env::temp_dir().join(format!("mqk_cli_sizing_{}", Uuid::new_v4()));
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

fn bars_path(root: &Path) -> String {
    root.join("bars")
        .join("1D")
        .join("SPY_1D.csv")
        .display()
        .to_string()
}

fn csv_args<'a>(bars: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "backtest",
        "csv",
        "--bars",
        bars,
        "--strategy",
        STRATEGY,
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
    ];
    a.extend_from_slice(extra);
    a
}

fn scan_args(root: &Path, out_dir: &str, benchmark: Option<&str>, extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = [
        "backtest",
        "scan-strategies",
        "--registry",
        &root.join("registry.json").display().to_string(),
        "--bars-root",
        &root.join("bars").display().to_string(),
        "--timeframe",
        "1D",
        "--strategy",
        STRATEGY,
        "--out-dir",
        out_dir,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(b) = benchmark {
        a.push("--benchmark-policy".into());
        a.push(b.into());
    }
    a.extend(extra.iter().map(|s| s.to_string()));
    a
}

fn run_scan(root: &Path, benchmark: Option<&str>, extra: &[&str]) -> std::process::Output {
    let out_dir = root
        .join(format!("scans_{}", Uuid::new_v4()))
        .display()
        .to_string();
    let args = scan_args(root, &out_dir, benchmark, extra);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// CF scan -> (candidate_run_id, candidate_config_id, evidence).
fn cf_scan_identity(root: &Path, extra: &[&str]) -> (String, String, serde_json::Value) {
    let out = run_scan(root, Some(CF_BENCH), extra);
    assert!(out.status.success(), "scan failed: {}", stderr(&out));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let dir = kv(&stdout, "artifacts_dir");
    let cands: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&dir).join("candidates.json")).unwrap(),
    )
    .unwrap();
    let ev = cands[0]["metrics"]["benchmark_capital_fraction"].clone();
    assert!(ev.is_object(), "CF evidence expected: {}", cands[0]);
    assert!(cands[0]["metrics"]["benchmark_v2"].is_null());
    (
        ev["candidate_run_id"].as_str().unwrap().to_string(),
        ev["candidate_config_id"].as_str().unwrap().to_string(),
        ev,
    )
}

#[test]
fn csv_flag_combinations_are_validated_without_a_default_fraction() -> anyhow::Result<()> {
    let fx = fixture()?;
    let bars = bars_path(&fx.0);
    let refused: &[(&[&str], &str)] = &[
        (
            &["--allocation-fraction-bps", "2500"],
            "requires --sizing-policy",
        ),
        (
            &[
                "--sizing-policy",
                FIXED,
                "--allocation-fraction-bps",
                "2500",
            ],
            "not valid with",
        ),
        (&["--sizing-policy", CF], "no default"),
        (
            &["--sizing-policy", CF, "--allocation-fraction-bps", "0"],
            "invalid --allocation-fraction-bps",
        ),
        (
            &["--sizing-policy", CF, "--allocation-fraction-bps", "10001"],
            "invalid --allocation-fraction-bps",
        ),
        (
            &["--sizing-policy", CF, "--allocation-fraction-bps=-5"],
            "invalid --allocation-fraction-bps",
        ),
        (
            &["--sizing-policy", "unknown_policy_v9"],
            "not an accepted policy",
        ),
        (
            &[
                "--sizing-policy",
                CF,
                "--allocation-fraction-bps",
                "2500",
                "--target-qty",
                "3",
            ],
            "--target-qty",
        ),
    ];
    for (extra, needle) in refused {
        let out = run(&csv_args(&bars, extra));
        assert!(!out.status.success(), "must refuse {extra:?}");
        assert!(
            stderr(&out).contains(needle),
            "{extra:?}: expected '{needle}' in: {}",
            stderr(&out)
        );
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("run_id="),
            "no run on refusal {extra:?}"
        );
    }
    // Malformed (non-integer) bps never reaches the policy.
    for bad in ["25.5", "abc", "1e3"] {
        let out = run(&csv_args(
            &bars,
            &["--sizing-policy", CF, "--allocation-fraction-bps", bad],
        ));
        assert!(!out.status.success(), "must refuse bps '{bad}'");
    }
    Ok(())
}

#[test]
fn csv_legacy_identity_is_unchanged_and_policy_changes_identity() -> anyhow::Result<()> {
    let fx = fixture()?;
    let bars = bars_path(&fx.0);
    let canonical: [&str; 8] = [
        "--integrity-calendar",
        "us-equity-regular",
        "--integrity-stale-threshold-ticks",
        "259200",
        "--integrity-gap-tolerance-bars",
        "3",
        "--initial-cash-micros",
        "100000000000",
    ];
    let legacy = run_ok(&csv_args(&bars, &canonical));
    assert_eq!(kv(&legacy, "config_hash"), CANONICAL_LEGACY_CONFIG);

    // Explicit fixed policy == omitted policy (historical identity).
    let mut with_fixed = canonical.to_vec();
    with_fixed.extend(["--sizing-policy", FIXED]);
    let fixed = run_ok(&csv_args(&bars, &with_fixed));
    assert_eq!(kv(&fixed, "config_hash"), CANONICAL_LEGACY_CONFIG);
    assert_eq!(kv(&fixed, "run_id"), kv(&legacy, "run_id"));

    let cf_id = |bps: &str| {
        let mut a = canonical.to_vec();
        a.extend(["--sizing-policy", CF, "--allocation-fraction-bps", bps]);
        let out = run_ok(&csv_args(&bars, &a));
        (kv(&out, "config_hash"), kv(&out, "run_id"))
    };
    let (c2500, r2500) = cf_id("2500");
    let (c5000, r5000) = cf_id("5000");
    assert_ne!(c2500, CANONICAL_LEGACY_CONFIG);
    assert_ne!(r2500, kv(&legacy, "run_id"));
    assert_ne!(c2500, c5000, "fraction must change identity");
    assert_ne!(r2500, r5000);
    // Deterministic: the same flags reproduce the same identity.
    assert_eq!(cf_id("2500"), (c2500, r2500));
    Ok(())
}

#[test]
fn cf_scan_and_cf_csv_with_identical_flags_share_identity() -> anyhow::Result<()> {
    let fx = fixture()?;
    let bars = bars_path(&fx.0);
    let flags = [
        "--sizing-policy",
        CF,
        "--allocation-fraction-bps",
        "2500",
        "--integrity-calendar",
        "us-equity-regular",
        "--integrity-stale-threshold-ticks",
        "259200",
        "--integrity-gap-tolerance-bars",
        "3",
        "--initial-cash-micros",
        "100000000000",
    ];
    let csv = run_ok(&csv_args(&bars, &flags));
    let (cand_run, cand_cfg, ev) = cf_scan_identity(&fx.0, &flags);
    assert_eq!(cand_cfg, kv(&csv, "config_hash"));
    assert_eq!(cand_run, kv(&csv, "run_id"));
    assert_eq!(ev["sizing_policy_id"], CF);
    assert_eq!(ev["allocation_fraction_bps"], 2500);
    assert_eq!(ev["initial_allocated_capital_micros"], 100_000_000_000i64);
    // 25% of $100k = $25k budget; the causal close is >= $100 -> whole shares.
    let budget = ev["position_budget_micros"].as_i64().unwrap();
    assert_eq!(budget, 25_000_000_000);
    let px = ev["reference_price_micros"].as_i64().unwrap();
    let qty = ev["candidate_target_qty_micros"].as_i64().unwrap();
    assert_eq!(qty, (budget / px) * 1_000_000);

    // Caps are part of the config: they change identity and apply to both.
    let capped_flags = {
        let mut f = flags.to_vec();
        f.extend(["--max-target-qty", "10"]);
        f
    };
    let (cand_run_c, cand_cfg_c, ev_c) = cf_scan_identity(&fx.0, &capped_flags);
    assert_ne!(cand_cfg_c, cand_cfg);
    assert_ne!(cand_run_c, cand_run);
    assert!(ev_c["candidate_target_qty_micros"].as_i64().unwrap() <= 10_000_000);
    let csv_capped = run_ok(&csv_args(&bars, &capped_flags));
    assert_eq!(cand_cfg_c, kv(&csv_capped, "config_hash"));
    assert_eq!(cand_run_c, kv(&csv_capped, "run_id"));
    Ok(())
}

#[test]
fn scan_requires_the_benchmark_matching_the_sizing_policy() -> anyhow::Result<()> {
    let fx = fixture()?;
    let cf_flags = ["--sizing-policy", CF, "--allocation-fraction-bps", "2500"];

    // CF sizing with Benchmark V2, with no benchmark, or with the legacy path.
    for bench in [Some(V2_POLICY), None] {
        let out = run_scan(&fx.0, bench, &cf_flags);
        assert!(
            !out.status.success(),
            "CF sizing + {bench:?} must be refused"
        );
    }
    // Fixed (explicit or omitted) sizing with the CF benchmark.
    for extra in [vec![], vec!["--sizing-policy", FIXED]] {
        let out = run_scan(&fx.0, Some(CF_BENCH), &extra);
        assert!(
            !out.status.success(),
            "fixed sizing + CF benchmark: {extra:?}"
        );
        assert!(
            stderr(&out).contains("requires --benchmark-policy"),
            "{}",
            stderr(&out)
        );
    }
    // CF benchmark without an explicit fraction: no default.
    let out = run_scan(&fx.0, Some(CF_BENCH), &["--sizing-policy", CF]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no default"), "{}", stderr(&out));

    // Caps are not accepted on the legacy V2 scan.
    let out = run_scan(&fx.0, Some(V2_POLICY), &["--max-target-qty", "5"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("only with the capital-fraction policy"),
        "{}",
        stderr(&out)
    );

    // Sizing flags are refused on the legacy (no benchmark policy) scan.
    let out = run_scan(&fx.0, None, &["--sizing-policy", FIXED]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("accepted only with --benchmark-policy"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn legacy_v2_scan_still_carries_v2_evidence_not_capital_fraction() -> anyhow::Result<()> {
    let fx = fixture()?;
    let out = run_scan(&fx.0, Some(V2_POLICY), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let dir = kv(&stdout, "artifacts_dir");
    let cands: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&dir).join("candidates.json")).unwrap(),
    )
    .unwrap();
    assert!(cands[0]["metrics"]["benchmark_v2"].is_object());
    assert!(cands[0]["metrics"]["benchmark_capital_fraction"].is_null());
    Ok(())
}
