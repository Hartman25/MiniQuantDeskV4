//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` / `PROMOTION-STRESS-AUTHORITY-REPAIR-01`
//! -- real cross-language DSR/PBO sensitivity integration.
//!
//! Proves the actual Rust<->Python subprocess wiring works end-to-end:
//! Rust spawns a REAL Python process running
//! `mqk_research.ml.dsr_pbo_sensitivity_cli` against a real (disposable,
//! SQLite) registry, and correctly classifies the result. Deliberately
//! narrow -- unit-level coverage of the Python CLI itself (evaluated /
//! not_evaluable / error, determinism, real 2-trial DSR/PBO computation)
//! lives in `research-py/tests/test_dsr_pbo_sensitivity_cli.py`; this file
//! exercises ONLY the Rust<->Python boundary.
//!
//! Requires a real `python` (or `MQK_RESEARCH_PYTHON`) with `research-py`'s
//! runtime deps (pandas/numpy) importable -- `#[ignore]`d like every other
//! environment-dependent test in this codebase; run with `--ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use mqk_backtest::dsr_pbo_sensitivity_scenario;

fn research_py_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("research-py")
}

fn python_executable() -> String {
    std::env::var("MQK_RESEARCH_PYTHON").unwrap_or_else(|_| "python".to_string())
}

/// Valid Python string-literal source for `s` (JSON string escaping is a
/// valid subset of Python string-literal escaping for the plain paths and
/// identifiers used here).
fn py_str_literal(s: &str) -> String {
    serde_json::to_string(s).expect("string always serializes")
}

/// Register ONE trial with zero attempts via a real Python subprocess using
/// `ResearchResultStore` directly -- the minimal REAL fixture that makes
/// the trial genuinely registered but NOT part of any judged comparison
/// scope (mirrors `build_multiple_testing_judge`'s own `never_attempted`
/// exclusion path -- see that function's `load_exclusions` handling).
fn register_never_attempted_trial(registry_db: &Path, trial_id: &str) {
    let script = format!(
        "from pathlib import Path\n\
         from mqk_research.exp_distributed.storage import ResearchResultStore\n\
         from mqk_research.ml.economic_walkforward import PROTOCOL_ID\n\
         store = ResearchResultStore(Path({db}))\n\
         store.register_hypothesis(hypothesis_id='hyp', experiment_id='exp.rust_it')\n\
         store.register_trial(\n\
         \ttrial_id={trial_id}, experiment_id='exp.rust_it', hypothesis_id='hyp',\n\
         \tstrategy_id='only', protocol_id=PROTOCOL_ID, identity={{'minimal': True}},\n\
         )\n",
        db = py_str_literal(&registry_db.display().to_string()),
        trial_id = py_str_literal(trial_id),
    );

    let src_dir = research_py_root().join("src");
    let output = Command::new(python_executable())
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
}

#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn dpsr01a_never_attempted_trial_is_genuinely_inapplicable() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_dpsr01a_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let registry_db = dir.join("registry.sqlite3");
    register_never_attempted_trial(&registry_db, "rust_it_never_attempted");

    let outcome = dsr_pbo_sensitivity_scenario(
        &python_executable(),
        &research_py_root(),
        &registry_db,
        "rust_it_never_attempted",
        &[8, 10],
    );

    assert_eq!(outcome.name, "dsr_pbo_sensitivity");
    assert!(
        !outcome.applicable,
        "a never-attempted trial has no comparison scope at all -- must be genuinely \
         inapplicable, not a failure: {outcome:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn dpsr01b_unknown_trial_id_is_a_real_failure_not_a_silent_pass() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_dpsr01b_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Registry file created (empty DB, schema initialized) but no such
    // trial was ever registered.
    register_never_attempted_trial(&dir.join("registry.sqlite3"), "some_other_trial");
    let registry_db = dir.join("registry.sqlite3");

    let outcome = dsr_pbo_sensitivity_scenario(
        &python_executable(),
        &research_py_root(),
        &registry_db,
        "totally_unknown_trial_id",
        &[8, 10],
    );

    assert_eq!(outcome.name, "dsr_pbo_sensitivity");
    assert!(outcome.applicable, "an operational error is never inapplicable");
    assert!(!outcome.passed, "an unknown trial_id must never silently pass: {outcome:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires a real python with research-py's runtime deps (pandas/numpy) importable"]
fn dpsr01c_bad_python_executable_fails_closed_not_a_panic() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_dpsr01c_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let registry_db = dir.join("registry.sqlite3");
    register_never_attempted_trial(&registry_db, "trial_for_bad_python_test");

    let outcome = dsr_pbo_sensitivity_scenario(
        "mqk_definitely_not_a_real_python_executable_xyz",
        &research_py_root(),
        &registry_db,
        "trial_for_bad_python_test",
        &[8, 10],
    );

    assert!(outcome.applicable);
    assert!(!outcome.passed, "a spawn failure must fail closed, never silently pass: {outcome:?}");
    assert!(outcome.reason.unwrap_or_default().contains("failed to spawn"));

    let _ = std::fs::remove_dir_all(&dir);
}
