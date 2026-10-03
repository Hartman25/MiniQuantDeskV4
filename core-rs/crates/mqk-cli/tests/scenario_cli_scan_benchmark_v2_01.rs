//! `mqk backtest scan-strategies` / `review-scan --benchmark-policy`: the CLI
//! seam that makes Benchmark V2 the review alpha authority for exact-target
//! native candidates. Real binary, local fixtures only, no DB/provider env.

use std::path::{Path, PathBuf};
use std::process::Output;

use uuid::Uuid;

const V2_ID: &str = "capital_matched_exact_target_buy_hold_v1";
const STRATEGY: &str = "absolute_momentum_252";

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mqk_cli_scan_bv2_{label}_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn bars_csv(symbol: &str, count: usize, rising: bool) -> String {
    let mut out = String::from(
        "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
    );
    for i in 0..count as i64 {
        let c = if rising {
            100_000_000 + i * 50_000
        } else {
            200_000_000 - i * 50_000
        };
        let end_ts = 86_400 * (i + 1);
        out.push_str(&format!(
            "{symbol},1D,{end_ts},{c},{},{},{c},1000,t\n",
            c + 1_000_000,
            c - 1_000_000
        ));
    }
    out
}

struct Fx {
    _dir: PathBuf,
    registry: PathBuf,
    bars_root: PathBuf,
    out: PathBuf,
}

fn fixture(label: &str, rising: bool) -> Fx {
    let dir = temp_dir(label);
    let registry = dir.join("equities.json");
    std::fs::write(
        &registry,
        r#"[{"instrument_id":"equity:US:SPY","symbol":"SPY","asset_class":"equity","provider":"twelvedata","provider_symbol":"SPY","venue":"NYSE","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"fixture"}]"#,
    )
    .unwrap();
    let bars_root = dir.join("bars");
    std::fs::create_dir_all(bars_root.join("1D")).unwrap();
    std::fs::write(
        bars_root.join("1D").join("SPY_1D.csv"),
        bars_csv("SPY", 320, rising),
    )
    .unwrap();
    Fx {
        out: dir.join("scans"),
        _dir: dir,
        registry,
        bars_root,
    }
}

fn run(args: &[&str]) -> Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_DATABASE_URL")
        .env_remove("TWELVEDATA_API_KEY")
        .output()
        .expect("run mqk-cli")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn combined(o: &Output) -> String {
    format!("{}{}", stdout(o), String::from_utf8_lossy(&o.stderr))
}

fn scan(fx: &Fx, policy: Option<&str>) -> Output {
    let mut args = vec![
        "backtest".to_string(),
        "scan-strategies".to_string(),
        "--registry".to_string(),
        fx.registry.display().to_string(),
        "--bars-root".to_string(),
        fx.bars_root.display().to_string(),
        "--timeframe".to_string(),
        "1D".to_string(),
        "--strategy".to_string(),
        STRATEGY.to_string(),
        "--out-dir".to_string(),
        fx.out.display().to_string(),
    ];
    if let Some(p) = policy {
        args.push("--benchmark-policy".to_string());
        args.push(p.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

fn review(scan_dir: &str, out: &Path, policy: Option<&str>) -> Output {
    let mut args = vec![
        "backtest".to_string(),
        "review-scan".to_string(),
        "--artifact-dir".to_string(),
        scan_dir.to_string(),
        "--out-dir".to_string(),
        out.display().to_string(),
    ];
    if let Some(p) = policy {
        args.push("--benchmark-policy".to_string());
        args.push(p.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

fn kv(o: &Output, key: &str) -> String {
    stdout(o)
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key} in:\n{}", combined(o)))
        .trim()
        .to_string()
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// End to end through the real binary: V2 scan -> V2 review. The review row
/// carries the V2 evidence, both manifests record the policy, and the scan
/// identity differs from a legacy scan of the same inputs.
#[test]
fn v2_scan_and_review_end_to_end_records_the_policy() {
    let fx = fixture("e2e", true);
    let legacy = scan(&fx, None);
    assert!(legacy.status.success(), "{}", combined(&legacy));
    let v2 = scan(&fx, Some(V2_ID));
    assert!(v2.status.success(), "{}", combined(&v2));
    assert_ne!(kv(&legacy, "scan_id"), kv(&v2, "scan_id"));

    let v2_dir = kv(&v2, "artifacts_dir");
    let cands = read_json(&Path::new(&v2_dir).join("candidates.json"));
    let ev = &cands[0]["metrics"]["benchmark_v2"];
    assert_eq!(ev["policy_id"], V2_ID);
    assert_eq!(ev["candidate_target_qty_micros"], 1_000_000);
    assert_eq!(ev["benchmark_target_qty_micros"], 1_000_000);
    assert_eq!(cands[0]["metrics"]["alpha_pct"], ev["alpha_pct"]);
    assert_eq!(
        read_json(&Path::new(&v2_dir).join("manifest.json"))["benchmark_policy_id"],
        V2_ID
    );

    // The legacy scan artifact stays free of V2 vocabulary.
    let legacy_dir = kv(&legacy, "artifacts_dir");
    let legacy_text = std::fs::read_to_string(Path::new(&legacy_dir).join("candidates.json"))
        .unwrap()
        + &std::fs::read_to_string(Path::new(&legacy_dir).join("manifest.json")).unwrap();
    assert!(!legacy_text.contains("benchmark_v2") && !legacy_text.contains("benchmark_policy"));

    let rev_out = fx.out.join("reviews");
    let rev = review(&v2_dir, &rev_out, Some(V2_ID));
    assert!(rev.status.success(), "{}", combined(&rev));
    let rev_dir = PathBuf::from(kv(&rev, "review_artifact_dir"));
    assert_eq!(
        read_json(&rev_dir.join("manifest.json"))["benchmark_policy_id"],
        V2_ID
    );
    let rows = read_json(&rev_dir.join("review_decisions.json"));
    assert_eq!(rows[0]["benchmark_v2"]["policy_id"], V2_ID);
    assert_ne!(rows[0]["review_state"], "blocked", "{rows}");
    assert_eq!(
        read_json(&rev_dir.join("manifest.json"))["policy_min_alpha_pct"],
        0.0
    );
}

/// A legacy scan can never be reviewed as V2 authority, a V2 scan is not
/// reviewable under the legacy contract, and an unrecognized policy id is
/// refused by both commands.
#[test]
fn policy_mismatch_and_unknown_policy_are_refused_by_the_cli() {
    let fx = fixture("mismatch", true);
    let legacy = scan(&fx, None);
    let v2 = scan(&fx, Some(V2_ID));
    let (legacy_dir, v2_dir) = (kv(&legacy, "artifacts_dir"), kv(&v2, "artifacts_dir"));
    let rev_out = fx.out.join("reviews");

    let r = review(&legacy_dir, &rev_out, Some(V2_ID));
    assert!(!r.status.success(), "legacy scan reviewed as V2 must fail");
    assert!(combined(&r).contains("does not match"), "{}", combined(&r));

    let r = review(&v2_dir, &rev_out, None);
    assert!(!r.status.success(), "V2 scan reviewed as legacy must fail");

    let r = review(&v2_dir, &rev_out, Some("buy_and_hold_legacy"));
    assert!(!r.status.success());
    assert!(
        combined(&r).contains(V2_ID),
        "message names the accepted id"
    );

    let r = scan(&fx, Some("buy_and_hold_legacy"));
    assert!(!r.status.success());
    assert!(combined(&r).contains(V2_ID));
    assert!(
        !rev_out.exists(),
        "no review artifact is written on refusal"
    );
}

/// A candidate whose Benchmark V2 cannot be computed (never takes a position)
/// is reported `benchmark_unavailable` and reviewed `blocked` -- never judged
/// against the legacy benchmark.
#[test]
fn v2_scan_with_no_computable_benchmark_is_blocked_not_legacy_judged() {
    let fx = fixture("flat", false);
    let v2 = scan(&fx, Some(V2_ID));
    assert!(v2.status.success(), "{}", combined(&v2));
    let v2_dir = kv(&v2, "artifacts_dir");
    let cands = read_json(&Path::new(&v2_dir).join("candidates.json"));
    assert_eq!(cands[0]["reason_code"], "benchmark_unavailable");
    assert_eq!(cands[0]["truth_state"], "metrics_unavailable");
    assert!(cands[0]["metrics"]["alpha_pct"].is_null());

    let rev = review(&v2_dir, &fx.out.join("reviews"), Some(V2_ID));
    assert!(rev.status.success(), "{}", combined(&rev));
    let rows =
        read_json(&PathBuf::from(kv(&rev, "review_artifact_dir")).join("review_decisions.json"));
    assert_eq!(rows[0]["review_state"], "blocked");
    assert_eq!(rows[0]["reason_codes"][0], "not_candidate_ranked");
}
