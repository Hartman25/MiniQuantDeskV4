//! BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: proves the real, ordinary
//! production workflow -- not a hand-built fixture -- generates the
//! complete Backtest-side promotion evidence set (`stress_suite.json` +
//! `robustness_gauntlet.json`, the latter finalized with a real DSR/PBO
//! sensitivity result merged in via a real Python subprocess against a
//! real disposable SQLite registry).
//!
//! Every test here runs the ACTUAL compiled `mqk-cli` binary (`mqk backtest
//! csv` then `mqk backtest finalize-robustness-sensitivity`), exactly the
//! two commands a real operator would run -- never calling `mqk_artifacts`
//! writers directly. Requires a real `python` with research-py's runtime
//! deps (pandas/numpy) importable; `#[ignore]`d like every other
//! environment-dependent test in this crate.

use std::path::{Path, PathBuf};

use uuid::Uuid;

fn research_py_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("research-py")
}

fn py_str_literal(s: &str) -> String {
    serde_json::to_string(s).expect("string always serializes")
}

/// Real Python subprocess, `ResearchResultStore` directly -- mirrors
/// `mqk-backtest`'s own `scenario_dsr_pbo_sensitivity_01.rs` fixture
/// builder. Registers ONE trial with zero attempts (genuinely registered,
/// but not part of any judged comparison scope) under `strategy_id`.
///
/// FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: also builds a REAL
/// whole-experiment-scoped judge over `exp.cli_it` (idempotent, safe even
/// when zero trials are economically evaluable) and returns its durable
/// `judge_artifact_sha256`, which `dsr_pbo_sensitivity_scenario` now
/// requires as a caller-supplied authority -- mirrors
/// `mqk-backtest`'s `scenario_dsr_pbo_sensitivity_01.rs` fixture exactly,
/// never a hand-built/fake sha256.
fn register_never_attempted_trial(registry_db: &Path, trial_id: &str, strategy_id: &str) -> String {
    let script = format!(
        "from pathlib import Path\n\
         from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes\n\
         from mqk_research.exp_distributed.storage import ResearchResultStore\n\
         from mqk_research.ml.economic_walkforward import PROTOCOL_ID\n\
         from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge\n\
         store = ResearchResultStore(Path({db}))\n\
         store.register_hypothesis(hypothesis_id='hyp', experiment_id='exp.cli_it')\n\
         store.register_trial(\n\
         \ttrial_id={trial_id}, experiment_id='exp.cli_it', hypothesis_id='hyp',\n\
         \tstrategy_id={strategy_id}, protocol_id=PROTOCOL_ID, identity={{'minimal': True}},\n\
         )\n\
         artifact = build_multiple_testing_judge(experiment_id='exp.cli_it', registry_db=Path({db}))\n\
         print(sha256_bytes(canonical_json(artifact).encode('utf-8')), end='')\n",
        db = py_str_literal(&registry_db.display().to_string()),
        trial_id = py_str_literal(trial_id),
        strategy_id = py_str_literal(strategy_id),
    );
    let src_dir = research_py_root().join("src");
    let output = std::process::Command::new("python")
        .env("PYTHONPATH", &src_dir)
        .arg("-c")
        .arg(&script)
        .output()
        .expect("failed to spawn python fixture-builder");
    assert!(
        output.status.success(),
        "fixture-builder script failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_bars_csv(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mqk_cli_promo_evidence_{tag}_{}.csv", Uuid::new_v4()));
    let mut body = String::from("symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume\n");
    // 60 daily bars starting 2024-01-02 -- spans 2+ calendar months, a mild
    // uptrend so the real stress/robustness scenarios have genuine
    // (non-degenerate) equity/drawdown data to compute against.
    let start: i64 = 1_704_229_200; // 2024-01-02T21:00:00Z
    let mut price = 500.0_f64;
    for i in 0..60 {
        let ts = start + i * 86_400;
        price *= 1.002;
        let o = (price * 1_000_000.0) as i64;
        let h = (price * 1.01 * 1_000_000.0) as i64;
        let l = (price * 0.99 * 1_000_000.0) as i64;
        let c = (price * 1_000_000.0) as i64;
        body.push_str(&format!("SPY,{ts},{o},{h},{l},{c},10000\n"));
    }
    std::fs::write(&path, body).expect("write bars csv");
    path
}

fn fresh_out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mqk_cli_promo_evidence_out_{tag}_{}", Uuid::new_v4()))
}

fn run_cli(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .output()
        .expect("failed to spawn mqk-cli")
}

fn run_cli_ok(args: &[&str]) -> String {
    let output = run_cli(args);
    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn extract_run_id(stdout: &str) -> String {
    for line in stdout.lines() {
        if let Some(id) = line.strip_prefix("run_id=") {
            return id.to_string();
        }
    }
    panic!("stdout did not contain run_id=...; stdout:\n{stdout}");
}

/// The real, ordinary `mqk backtest csv --out-dir` workflow produces
/// `stress_suite.json` AND `robustness_gauntlet.json` alongside
/// `backtest_report.json`/`manifest.json`/`audit.jsonl` -- no manual
/// fixture construction, no direct `mqk_artifacts` writer calls.
#[test]
fn backtest_csv_produces_stress_and_robustness_evidence() {
    let bars = write_bars_csv("basic");
    let out_dir = fresh_out_dir("basic");

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars.to_string_lossy(),
        "--strategy",
        "swing_momentum",
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
        "--integrity-stale-threshold-ticks",
        "200000",
        "--out-dir",
        &out_dir.to_string_lossy(),
    ]);
    assert!(stdout.contains("promotion_evidence_written=true"));

    let run_id = extract_run_id(&stdout);
    let run_dir = out_dir.join(&run_id);
    assert!(run_dir.join("stress_suite.json").exists(), "stress_suite.json must be real");
    assert!(
        run_dir.join("robustness_gauntlet.json").exists(),
        "robustness_gauntlet.json must be real"
    );

    let gauntlet: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("robustness_gauntlet.json")).unwrap())
            .unwrap();
    assert_eq!(gauntlet["scenarios"].as_array().unwrap().len(), 6);
    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01 / FINAL-P9-ROBUSTNESS-SEMANTICS-01
    // added two more required-but-separately-finalized deferred scenarios
    // (p7a_p7b_economic_replay_stress, genuine_shuffled_placebo) alongside
    // dsr_pbo_sensitivity, so 3 -- not 1 -- remain deferred right after the
    // real backtest run.
    assert_eq!(
        gauntlet["deferred"].as_array().unwrap().len(),
        3,
        "dsr_pbo_sensitivity, p7a_p7b_economic_replay_stress, genuine_shuffled_placebo not yet finalized"
    );

    let audit = std::fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    assert!(audit.contains("backtest_run_completed"));
    assert!(audit.contains("stress_suite_completed"));
    assert!(audit.contains("robustness_gauntlet_completed"));
}

/// The full two-phase production chain: real backtest execution, then real
/// DSR/PBO sensitivity finalization against a real (never-attempted, so
/// genuinely inapplicable) Research trial -- proving the exact CLI
/// invocations an operator would run end to end.
#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn backtest_evidence_finalizes_with_real_dsr_pbo_sensitivity() {
    let bars = write_bars_csv("finalize");
    let out_dir = fresh_out_dir("finalize");

    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars.to_string_lossy(),
        "--strategy",
        "swing_momentum",
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
        "--integrity-stale-threshold-ticks",
        "200000",
        "--out-dir",
        &out_dir.to_string_lossy(),
    ]);
    let run_id = extract_run_id(&stdout);

    let registry_db = std::env::temp_dir().join(format!("mqk_cli_promo_evidence_registry_{}.sqlite3", Uuid::new_v4()));
    let judge_sha256 = register_never_attempted_trial(&registry_db, "cli_it_trial", "swing_momentum");

    let finalize_stdout = run_cli_ok(&[
        "backtest",
        "finalize-robustness-sensitivity",
        "--artifact-root",
        &out_dir.to_string_lossy(),
        "--run-id",
        &run_id,
        "--registry-db",
        &registry_db.to_string_lossy(),
        "--trial-id",
        "cli_it_trial",
        "--judge-artifact-sha256",
        &judge_sha256,
        "--research-py-root",
        &research_py_root().to_string_lossy(),
        "--python",
        "python",
        "--block-counts",
        "8,10",
        "--dsr-max-sensitivity-range",
        "0.25",
        "--pbo-max-sensitivity-range",
        "0.25",
    ]);
    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01 / FINAL-P9-ROBUSTNESS-SEMANTICS-01:
    // two more required scenarios (p7a_p7b_economic_replay_stress,
    // genuine_shuffled_placebo) now have their own separate finalize
    // commands and remain deferred after this one -- `is_complete()`
    // correctly reports false until every required scenario is finalized.
    assert!(finalize_stdout.contains("is_complete=false"));
    assert!(finalize_stdout.contains("scenarios_run=7"));

    let run_dir = out_dir.join(&run_id);
    let gauntlet: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("robustness_gauntlet.json")).unwrap())
            .unwrap();
    assert_eq!(gauntlet["scenarios"].as_array().unwrap().len(), 7);
    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01 / FINAL-P9-ROBUSTNESS-SEMANTICS-01
    // added two more required-but-separately-finalized deferred scenarios
    // (p7a_p7b_economic_replay_stress, genuine_shuffled_placebo); this
    // command only ever merges dsr_pbo_sensitivity, so 2 remain deferred.
    assert_eq!(gauntlet["deferred"].as_array().unwrap().len(), 2);

    let audit = std::fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    assert!(audit.contains("robustness_gauntlet_finalized"));
}

/// Finalization is idempotent -- an identical replay is a real no-op
/// (`exit 0`, no duplicated scenario, no second finalized audit event).
#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn finalize_is_idempotent_across_two_runs() {
    let bars = write_bars_csv("idem");
    let out_dir = fresh_out_dir("idem");
    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars.to_string_lossy(),
        "--strategy",
        "swing_momentum",
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
        "--integrity-stale-threshold-ticks",
        "200000",
        "--out-dir",
        &out_dir.to_string_lossy(),
    ]);
    let run_id = extract_run_id(&stdout);
    let registry_db = std::env::temp_dir().join(format!("mqk_cli_promo_evidence_registry_{}.sqlite3", Uuid::new_v4()));
    let judge_sha256 = register_never_attempted_trial(&registry_db, "cli_it_idem_trial", "swing_momentum");

    let out_dir_s = out_dir.to_string_lossy().to_string();
    let registry_db_s = registry_db.to_string_lossy().to_string();
    let research_py_root_s = research_py_root().to_string_lossy().to_string();
    let args = [
        "backtest",
        "finalize-robustness-sensitivity",
        "--artifact-root",
        &out_dir_s,
        "--run-id",
        run_id.as_str(),
        "--registry-db",
        &registry_db_s,
        "--trial-id",
        "cli_it_idem_trial",
        "--judge-artifact-sha256",
        judge_sha256.as_str(),
        "--research-py-root",
        &research_py_root_s,
        "--python",
        "python",
        "--block-counts",
        "8,10",
        "--dsr-max-sensitivity-range",
        "0.25",
        "--pbo-max-sensitivity-range",
        "0.25",
    ];
    run_cli_ok(&args);
    run_cli_ok(&args);

    let run_dir = out_dir.join(&run_id);
    let audit = std::fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let finalized_count = audit.lines().filter(|l| l.contains("robustness_gauntlet_finalized")).count();
    assert_eq!(finalized_count, 1, "replay must not append a second finalized audit event");
}

/// A Research trial genuinely registered under a DIFFERENT strategy is
/// refused, never silently merged.
#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn finalize_rejects_research_trial_strategy_mismatch() {
    let bars = write_bars_csv("mismatch");
    let out_dir = fresh_out_dir("mismatch");
    let stdout = run_cli_ok(&[
        "backtest",
        "csv",
        "--bars",
        &bars.to_string_lossy(),
        "--strategy",
        "swing_momentum",
        "--symbol",
        "SPY",
        "--timeframe-secs",
        "86400",
        "--integrity-stale-threshold-ticks",
        "200000",
        "--out-dir",
        &out_dir.to_string_lossy(),
    ]);
    let run_id = extract_run_id(&stdout);
    let registry_db = std::env::temp_dir().join(format!("mqk_cli_promo_evidence_registry_{}.sqlite3", Uuid::new_v4()));
    let judge_sha256 =
        register_never_attempted_trial(&registry_db, "cli_it_mismatch_trial", "a_totally_different_strategy");

    let output = run_cli(&[
        "backtest",
        "finalize-robustness-sensitivity",
        "--artifact-root",
        &out_dir.to_string_lossy(),
        "--run-id",
        &run_id,
        "--registry-db",
        &registry_db.to_string_lossy(),
        "--trial-id",
        "cli_it_mismatch_trial",
        "--judge-artifact-sha256",
        &judge_sha256,
        "--research-py-root",
        &research_py_root().to_string_lossy(),
        "--python",
        "python",
        "--block-counts",
        "8,10",
        "--dsr-max-sensitivity-range",
        "0.25",
        "--pbo-max-sensitivity-range",
        "0.25",
    ]);

    // The mismatch itself is a `passed:false` scenario outcome (printed to
    // stdout) that finalize() then durably merges as a real recorded
    // failure -- the command still exits 0 (a genuine result was recorded,
    // not a crash), but the merged evidence must show the rejection.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Research trial mismatch"), "stdout:\n{stdout}");
    assert!(stdout.contains("applicable=true"));
    assert!(stdout.contains("passed=false"));

    let run_dir = out_dir.join(&run_id);
    let gauntlet: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("robustness_gauntlet.json")).unwrap())
            .unwrap();
    let sensitivity = gauntlet["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "dsr_pbo_sensitivity")
        .expect("dsr_pbo_sensitivity must be recorded even as a failure");
    assert_eq!(sensitivity["passed"], false);
    assert!(sensitivity["reason"].as_str().unwrap().contains("Research trial mismatch"));
}

/// Finalization requires the candidate's real inline evidence to already
/// exist -- it can never create promotion evidence from scratch.
#[test]
fn finalize_fails_when_backtest_evidence_was_never_generated() {
    let out_dir = fresh_out_dir("missing");
    std::fs::create_dir_all(&out_dir).unwrap();
    let fake_run_id = Uuid::new_v4();

    let output = run_cli(&[
        "backtest",
        "finalize-robustness-sensitivity",
        "--artifact-root",
        &out_dir.to_string_lossy(),
        "--run-id",
        &fake_run_id.to_string(),
        "--registry-db",
        "nonexistent_registry.sqlite3",
        "--trial-id",
        "irrelevant",
        // Values below are syntactically-valid placeholders only -- clap
        // parses ALL required args before the handler runs, and this test
        // must fail at the EARLIER `load_canonical_robustness_gauntlet`
        // step regardless of what these are, so none needs to be a real
        // registered judge artifact.
        "--judge-artifact-sha256",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "--research-py-root",
        &research_py_root().to_string_lossy(),
        "--python",
        "python",
        "--block-counts",
        "8,10",
        "--dsr-max-sensitivity-range",
        "0.25",
        "--pbo-max-sensitivity-range",
        "0.25",
    ]);

    assert!(
        !output.status.success(),
        "finalize must fail closed when no real backtest evidence exists for this run_id"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("robustness_gauntlet.json") || stderr.contains("must already be real"),
        "stderr:\n{stderr}"
    );
}
